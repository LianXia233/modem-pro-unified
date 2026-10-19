//! 逐型号 AT 指令面（`GET /api/modules?model=…` / `?modem_id=…`）。
//!
//! ## 设计
//!
//! 中央只做**装配**：基础 3GPP 指令 + 厂商模块自己声明的条目 + 支持清单里的
//! USB/PCIe 模式切换 AT。各厂商的差异写在 `modem/vendor/<厂商>.rs` 的
//! [`VendorSpec::at_plan_commands`] 里 —— 加一家厂商只动一个文件。
//!
//! ## 与上游 Go 版的关系
//!
//! 上游 `modem_at_plan.go`（35K）把「能力 → 指令 → 解析函数」逐条反射出来。
//! Rust 版沿用同一份字段契约（`key/label/cmd/candidates/parser/format/on/source/note`），
//! 但指令来源改为**本仓库的真实实现**：
//!
//! - 公共能力（身份/信号/注册/运营商/短信/扫网/USSD/飞行模式）走 3GPP 默认；
//! - 厂商能力（服务小区/邻区/CA/温度/流量/锁频段/组网模式/USB/IMEI）由各厂商
//!   模块用 `vendor_cmds()` 与现有钩子声明；
//! - USB/PCIe 模式切换以支持清单的 `usb_at` / `pcie_at` 为准（按型号精确）。
//!
//! 没实现的条目**不出现**，或在 `gaps` 里写清原因 —— 不生成「愿望表」。

use serde::Serialize;

use crate::modem::bands;
use crate::modem::catalog;
use crate::modem::variant;
use crate::modem::vendor::Vendor;

/// 一条能力/指令。
#[derive(Debug, Clone, Serialize)]
pub struct AtCmdSpec {
    pub key: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cmd: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub enable_cmd: String,
    pub parser: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub format: String,
    pub on: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl AtCmdSpec {
    pub fn on(
        key: &str,
        label: &str,
        cmd: &str,
        parser: &str,
        format: &str,
        source: &str,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            cmd: cmd.into(),
            candidates: Vec::new(),
            enable_cmd: String::new(),
            parser: parser.into(),
            format: format.into(),
            on: true,
            source: source.into(),
            note: String::new(),
        }
    }

    pub fn off(key: &str, label: &str, note: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            cmd: String::new(),
            candidates: Vec::new(),
            enable_cmd: String::new(),
            parser: String::new(),
            format: String::new(),
            on: false,
            source: "未接入".into(),
            note: note.into(),
        }
    }
}

/// 计划上下文（厂商 + 型号 + 平台）。
pub struct PlanCtx<'a> {
    pub vendor: Vendor,
    pub model: &'a str,
    pub platform: &'a str,
    /// 广和通 MTK 平台（与 `Modem::is_fibocom_mtk` 同口径）
    pub mtk: bool,
    /// 平台=lte 的旧款（锁频段走 QCFG 位图）
    pub legacy_lte: bool,
}

impl PlanCtx<'_> {
    fn spec(&self) -> &'static dyn crate::modem::vendor::VendorSpec {
        self.vendor.spec()
    }
}

/// 一个型号（或一台在网模组）的 AT 指令面。
#[derive(Debug, Clone, Serialize)]
pub struct ModemAtPlan {
    pub model: String,
    pub vendor: String,
    pub vendor_name: String,
    pub platform: String,
    pub platform_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub platform_variant: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub platform_variant_note: String,
    pub in_catalog: bool,
    pub live: bool,
    pub usb_modes: Vec<String>,
    pub pcie_modes: Vec<String>,
    pub usb_text: String,
    pub pcie_text: String,
    /// 本型号的支持频段（QModem 频段表，出厂口径）。
    ///
    /// `None` = 型号不在频段表里；`Some` 但四组全空 = 在表里、上游没写频段 ——
    /// 两种都不是「不支持任何频段」，`bands_note` 里有明确说法。
    pub bands: Option<bands::ModemBands>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bands_note: String,
    pub cmd_impl: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cmd_impl_note: String,
    pub lock_dialect: String,
    pub usb_dialect: String,
    pub mode_dialect: String,
    pub imei_dialect: String,
    pub commands: Vec<AtCmdSpec>,
    pub gaps: Vec<String>,
    pub gap_notes: Vec<String>,
}

