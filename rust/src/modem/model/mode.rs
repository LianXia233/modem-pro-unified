//! 组网模式偏好（`GET /api/mode`）。
//!
//! 各厂商指令不同：移远 `AT+QNWPREFCFG="mode_pref"`、芯讯通 `AT+CNMP?`，
//! 都不支持时回落只读的 `AT+CFUN?`（仅给出运行状态，不当作模式可选）。

use std::time::Duration;

use serde::Serialize;

use crate::error::{msg, Result};

use super::super::at::{csv, extract_field, is_ok, Prio};
use super::super::vendor::Vendor;
use super::modem::Modem;

/// 一个可选的组网模式。
#[derive(Debug, Clone, Serialize)]
pub struct ModeOption {
    pub value: String,
    pub label: String,
}

/// 组网模式偏好。
#[derive(Debug, Clone, Serialize)]
pub struct ModePref {
    pub value: String,
    pub value_text: String,
    pub options: Vec<ModeOption>,
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Modem {
    /// 读取组网模式偏好（**走 30 秒缓存**）。
    ///
    /// 切制式是低频操作，但页面会反复问；缓存掉省串口。
    /// 写之后的回读用 [`Self::mode_pref_refresh`]。
    pub fn mode_pref(&self) -> ModePref {
        if let Some(v) = self.mode_cache.get() {
            return v;
        }
        let v = self.read_mode_pref();
        self.mode_cache.set(v.clone());
        v
    }

    /// 强制实时读取（写之后的回读确认用）。
    pub fn mode_pref_refresh(&self) -> ModePref {
        self.mode_cache.invalidate();
        self.mode_pref()
    }

    fn read_mode_pref(&self) -> ModePref {
        let vendor = self.vendor();
        // 各家读哪条、用哪个解析分支，统一在 `VendorSpec::mode_read` 里
        let (cmd, source) = vendor.spec().mode_read();
        if cmd.is_empty() {
            // 这家没有可用的读法：**不发空指令**，直接诚实降级
            return ModePref {
                value: String::new(),
                value_text: String::new(),
                options: mode_options_for(vendor),
                supported: false,
                raw: None,
                note: Some(format!(
                    "{} 的组网模式读法未取证（不发未经验证的私有指令）",
                    vendor.label()
                )),
            };
        }
        let resp = self
            .with_at_prio(Prio::Data, |p| p.command(cmd, self.cmd_timeout))
            .unwrap_or_default();
        if is_ok(&resp) {
            if let Some(value) = parse_mode_value(&resp, source) {
                return ModePref {
                    value_text: mode_text(&value),
                    value,
                    options: mode_options_for(vendor),
                    supported: true,
                    raw: Some(resp.trim().to_string()),
                    note: None,
                };
            }
        }
        ModePref {
            value: String::new(),
            value_text: String::new(),
            options: mode_options_for(vendor),
            supported: false,
            raw: Some(resp.trim().to_string()),
            note: Some("固件未提供可读写的组网模式指令".to_string()),
        }
    }

    /// 下发组网模式偏好（`AT+QNWPREFCFG="mode_pref",<value>` / 芯讯通 `AT+CNMP=<value>`）。
    ///
    /// `value` 由界面直接给（语义值或本机原码都行），写完**回读确认**：
    /// 切换制式会让模组重新选网、短暂断网，必须让界面看到模组的真实返回值。
    pub fn set_mode_pref(&self, value: &str) -> Result<ModePref> {
        let value = value.trim();
        if value.is_empty() {
            return Err(msg("先读一次当前值（该模组未提供可用指令）"));
        }
        let vendor = self.vendor();
        let cmd = match vendor {
            Vendor::Simcom => format!("AT+CNMP={}", cnmp_code(value)),
            // 鼎桥：读-改-写（其余字段回读后原样带回去），acqorder 换成目标档
            Vendor::TDTech => {
                let cur = self
                    .with_at_prio(Prio::Data, |p| p.command("AT^SYSCFGEX?", self.cmd_timeout))
                    .unwrap_or_default();
                let Some((acq, band, roam, srv, lteband)) = parse_syscfgex(&cur) else {
                    return Err(msg("读不到当前的 AT^SYSCFGEX 配置，拒绝盲写"));
                };
                let Some(new_acq) = tdtech_acq_order(value) else {
                    return Err(msg(format!("鼎桥不支持的目标模式：{value}")));
                };
                let _ = acq;
                format!("AT^SYSCFGEX=\"{new_acq}\",{band},{roam},{srv},{lteband}")
            }
            // 展锐：5G 开关（1 = 5G+4G、0 = 仅 4G）
            Vendor::Unisoc => match value.to_ascii_uppercase().as_str() {
                "LTE:NR5G" | "NR5G:LTE" => "AT+SP5GRAN=1".to_string(),
                "LTE" => "AT+SP5GRAN=0".to_string(),
                other => return Err(msg(format!("展锐不支持的目标模式：{other}"))),
            },
            // 百旺 QDC507：位标志写入（参考实现 baiwang_control.lua 的 set_network）
            Vendor::Baiwang => {
                let Some(code) = baiwang_nwscanmodeex_code(value) else {
                    return Err(msg(format!("QDC507 不支持的目标模式：{value}")));
                };
                format!("AT+QCFG=\"nwscanmodeex\",{code}")
            }
            _ => format!("AT+QNWPREFCFG=\"mode_pref\",{value}"),
        };
        let resp = self.with_at_prio(Prio::Data, |p| p.command(&cmd, Duration::from_secs(10)))?;
        if !is_ok(&resp) {
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        let matches = |p: &ModePref| !p.value.is_empty() && p.value.eq_ignore_ascii_case(value);
        let pref = self.mode_pref_refresh();
        if matches(&pref) {
            return Ok(pref);
        }
        // QDC507 先应答、NAS 偏好后可读还有延迟（参考实现轮询 5×1s 后断言）。
        // 这里最多再等 3 秒；仍读不回来就按模组真实状态报错，不假报成功。
        if vendor == Vendor::Baiwang {
            for _ in 0..3 {
                std::thread::sleep(Duration::from_secs(1));
                let p = self.mode_pref_refresh();
                if matches(&p) {
                    return Ok(p);
                }
            }
        }
        // 回读不到目标值时不算成功：界面会照模组的真实状态显示
        Err(msg(format!(
            "已下发但回读为「{}」（目标 {value}）",
            if pref.value.is_empty() {
                "空"
            } else {
                &pref.value
            }
        )))
    }
}

/// 语义值 → 芯讯通 `AT+CNMP` 的原码（不认识的按 AUTO=2）。
fn cnmp_code(value: &str) -> &'static str {
    match value.to_ascii_uppercase().as_str() {
        "LTE" => "38",
        "NR5G" => "71",
        "LTE:NR5G" | "NR5G:LTE" => "109",
        _ => "2",
    }
}

/// 语义值 → 鼎桥 `AT^SYSCFGEX` 的 acqorder（实机当前值 "080302" = NR→LTE→WCDMA = 自动）。
pub fn tdtech_acq_order(value: &str) -> Option<&'static str> {
    match value.to_ascii_uppercase().as_str() {
        "AUTO" => Some("080302"),
        "NR5G" => Some("08"),
        "LTE" => Some("03"),
        "LTE:NR5G" | "NR5G:LTE" => Some("0803"),
        _ => None,
    }
}

/// 鼎桥 acqorder → 语义值（认不出就不报：宁可不显示，也不显示错的数据）。
pub fn tdtech_mode_from_acq(acq: &str) -> Option<&'static str> {
    match acq.trim() {
        "080302" => Some("AUTO"),
        "08" => Some("NR5G"),
        "03" => Some("LTE"),
        "0803" => Some("LTE:NR5G"),
        _ => None,
    }
}

/// 解析 `^SYSCFGEX: "<acqorder>",<band>,<roam>,<srvdomain>,<lteband>` 五个字段。
pub fn parse_syscfgex(resp: &str) -> Option<(String, String, String, String, String)> {
    let field = extract_field(resp, "^SYSCFGEX")?;
    let parts = csv(&field);
    let acq = parts.first()?.trim_matches('"').to_string();
    let band = parts.get(1)?.clone();
    let roam = parts.get(2)?.clone();
    let srv = parts.get(3)?.clone();
    let lteband = parts.get(4)?.clone();
    if band.is_empty() || roam.is_empty() || srv.is_empty() || lteband.is_empty() {
        return None;
    }
    Some((acq, band, roam, srv, lteband))
}

/// 展锐 `AT+SPRAT?` 应答 → 语义值（实测：LTE 32 = 已开 5G、LTE 16 = 仅 4G）。
pub fn unisoc_mode_from_sprat(resp: &str) -> Option<&'static str> {
    let field = extract_field(resp, "+SPRAT")?;
    let token = field.split_whitespace().last()?;
    match token {
        "32" => Some("LTE:NR5G"),
        "16" => Some("LTE"),
        _ => None,
    }
}