/// 基础 3GPP + 公共钩子（各厂商从这里起步，再按需替换/追加）。
pub fn base_specs(ctx: &PlanCtx) -> Vec<AtCmdSpec> {
    let sp = ctx.spec();
    let mut v = Vec::new();

    v.push(AtCmdSpec::on(
        "identity",
        "型号 / 固件",
        "ATI",
        "model::modem::identity",
        "ATI 多行文本（厂商/型号/固件）",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "serving",
        "服务小区",
        sp.serving_cell_cmd(),
        "model::cells::parse_serving",
        "厂商私有（QENG / CPSI / ^MONSC / CCED…）",
        "厂商指令集",
    ));
    v.push(AtCmdSpec::on(
        "netinfo",
        "网络接入信息",
        sp.net_info_cmd(),
        "model::network",
        "制式/频段/信道",
        "厂商指令集",
    ));
    let mut signal = AtCmdSpec::on(
        "signal",
        "信号（CSQ/CESQ）",
        "AT+CSQ",
        "model::signal::parse_csq",
        "rssi,ber（0-31/99）",
        "3GPP",
    );
    signal.candidates = vec!["AT+CSQ".into(), "AT+CESQ".into()];
    v.push(signal);
    let mut reg = AtCmdSpec::on(
        "reg",
        "注册状态",
        "AT+CEREG?",
        "model::regstate",
        "stat,lac,ci,act",
        "3GPP",
    );
    reg.candidates = vec!["AT+CEREG?".into(), "AT+CREG?".into()];
    v.push(reg);
    v.push(AtCmdSpec::on(
        "operator",
        "运营商",
        "AT+COPS?",
        "model::network::parse_operator",
        "mode,format,oper",
        "3GPP",
    ));

    // 温度 / 流量 / 组网模式：由厂商钩子给，没给就标未接入
    let temps = sp.temperature_cmds(ctx.mtk);
    if temps.is_empty() {
        v.push(AtCmdSpec::off("temp", "模组温度", "本厂商未提供温度指令"));
    } else {
        let mut s = AtCmdSpec::on(
            "temp",
            "模组温度",
            temps[0],
            "model::siminfo::parse_temp",
            "厂商私有（多传感器）",
            "厂商指令集",
        );
        s.candidates = temps.iter().map(|s| (*s).to_string()).collect();
        v.push(s);
    }
    let traffic = sp.traffic_candidates();
    if traffic.is_empty() {
        v.push(AtCmdSpec::off(
            "traffic",
            "流量计数 / 速率",
            "本厂商未提供流量指令",
        ));
    } else {
        let mut s = AtCmdSpec::on(
            "traffic",
            "流量计数 / 速率",
            traffic[0],
            "model::traffic",
            "累计字节 / 速率",
            "厂商指令集",
        );
        s.candidates = traffic.iter().map(|s| (*s).to_string()).collect();
        v.push(s);
    }
    v.push(AtCmdSpec::on(
        "voltage",
        "供电电压",
        "AT+CADC?",
        "model::siminfo::parse_voltage",
        "ADC 原始值 → mV",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "apn",
        "APN / 激活的 PDP",
        "AT+CGDCONT?",
        "model::params",
        "cid,type,apn",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "pdp_addr",
        "PDP 本机地址",
        "AT+CGPADDR=<cid>",
        "model::addr",
        "cid,ipv4[,ipv6]",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "iccid",
        "ICCID",
        "AT+CCID",
        "model::siminfo::parse_iccid",
        "裸数字或 +CCID:",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "imsi",
        "IMSI",
        "AT+CIMI",
        "model::siminfo::parse_imsi",
        "15 位数字",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "clock",
        "模组时钟",
        "AT+CCLK?",
        "model::siminfo::parse_clock",
        "yy/MM/dd,hh:mm:ss±zz",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "sms_read",
        "短信读取",
        "AT+CMGL=4",
        "model::sms::parse_pdu",
        "PDU 列表（CMGL=4）",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "sms_send",
        "短信发送",
        "AT+CMGS=<len>",
        "model::sms::send",
        "PDU + Ctrl-Z",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "netscan",
        "主动扫网",
        "AT+COPS=?",
        "model::netscan",
        "(stat,long,short,num,act)",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "ussd",
        "USSD",
        "AT+CUSD=1,\"<code>\",15",
        "model::ussd",
        "+CUSD: stat,text",
        "3GPP",
    ));
    v.push(AtCmdSpec::on(
        "airplane",
        "飞行模式",
        "AT+CFUN?",
        "model::airplane",
        "+CFUN: mode",
        "3GPP",
    ));

    // 组网模式偏好：厂商给了读指令才有
    let (mode_cmd, mode_branch) = sp.mode_read();
    if mode_cmd.is_empty() {
        v.push(AtCmdSpec::off(
            "lock_mode",
            "组网模式偏好",
            "本厂商未提供组网模式读指令",
        ));
    } else {
        v.push(AtCmdSpec::on(
            "lock_mode",
            "组网模式偏好",
            mode_cmd,
            &format!("model::mode::{mode_branch}"),
            "厂商私有枚举",
            "厂商指令集",
        ));
    }

    v
}