/// 百旺 QDC507 `AT+QCFG="nwscanmodeex"` 应答 → 语义值。
///
/// 位标志来自参考实现 `baiwang_control.lua` 的 `values` 表（`2G=4` / `3G=8` /
/// `4G=16`，实机 `QDC507GLEFM21`）：`+QCFG: "nwscanmodeex",24` = 4G + 3G。
/// 0（没有任何制式）与未知位一律返回 `None` —— 宁可不显示，也不显示错的数据。
pub fn baiwang_mode_from_nwscanmodeex(resp: &str) -> Option<String> {
    let field = extract_field(resp, "+QCFG")?;
    let parts = csv(&field);
    if parts.first()?.trim_matches('"') != "nwscanmodeex" {
        return None;
    }
    let n: i32 = parts.get(1)?.trim().parse().ok()?;
    baiwang_mode_from_bits(n)
}

/// 位标志 → 归一化语义值（制式从高到低，与 `LTE:NR5G` 的写法一致）。
fn baiwang_mode_from_bits(n: i32) -> Option<String> {
    // 只认 2G/3G/4G 三位（4/8/16）；0 = 没开任何制式，未知位不猜
    if n <= 0 || n & !0b1_1100 != 0 {
        return None;
    }
    let mut v = Vec::new();
    if n & 16 != 0 {
        v.push("LTE");
    }
    if n & 8 != 0 {
        v.push("WCDMA");
    }
    if n & 4 != 0 {
        v.push("GSM");
    }
    Some(v.join(":"))
}

/// 语义值 → `AT+QCFG="nwscanmodeex"` 位标志（与 [`baiwang_mode_from_bits`] 互逆）。
pub fn baiwang_nwscanmodeex_code(value: &str) -> Option<i32> {
    let mut n = 0;
    for part in value.to_ascii_uppercase().split(':') {
        n |= match part.trim() {
            "LTE" => 16,
            "WCDMA" => 8,
            "GSM" => 4,
            _ => return None,
        };
    }
    (n > 0).then_some(n)
}

/// 从应答里取模式取值。
pub fn parse_mode_value(resp: &str, source: &str) -> Option<String> {
    match source {
        "cnmp" => {
            let field = extract_field(resp, "+CNMP")?;
            let v: i32 = csv(&field).first()?.parse().ok()?;
            Some(cnmp_value(v).to_string())
        }
        "syscfgex" => {
            let (acq, ..) = parse_syscfgex(resp)?;
            Some(tdtech_mode_from_acq(&acq)?.to_string())
        }
        "sprat" => Some(unisoc_mode_from_sprat(resp)?.to_string()),
        // 华为：`^SYSCFGEX: "03",40000000,1,2,...` 的第 1 段是 acqorder 十六进制串
        "huawei_acq" => huawei_mode_from_acq(resp),
        // Telit：3GPP WS46 数字码
        "ws46" => telit_mode_from_ws46(resp),
        // Sierra：`!SELRAT: <n>`
        "selrat" => sierra_mode_from_selrat(resp),
        // 百旺 QDC507：`+QCFG: "nwscanmodeex",<位标志>`
        "baiwang_nwscan" => baiwang_mode_from_nwscanmodeex(resp),
        _ => {
            let field = extract_field(resp, "+QNWPREFCFG")?;
            let parts = csv(&field);
            parts.get(1).filter(|s| !s.is_empty()).cloned()
        }
    }
}