/// 在厂商指令表里找一条包含 `needle` 的指令，命中就追加。
///
/// 这是各厂商模块最常用的写法：指令表本身就是「设备上真正会用」的那份，
/// 不需要再抄一遍。
pub fn add_from_vendor(
    out: &mut Vec<AtCmdSpec>,
    ctx: &PlanCtx,
    key: &str,
    label: &str,
    needle: &str,
    parser: &str,
    format: &str,
) -> bool {
    let needle = needle.to_ascii_lowercase();
    if let Some(cmd) = ctx
        .spec()
        .vendor_cmds()
        .iter()
        .find(|c| c.to_ascii_lowercase().contains(&needle))
    {
        replace(
            out,
            AtCmdSpec::on(key, label, cmd, parser, format, "厂商指令集"),
        );
        true
    } else {
        false
    }
}

/// 同 key 覆盖，否则追加（保证一个能力只有一条最终结论）。
pub fn replace(out: &mut Vec<AtCmdSpec>, spec: AtCmdSpec) {
    if let Some(slot) = out.iter_mut().find(|s| s.key == spec.key) {
        *slot = spec;
    } else {
        out.push(spec);
    }
}

/// 按型号推导（`?model=`）。
pub fn for_model(model: &str, vendor_hint: Option<Vendor>, live: bool) -> ModemAtPlan {
    let entry = catalog::lookup(model);
    let mut vendor = entry
        .and_then(|e| Vendor::from_key(&e.vendor))
        .or(vendor_hint)
        .unwrap_or(Vendor::Unknown);
    // 清单里 `huawei` 同时包含华为与鼎桥：MT5700/MT5710 的 ATI 认成 TDTech
    let upper = model.to_uppercase();
    if vendor == Vendor::Huawei && (upper.contains("MT5700") || upper.contains("MT5710")) {
        vendor = Vendor::TDTech;
    }
    build(model, model, vendor, entry, live)
}

/// 按在网模组推导（`?modem_id=`）。
pub fn for_modem(modem: &crate::modem::model::Modem) -> ModemAtPlan {
    let model = modem.model().unwrap_or_default();
    let entry = catalog::lookup(&model);
    // 频段表的匹配键带上 ATI 原文：有些固件只在原文里报别名（如 QogirN6Pro_PS）
    let key = format!("{} {}", model, modem.model_raw().unwrap_or_default());
    build(&model, &key, modem.vendor(), entry, true)
}