/// 华为 `AT^SYSCFGEX?` → 归一化取值。
///
/// 字段 1 是 acqorder（十六进制串）：`"00"` = 自动、`"03"` = 仅 LTE。
/// 其余取值（GSM/WCDMA 单制式、含 NR 的组合）语义没有逐条取证，
/// 返回 `None` 让上层判「不可读」，而不是猜一个错的档位。
fn huawei_mode_from_acq(resp: &str) -> Option<String> {
    let field = extract_field(resp, "^SYSCFGEX")?;
    let acq = csv(&field)
        .first()
        .map(|s| s.trim_matches('"').trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())?;
    match acq.as_str() {
        "00" | "0" => Some("AUTO".to_string()),
        "03" | "3" => Some("LTE".to_string()),
        _ => None,
    }
}

/// Telit `AT+WS46?` → 归一化取值（3GPP TS 27.007 的 WS46 表）。
fn telit_mode_from_ws46(resp: &str) -> Option<String> {
    let field = extract_field(resp, "+WS46")?;
    let n: i32 = csv(&field).first()?.trim().parse().ok()?;
    match n {
        22 => Some("LTE".to_string()),
        28 => Some("LTE:NR5G".to_string()),
        _ => None,
    }
}

/// Sierra `AT!SELRAT?` → 归一化取值。
fn sierra_mode_from_selrat(resp: &str) -> Option<String> {
    let field = extract_field(resp, "!SELRAT")?;
    let n: i32 = csv(&field).first()?.trim().parse().ok()?;
    match n {
        0 | 6 => Some("AUTO".to_string()),
        14 => Some("LTE".to_string()),
        _ => None,
    }
}

/// 芯讯通 `AT+CNMP` 取值 → 归一化取值。2=自动，38=LTE，71=NR5G，109=LTE:NR5G。
fn cnmp_value(v: i32) -> &'static str {
    match v {
        38 => "LTE",
        71 => "NR5G",
        109 => "LTE:NR5G",
        _ => "AUTO",
    }
}

/// 取值 → 中文说明。
pub fn mode_text(value: &str) -> String {
    match value.to_ascii_uppercase().as_str() {
        "AUTO" => "自动".to_string(),
        "LTE" => "仅 4G LTE".to_string(),
        "NR5G" => "仅 5G NR".to_string(),
        "LTE:NR5G" | "NR5G:LTE" => "4G + 5G".to_string(),
        "WCDMA" => "仅 3G WCDMA".to_string(),
        "GSM" => "仅 2G GSM".to_string(),
        // 百旺 QDC507 的组合写法（制式从高到低）
        "LTE:WCDMA" => "4G + 3G".to_string(),
        "LTE:GSM" => "4G + 2G".to_string(),
        "LTE:WCDMA:GSM" => "4G + 3G + 2G".to_string(),
        "WCDMA:GSM" => "3G + 2G".to_string(),
        "" => String::new(),
        other => other.to_string(),
    }
}

/// 统一的可选项（界面下拉）。
pub fn mode_options() -> Vec<ModeOption> {
    vec![
        ModeOption {
            value: "AUTO".into(),
            label: "自动".into(),
        },
        ModeOption {
            value: "LTE".into(),
            label: "仅 4G LTE".into(),
        },
        ModeOption {
            value: "NR5G".into(),
            label: "仅 5G NR".into(),
        },
        ModeOption {
            value: "LTE:NR5G".into(),
            label: "4G + 5G".into(),
        },
    ]
}

/// 按厂商给可选项：展锐只有 5G 开关这两档（界面就是它）。
pub fn mode_options_for(vendor: Vendor) -> Vec<ModeOption> {
    match vendor {
        Vendor::Unisoc => vec![
            ModeOption {
                value: "LTE:NR5G".into(),
                label: "5G + 4G".into(),
            },
            ModeOption {
                value: "LTE".into(),
                label: "仅 4G".into(),
            },
        ],
        // 百旺 QDC507 是 LTE 模组：位标志只给含 LTE 的组合（参考实现
        // baiwang_control.lua 允许 2G/3G/4G 任意非空组合，实机是 LTE 硬件）
        Vendor::Baiwang => vec![
            ModeOption {
                value: "LTE".into(),
                label: "仅 4G LTE".into(),
            },
            ModeOption {
                value: "LTE:WCDMA".into(),
                label: "4G + 3G".into(),
            },
            ModeOption {
                value: "LTE:GSM".into(),
                label: "4G + 2G".into(),
            },
            ModeOption {
                value: "LTE:WCDMA:GSM".into(),
                label: "4G + 3G + 2G".into(),
            },
        ],
        _ => mode_options(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        baiwang_mode_from_bits, baiwang_mode_from_nwscanmodeex, baiwang_nwscanmodeex_code,
        cnmp_code, huawei_mode_from_acq, mode_options_for, mode_text, parse_mode_value,
        parse_syscfgex, sierra_mode_from_selrat, tdtech_acq_order, tdtech_mode_from_acq,
        telit_mode_from_ws46, unisoc_mode_from_sprat,
    };
    use crate::modem::vendor::Vendor;

    /// 百旺 QDC507：`AT+QCFG="nwscanmodeex"` 位标志 ↔ 语义值互逆。
    ///
    /// 位标志依据参考实现 `baiwang_control.lua` 的 values 表（2G=4 / 3G=8 / 4G=16）。
    #[test]
    fn baiwang_nwscanmodeex_bits_round_trip() {
        // 读：应答 → 语义值
        assert_eq!(
            baiwang_mode_from_nwscanmodeex("+QCFG: \"nwscanmodeex\",16\r\nOK").as_deref(),
            Some("LTE")
        );
        assert_eq!(
            baiwang_mode_from_nwscanmodeex("+QCFG: \"nwscanmodeex\",24\r\nOK").as_deref(),
            Some("LTE:WCDMA")
        );
        assert_eq!(
            baiwang_mode_from_nwscanmodeex("+QCFG: \"nwscanmodeex\",28\r\nOK").as_deref(),
            Some("LTE:WCDMA:GSM")
        );
        assert_eq!(
            parse_mode_value("+QCFG: \"nwscanmodeex\",20\r\nOK", "baiwang_nwscan").as_deref(),
            Some("LTE:GSM")
        );
        // 0 = 没开任何制式（参考实现拒绝 mode=0），未知位不猜
        assert_eq!(baiwang_mode_from_bits(0), None);
        assert_eq!(baiwang_mode_from_bits(32), None, "未知位不猜");
        assert_eq!(baiwang_mode_from_bits(-1), None);
        // 不是 nwscanmodeex 的 QCFG 应答不接
        assert_eq!(
            baiwang_mode_from_nwscanmodeex("+QCFG: \"usbnet\",1\r\nOK"),
            None
        );
        // 写：语义值 → 位标志，与读互逆
        for v in [
            "LTE",
            "LTE:WCDMA",
            "LTE:GSM",
            "LTE:WCDMA:GSM",
            "WCDMA",
            "GSM",
        ] {
            let code = baiwang_nwscanmodeex_code(v).expect(v);
            assert_eq!(baiwang_mode_from_bits(code).as_deref(), Some(v));
        }
        assert_eq!(baiwang_nwscanmodeex_code("AUTO"), None);
        assert_eq!(baiwang_nwscanmodeex_code(""), None);
        // 下拉只给含 LTE 的组合（实机是 LTE 硬件）
        let opts = mode_options_for(Vendor::Baiwang);
        assert!(opts.iter().all(|o| o.value.starts_with("LTE")));
        assert_eq!(mode_text("LTE:WCDMA"), "4G + 3G");
    }

    #[test]
    fn new_vendor_mode_parsers() {
        // 华为 acqorder："00" 自动、"03" 仅 LTE、其它不猜
        assert_eq!(
            huawei_mode_from_acq("^SYSCFGEX: \"00\",40000000,1,2,40000000,,").as_deref(),
            Some("AUTO")
        );
        assert_eq!(
            huawei_mode_from_acq("^SYSCFGEX: \"03\",40000000,1,2,40000000,,").as_deref(),
            Some("LTE")
        );
        assert_eq!(
            huawei_mode_from_acq("^SYSCFGEX: \"02\",40000000,1,2,40000000,,"),
            None,
            "WCDMA 单制式没取证，不能猜成 AUTO"
        );

        // Telit WS46：22 = LTE、28 = LTE+NR5G
        assert_eq!(telit_mode_from_ws46("+WS46: 22").as_deref(), Some("LTE"));
        assert_eq!(
            telit_mode_from_ws46("+WS46: 28").as_deref(),
            Some("LTE:NR5G")
        );
        assert_eq!(telit_mode_from_ws46("+WS46: 25"), None);

        // Sierra SELRAT：0/6 = 自动、14 = LTE
        assert_eq!(
            sierra_mode_from_selrat("!SELRAT: 00").as_deref(),
            Some("AUTO")
        );
        assert_eq!(
            sierra_mode_from_selrat("!SELRAT: 06").as_deref(),
            Some("AUTO")
        );
        assert_eq!(
            sierra_mode_from_selrat("!SELRAT: 14").as_deref(),
            Some("LTE")
        );
        assert_eq!(sierra_mode_from_selrat("!SELRAT: 99"), None);
    }

    #[test]
    fn qcf_value() {
        let v = parse_mode_value("+QNWPREFCFG: \"mode_pref\",AUTO\r\nOK", "qcf");
        assert_eq!(v.as_deref(), Some("AUTO"));
    }

    #[test]
    fn cnmp_value() {
        assert_eq!(
            parse_mode_value("+CNMP: 38\r\nOK", "cnmp").as_deref(),
            Some("LTE")
        );
        assert_eq!(
            parse_mode_value("+CNMP: 2\r\nOK", "cnmp").as_deref(),
            Some("AUTO")
        );
    }

    #[test]
    fn cnmp_codes() {
        assert_eq!(cnmp_code("LTE"), "38");
        assert_eq!(cnmp_code("NR5G"), "71");
        assert_eq!(cnmp_code("lte:nr5g"), "109");
        assert_eq!(cnmp_code("乱写"), "2");
    }

    #[test]
    fn text_mapping() {
        assert_eq!(mode_text("NR5G"), "仅 5G NR");
        assert_eq!(mode_text("lte:nr5g"), "4G + 5G");
    }

    #[test]
    fn tdtech_acqorder_roundtrip() {
        // 实机当前值 "080302"（NR→LTE→WCDMA）= 界面上的 AUTO
        assert_eq!(tdtech_mode_from_acq("080302"), Some("AUTO"));
        assert_eq!(tdtech_mode_from_acq("08"), Some("NR5G"));
        assert_eq!(tdtech_mode_from_acq("03"), Some("LTE"));
        assert_eq!(tdtech_mode_from_acq("0803"), Some("LTE:NR5G"));
        // 只有 2G 的配置不该映射成任何界面模式
        assert_eq!(tdtech_mode_from_acq("01"), None);
        assert_eq!(tdtech_acq_order("AUTO"), Some("080302"));
        assert_eq!(tdtech_acq_order("lte"), Some("03"));
        assert_eq!(tdtech_acq_order("BOGUS"), None);
    }

    #[test]
    fn tdtech_syscfgex_parse() {
        let (acq, band, roam, srv, lteband) = parse_syscfgex(
            "AT^SYSCFGEX?\r\n^SYSCFGEX: \"080302\",2000000680380,1,2,1E200000095\r\nOK",
        )
        .expect("应解析出五个字段");
        assert_eq!(acq, "080302");
        assert_eq!(band, "2000000680380");
        assert_eq!(roam, "1");
        assert_eq!(srv, "2");
        assert_eq!(lteband, "1E200000095");
        assert_eq!(
            parse_mode_value(
                "^SYSCFGEX: \"080302\",2000000680380,1,2,1E200000095",
                "syscfgex"
            )
            .as_deref(),
            Some("AUTO")
        );
        assert_eq!(parse_syscfgex("ERROR"), None);
    }

    #[test]
    fn unisoc_sprat_parse() {
        assert_eq!(
            parse_mode_value("+SPRAT: LTE 32\r\nOK", "sprat").as_deref(),
            Some("LTE:NR5G")
        );
        assert_eq!(
            parse_mode_value("+SPRAT: LTE 16\r\nOK", "sprat").as_deref(),
            Some("LTE")
        );
        assert_eq!(unisoc_mode_from_sprat("+SPRAT: LTE 7"), None);
        // 展锐只有两档可选
        assert_eq!(mode_options_for(Vendor::Unisoc).len(), 2);
        assert_eq!(mode_options_for(Vendor::Quectel).len(), 4);
    }
}