fn build(
    model: &str,
    // 频段表匹配键（型号 + ATI 原文；离线时与 `model` 相同）。
    key: &str,
    vendor: Vendor,
    entry: Option<&'static catalog::Entry>,
    live: bool,
) -> ModemAtPlan {
    let platform = entry
        .map(|e| e.platform.clone())
        .unwrap_or_else(|| variant::platform_for(vendor, model));
    let mtk = platform == "mediatek" || model.to_uppercase().contains("FM350");
    let legacy_lte = variant::flavor_for(vendor, model) == variant::Flavor::LegacyLte;
    let ctx = PlanCtx {
        vendor,
        model,
        platform: &platform,
        mtk,
        legacy_lte,
    };

    let mut commands = vendor.spec().at_plan_commands(&ctx);
    let mut gaps = Vec::new();
    let mut gap_notes = Vec::new();

    // USB / PCIe 模式切换：以清单的按型号数据为准（有就覆盖基础里的同 key）
    if let Some(e) = entry {
        for side in ["usb", "pcie"] {
            let (modes, set) = if side == "usb" {
                (&e.usb, e.usb_at.as_ref())
            } else {
                (&e.pcie, e.pcie_at.as_ref())
            };
            if modes.is_empty() {
                continue;
            }
            if let Some(s) = set {
                if !s.query.is_empty() {
                    replace(
                        &mut commands,
                        AtCmdSpec::on(
                            &format!("{side}_query"),
                            &format!("查询 {} 模式", side.to_uppercase()),
                            &s.query,
                            "catalog::ModeAtSet",
                            "AT 查询（当前模式）",
                            "QModem 指令脚本",
                        ),
                    );
                }
            }
            let mut missing = Vec::new();
            for kind in modes {
                let cmd = if side == "usb" {
                    e.usb_command_for(kind)
                } else {
                    e.pcie_command_for(kind)
                };
                match cmd {
                    Some(cmd) => replace(
                        &mut commands,
                        AtCmdSpec::on(
                            &format!("{side}_{kind}"),
                            &format!("{} 切到 {}", side.to_uppercase(), catalog::kind_label(kind)),
                            cmd,
                            "catalog::ModeAtSet",
                            "完整 AT 指令（可直接下发）",
                            "QModem 指令脚本",
                        ),
                    ),
                    None => missing.push(catalog::kind_label(kind)),
                }
            }
            if !missing.is_empty() {
                gaps.push(format!(
                    "{} 模式切换：{}",
                    side.to_uppercase(),
                    missing.join(", ")
                ));
                gap_notes.push(
                    set.as_ref()
                        .map(|s| s.note.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| {
                            "QModem 未登记该形态的切换指令（只支持查询或由驱动决定）".into()
                        }),
                );
            }
        }
    } else {
        gaps.push("型号不在支持清单里：USB/PCIe 模式未知".into());
        gap_notes.push("请用 /api/vendor/cmds 查看该厂商的实际 AT 指令".into());
    }

    // 关闭的条目不进 commands（页面用 gaps 展示），保持列表只含真实会发的指令
    let off: Vec<AtCmdSpec> = commands.iter().filter(|c| !c.on).cloned().collect();
    commands.retain(|c| c.on);
    for c in off {
        gaps.push(format!("{}：{}", c.label, c.note));
    }

    let var = variant::lookup(vendor, model);
    ModemAtPlan {
        model: entry
            .map(|e| e.model.clone())
            .unwrap_or_else(|| model.to_string()),
        vendor: vendor.as_str().to_string(),
        vendor_name: vendor.label().to_string(),
        platform: platform.clone(),
        platform_name: catalog::platform_label(&platform),
        platform_variant: var.map(|v| v.label.to_string()).unwrap_or_default(),
        platform_variant_note: var.map(|v| v.evidence.to_string()).unwrap_or_default(),
        in_catalog: entry.is_some(),
        live,
        usb_modes: entry.map(|e| e.usb.clone()).unwrap_or_default(),
        pcie_modes: entry.map(|e| e.pcie.clone()).unwrap_or_default(),
        usb_text: entry.map(|e| e.usb_text()).unwrap_or_else(|| "—".into()),
        pcie_text: entry.map(|e| e.pcie_text()).unwrap_or_else(|| "—".into()),
        bands: bands::lookup(key).cloned(),
        bands_note: bands::note_for(key),
        cmd_impl: format!("rust::modem::vendor::{}", vendor.as_str()),
        cmd_impl_note: "指令来自厂商模块；USB/PCIe 切换来自 QModem 型号清单".into(),
        lock_dialect: var.map(|v| v.lock.to_string()).unwrap_or_default(),
        usb_dialect: var.map(|v| v.usb.to_string()).unwrap_or_default(),
        mode_dialect: var.map(|v| v.mode.to_string()).unwrap_or_default(),
        imei_dialect: var.map(|v| v.imei.to_string()).unwrap_or_default(),
        commands,
        gaps,
        gap_notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quectel_plan_has_core_and_vendor_commands() {
        let p = for_model("RM500Q-GL", None, false);
        assert_eq!(p.vendor, "quectel");
        assert_eq!(p.platform, "qualcomm");
        assert!(p.in_catalog);
        assert!(!p.platform_variant.is_empty());
        let keys: Vec<&str> = p.commands.iter().map(|c| c.key.as_str()).collect();
        for k in ["identity", "serving", "signal", "reg", "temp", "traffic", "usb_qmi"] {
            assert!(keys.contains(&k), "缺 {k}: {keys:?}");
        }
        // 型号清单的切换指令优先于厂商通用条目
        let qmi = p.commands.iter().find(|c| c.key == "usb_qmi").unwrap();
        assert_eq!(qmi.cmd, "AT+QCFG=\"usbnet\",0");
        assert_eq!(qmi.source, "QModem 指令脚本");
    }

    #[test]
    fn fibocom_mtk_plan_uses_rndis_and_egmrext() {
        let p = for_model("FM350-GL", None, false);
        assert_eq!(p.vendor, "fibocom");
        assert_eq!(p.platform_variant, "广和通 · 联发科");
        assert_eq!(p.usb_text, "RNDIS");
        assert_eq!(p.imei_dialect, "EGMREXT");
        let keys: Vec<&str> = p.commands.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"usb_rndis"));
        assert!(keys.contains(&"imei_set"));
    }

    #[test]
    fn huawei_plan_uses_phynum_and_hisilicon_variant() {
        let p = for_model("MT5700M-CN", None, false);
        assert_eq!(p.vendor, "tdtech");
        assert_eq!(p.platform, "hisilicon");
        assert_eq!(p.platform_variant, "华为 · 海思巴龙");
        let imei = p.commands.iter().find(|c| c.key == "imei_set").unwrap();
        assert!(imei.cmd.contains("PHYNUM"));
    }

    #[test]
    fn plan_carries_declared_bands() {
        // 有频段数据：四组数组 + 说明
        let p = for_model("FM350-GL", None, false);
        let b = p.bands.as_ref().expect("FM350-GL 在频段表里");
        assert_eq!(b.lte.len(), 19);
        assert!(b.nr_sa.contains(&78));
        assert!(p.bands_note.contains("LTE 19 个"), "{}", p.bands_note);

        // 在表里但上游没写频段：bands 有值、四组为空，说明不是「不支持」
        let p = for_model("T99W640", None, false);
        let b = p.bands.as_ref().expect("T99W640 在频段表里");
        assert!(!b.has_bands());
        assert!(p.bands_note.contains("没写频段"), "{}", p.bands_note);

        // 不在表里：bands=None，说明是「没登记」
        let p = for_model("XYZ-9999", Some(Vendor::Quectel), false);
        assert!(p.bands.is_none());
        assert!(p.bands_note.contains("没登记"), "{}", p.bands_note);
    }

    #[test]
    fn unknown_model_never_invents_commands() {
        let p = for_model("XYZ-9999", Some(Vendor::Quectel), false);
        assert!(!p.in_catalog);
        assert!(!p.commands.is_empty(), "厂商已知时仍给通用 3GPP 指令");
        assert!(p.gaps.iter().any(|g| g.contains("不在支持清单")));
        assert!(p.commands.iter().all(|c| c.key != "usb_qmi"));
    }
}
