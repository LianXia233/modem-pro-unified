//! 厂商识别与厂商专属能力（**一厂商一模块**）。
//!
//! 结构对齐上游的 `iface` + `module/<vendor>` 分层：
//!
//! ```text
//! vendor/
//!   mod.rs      —— Vendor 枚举 + VendorSpec 统一接口 + 注册表（本文件）
//!   quectel.rs  —— 移远：识别关键字 / AT 预设 / 流量 / 温度 / 厂商指令
//!   fibocom.rs  —— 广和通（含 FM350 的 MTK 平台差异）
//!   simcom.rs   —— 芯讯通（CNMP 模式、CNWSEARCH 扫小区、CPMUTEMP 温度）
//!   meig.rs     —— 美格
//!   tdtech.rs   —— 鼎桥（^ 私有指令族）
//!   unisoc.rs   —— 展锐（ATI 不支持、SP* 族）
//!   unknown.rs  —— 未识别：只给 3GPP 通用能力
//! ```
//!
//! ## 为什么这么拆
//!
//! 以前厂商差异散在 `at/presets.rs`、`http/handlers/vendor_cmds.rs`、
//! `model/traffic.rs`、`model/siminfo.rs`、`model/usbnet.rs` 里，各写一份
//! `match vendor { ... }` —— 加一家要改 5 处、漏一处还不报错。现在：
//!
//! - [`VendorSpec`] 是**唯一**的厂商能力接口，调用方只问 `vendor.spec()`；
//! - 每家一个文件，表与判定放在一起，改广和通不会碰到移远；
//! - 有默认实现的都给了保守默认（少给 = 不发未取证指令），新增厂商只写差异。
//!
//! ## 红线
//!
//! 1. **未取证就不发**：`VendorSpec` 的默认实现一律是「没有这个能力」，
//!    厂商模块只写**实测过**的指令（见各文件注释与 `PROGRESS.md`）。
//! 2. **识别顺序先中先得**：同一行里同时出现两家关键字时按 [`DETECT_ORDER`]。
//! 3. **MBIM 与厂商无关**：Basic Connect 是标准协议，内置实现统一处理
//!    （见 `runtime/dialer`）。

use serde::Serialize;

use crate::modem::at::AtPreset;

pub mod datacall;
pub mod fibocom;
pub mod foxconn;
pub mod gosuncn;
pub mod huawei;
pub mod meig;
pub mod neoway;
pub mod openluat;
pub mod quectel;
pub mod sierra;
pub mod simcom;
pub mod tdtech;
pub mod telit;
pub mod thales;
pub mod unisoc;
pub mod unknown;

/// 厂商标识（JSON 里是小写串）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Quectel,
    Fibocom,
    Simcom,
    Meig,
    TDTech,
    Unisoc,
    /// 华为（`^` 私有族：`AT^SYSCFGEX` / `AT^SYSINFOEX`）
    Huawei,
    /// 有方（`$` 私有族：`AT$MYSYSINFO` / `AT+NWSETBAND`）
    Neoway,
    /// Telit（`#` 私有族：`AT#CAINFOEXT` / `AT#BND`）
    Telit,
    /// Thales（`^` 私有族：`AT^DEBUG` / `AT^SLMODE`）
    Thales,
    /// Sierra Wireless（`!` 私有族：`AT!GSTATUS` / `AT!SELRAT`）
    Sierra,
    /// 富士康（`!` 私有族 + NV 写入）
    Foxconn,
    /// 高新兴（`Z` 私有族：`AT+ZBAND` / `AT+ZLOCKCELL`）
    Gosuncn,
    /// 合宙 Air 系列（`*` 私有族：`AT*BAND` / `AT+CCED`）
    Openluat,
    Unknown,
}

/// 识别顺序：先中先得。
/// 识别顺序：**先具体后宽泛**。
///
/// 关键：`TDTech` 排在 `Huawei` **之前** —— `MT5700M` 是两家共线型号，
/// 项目原有归属是鼎桥（`AT^MONSC` / `AT^CHIPTEMP` 那一族），华为模块因此
/// 不认 `MT5700M`（只认 `HUAWEI` / `MH5000` / `MT5710`）。
/// 同理 `Foxconn` 的 `T99W*` 与 Thales（共用 `AT^DEBUG`）要先认富士康。
pub const DETECT_ORDER: [Vendor; 14] = [
    Vendor::Quectel,
    Vendor::Fibocom,
    Vendor::Simcom,
    Vendor::Meig,
    Vendor::TDTech,
    Vendor::Huawei,
    Vendor::Unisoc,
    Vendor::Foxconn,
    Vendor::Thales,
    Vendor::Sierra,
    Vendor::Telit,
    Vendor::Gosuncn,
    Vendor::Neoway,
    Vendor::Openluat,
];

/// USB 网卡模式（`AT+QCFG="usbnet"`）的**写入策略**。
///
/// 读永远允许（只是查看）；写要看固件：有的厂商没有这条指令，有的值切过去会
/// 整机掉 USB —— 这些差异以前是 `model/usbnet.rs` 里的 `match vendor`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbnetWrite {
    /// 允许写。
    Allow,
    /// 拒绝写，附一句给人看的原因。
    Deny(&'static str),
}

/// 一家厂商的完整能力（统一接口）。
///
/// 默认实现刻意保守：没有这个能力就不给（`vendor_cmds` 为空、温度为空、
/// USB 网卡不可写），厂商模块只覆盖自己**实测过**的部分。
pub trait VendorSpec: Sync {
    /// 对应的标识。
    fn id(&self) -> Vendor;

    /// 界面上的可读名。
    fn label(&self) -> &'static str;

    /// 这一家的 `ATI`（或回落指令）应答里有没有它的关键字。
    fn detect(&self, lines: &[String]) -> bool;

    /// 服务小区/信号查询的首选指令（老接口，保留给调用方）。
    fn serving_cell_cmd(&self) -> &'static str {
        "AT+QENG=\"servingcell\""
    }

    /// 网络接入信息查询指令（`GET /api/netinfo`）。
    fn net_info_cmd(&self) -> &'static str {
        "AT+QNWINFO"
    }

    /// 这家厂商的私有 AT 预设表（`/api/at/presets`）。
    fn at_presets(&self) -> Vec<AtPreset> {
        Vec::new()
    }

    /// 这家厂商**不支持**的 3GPP 通用指令（合并预设时摘掉）。
    fn suppressed_common(&self) -> &'static [&'static str] {
        &[]
    }

    /// 这家厂商探测出的指令集（`/api/vendor/cmds`）。
    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[]
    }

    /// 流量计数的候选指令（按可信度排序）。
    ///
    /// 默认先试计数式 `AT+QGDCNT?`，再试速率式 `AT+GTSTATIS?`（未知机型兜底）。
    fn traffic_candidates(&self) -> &'static [&'static str] {
        &["AT+QGDCNT?", "AT+GTSTATIS?"]
    }

    /// 温度查询指令（按顺序试；`mtk_platform` 供广和通区分 MTK/高通）。
    fn temperature_cmds(&self, _mtk_platform: bool) -> &'static [&'static str] {
        &[]
    }

    /// USB 网卡模式能不能写这个值。
    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Allow
    }

    /// 组网模式偏好的读指令与解析分支（`("AT+…?", "分支名")`）。
    ///
    /// 默认**不发**（返回空指令）：`QNWPREFCFG` 只有移远/广和通/美格那一族认，
    /// 给别家发过去只会回 ERROR。各家在自己模块里显式声明；
    /// `model/mode.rs` 见到空指令直接判「本机不支持读」，不会往串口发空 AT。
    fn mode_read(&self) -> (&'static str, &'static str) {
        ("", "")
    }

    /// 主动扫小区指令（只有实测支持的那家给）。
    fn cell_scan_cmd(&self) -> Option<&'static str> {
        None
    }

    /// 模组侧数据通道（ECM / RNDIS / NCM）的拨号配方。
    ///
    /// 默认 `None` = 本厂商没有登记私有激活指令，调用方退回 3GPP 通用路径
    /// （`AT+CGACT`）。每家只写自己**有上游依据**的平台分支，平台不明时
    /// 由 [`datacall::plan`] 统一拦成通用路径 —— 不在厂商模块里猜。
    fn data_channel(&self, _ctx: &datacall::DataCallCtx<'_>) -> Option<datacall::DataChannel> {
        None
    }

    /// 本厂商的 AT 指令面（`/api/modules?model=` 用）。
    ///
    /// 默认 = 基础 3GPP + 公共钩子（服务小区/网络信息/温度/流量/组网模式）。
    /// 厂商模块覆盖它来声明自己的邻区/CA/ICCID/IMEI/锁频段等指令 ——
    /// **每家一个文件**，新增厂商不动中央装配。
    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        crate::modem::atplan::base_specs(ctx)
    }
}

impl Vendor {
    /// 取这家厂商的能力表（编译期常量，无分配）。
    pub fn spec(&self) -> &'static dyn VendorSpec {
        match self {
            Vendor::Quectel => &quectel::Spec,
            Vendor::Fibocom => &fibocom::Spec,
            Vendor::Simcom => &simcom::Spec,
            Vendor::Meig => &meig::Spec,
            Vendor::TDTech => &tdtech::Spec,
            Vendor::Unisoc => &unisoc::Spec,
            Vendor::Huawei => &huawei::Spec,
            Vendor::Neoway => &neoway::Spec,
            Vendor::Telit => &telit::Spec,
            Vendor::Thales => &thales::Spec,
            Vendor::Sierra => &sierra::Spec,
            Vendor::Foxconn => &foxconn::Spec,
            Vendor::Gosuncn => &gosuncn::Spec,
            Vendor::Openluat => &openluat::Spec,
            Vendor::Unknown => &unknown::Spec,
        }
    }

    /// 由 `ATI`（或回落指令）的应答**行**判定厂商。
    pub fn detect(lines: &[String]) -> Vendor {
        for v in DETECT_ORDER {
            if v.spec().detect(lines) {
                return v;
            }
        }
        Vendor::Unknown
    }

    /// 支持清单里的厂商标识（`quectel` / `fibocom` / `huawei`…）→ 枚举。
    ///
    /// 清单里 `huawei` 同时含华为与鼎桥：MT5700/MT5710 归 `TDTech`（ATI 也这么认）。
    pub fn from_key(key: &str) -> Option<Vendor> {
        Some(match key.trim().to_ascii_lowercase().as_str() {
            "quectel" => Vendor::Quectel,
            "fibocom" => Vendor::Fibocom,
            "simcom" => Vendor::Simcom,
            "meig" => Vendor::Meig,
            "huawei" => Vendor::Huawei,
            "tdtech" => Vendor::TDTech,
            "unisoc" => Vendor::Unisoc,
            "foxconn" => Vendor::Foxconn,
            "gosuncn" => Vendor::Gosuncn,
            "neoway" => Vendor::Neoway,
            "openluat" => Vendor::Openluat,
            "sierra" => Vendor::Sierra,
            "telit" => Vendor::Telit,
            "thales" => Vendor::Thales,
            "unknown" => Vendor::Unknown,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Vendor::Quectel => "quectel",
            Vendor::Fibocom => "fibocom",
            Vendor::Simcom => "simcom",
            Vendor::Meig => "meig",
            Vendor::TDTech => "tdtech",
            Vendor::Unisoc => "unisoc",
            Vendor::Huawei => "huawei",
            Vendor::Neoway => "neoway",
            Vendor::Telit => "telit",
            Vendor::Thales => "thales",
            Vendor::Sierra => "sierra",
            Vendor::Foxconn => "foxconn",
            Vendor::Gosuncn => "gosuncn",
            Vendor::Openluat => "openluat",
            Vendor::Unknown => "unknown",
        }
    }

    /// 厂商可读名（界面「厂商」一栏用）。
    pub fn label(&self) -> &'static str {
        self.spec().label()
    }

    /// 服务小区/信号查询的首选指令。
    pub fn serving_cell_cmd(&self) -> &'static str {
        self.spec().serving_cell_cmd()
    }

    /// 网络接入信息查询指令（`GET /api/netinfo`）。
    pub fn net_info_cmd(&self) -> &'static str {
        self.spec().net_info_cmd()
    }
}

/// 全大写一份（识别关键字比较用，避免每处 `to_uppercase` 写两遍）。
pub(crate) fn upper(s: &str) -> String {
    s.to_uppercase()
}

/// 逐行判断是否有任一关键字（识别用的公共小工具）。
pub(crate) fn any_line(lines: &[String], keys: &[&str]) -> bool {
    lines.iter().any(|l| {
        let u = upper(l);
        keys.iter().any(|k| u.contains(k))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_each_vendor() {
        assert_eq!(
            Vendor::detect(&lines(&["Quectel", "RM520N-GL"])),
            Vendor::Quectel
        );
        assert_eq!(
            Vendor::detect(&lines(&[
                "Manufacturer: Fibocom Wireless Inc.",
                "Model: FM160-EAU"
            ])),
            Vendor::Fibocom
        );
        // 厂名没出现，但典型机型在（广和通 FM650）
        assert_eq!(Vendor::detect(&lines(&["FM650-CN"])), Vendor::Fibocom);
        assert_eq!(Vendor::detect(&lines(&["RW350-CN"])), Vendor::Fibocom);
        assert_eq!(
            Vendor::detect(&lines(&["SIMCOM_SIM8200EA-M2"])),
            Vendor::Simcom
        );
        assert_eq!(Vendor::detect(&lines(&["SIM7900"])), Vendor::Simcom);
        assert_eq!(
            Vendor::detect(&lines(&["MeigSmart", "SRM815"])),
            Vendor::Meig
        );
        assert_eq!(Vendor::detect(&lines(&["SLM770"])), Vendor::Meig);
        assert_eq!(
            Vendor::detect(&lines(&["Manufacturer: TD Tech Ltd.", "Model: MT5700M-CN"])),
            Vendor::TDTech
        );
        // ---- 本轮新增的 8 家（方言族不同，逐个断言） ----
        assert_eq!(
            Vendor::detect(&lines(&["HUAWEI_MH5000-31"])),
            Vendor::Huawei
        );
        assert_eq!(Vendor::detect(&lines(&["NEOWAY N510M"])), Vendor::Neoway);
        assert_eq!(Vendor::detect(&lines(&["Telit FN990A40"])), Vendor::Telit);
        assert_eq!(Vendor::detect(&lines(&["THALES MV32-W-A"])), Vendor::Thales);
        assert_eq!(
            Vendor::detect(&lines(&["Sierra Wireless EM9190"])),
            Vendor::Sierra
        );
        assert_eq!(
            Vendor::detect(&lines(&["Foxconn T99W175"])),
            Vendor::Foxconn
        );
        assert_eq!(
            Vendor::detect(&lines(&["GOSUNCN ME3630-W"])),
            Vendor::Gosuncn
        );
        assert_eq!(Vendor::detect(&lines(&["Air724UG"])), Vendor::Openluat);

        // 展锐：ATI 不支持（回 +CME ERROR: 4），靠 CGMR 的平台标识认
        assert_eq!(
            Vendor::detect(&lines(&["Platform Version: MOCORTM_V2_22C_W25.39.4_Debug"])),
            Vendor::Unisoc
        );
        assert_eq!(Vendor::detect(&lines(&["ums9620_modem"])), Vendor::Unisoc);
        assert_eq!(Vendor::detect(&lines(&["+CME ERROR: 4"])), Vendor::Unknown);
    }

    #[test]
    fn order_first_match_wins() {
        // 同一行里同时出现两家关键字时按 DETECT_ORDER 先中先得
        assert_eq!(
            Vendor::detect(&lines(&["Quectel", "FM650"])),
            Vendor::Quectel
        );
        assert_eq!(
            Vendor::detect(&lines(&["SIMCOM", "MT5700"])),
            Vendor::Simcom
        );
    }

    #[test]
    fn every_vendor_has_a_spec_and_matching_id() {
        for v in DETECT_ORDER {
            let s = v.spec();
            assert_eq!(s.id(), v, "{v:?} 的 spec.id 对不上");
            assert!(!s.label().is_empty(), "{v:?} 缺 label");
            assert!(!v.as_str().is_empty());
        }
        assert_eq!(Vendor::Unknown.spec().id(), Vendor::Unknown);
    }

    #[test]
    fn unknown_only_gets_3gpp_capabilities() {
        let s = Vendor::Unknown.spec();
        assert!(s.at_presets().is_empty());
        assert_eq!(s.vendor_cmds(), &["AT+CSQ", "AT+CESQ", "AT+COPS?"]);
        assert_eq!(s.usbnet_write(1), UsbnetWrite::Allow);
        assert!(s.cell_scan_cmd().is_none());
    }

    #[test]
    fn new_vendors_declare_their_own_dialects() {
        use crate::modem::vendor::Vendor;
        // 每家的私有族前缀不同：认错族 = 发过去的指令全回 ERROR
        // (厂商, 小区指令, 模式读指令或空)
        let cases = [
            (Vendor::Huawei, "AT^SYSINFOEX", "AT^SYSCFGEX?"),
            (Vendor::Neoway, "AT+NETDMSGEX", ""),
            (Vendor::Telit, "AT#CAINFOEXT?", "AT+WS46?"),
            (Vendor::Thales, "AT^DEBUG", ""),
            (Vendor::Sierra, "AT!GSTATUS?", "AT!SELRAT?"),
            (Vendor::Foxconn, "AT^DEBUG", ""),
            (Vendor::Gosuncn, "AT+ZCELLINFO?", ""),
            (Vendor::Openluat, "AT+CCED=0,1", ""),
        ];
        for (v, cell, mode) in cases {
            let spec = v.spec();
            assert_eq!(spec.id(), v);
            assert!(!spec.label().is_empty(), "{v:?} 缺可读名");
            assert_eq!(spec.serving_cell_cmd(), cell, "{v:?} 的小区指令不对");
            assert_eq!(
                spec.mode_read().0,
                mode,
                "{v:?} 的模式读指令不对（没取证就必须留空，不能猜）"
            );
            // 预设表必须有内容（否则界面上这家一片空白）
            let presets = spec.at_presets();
            assert!(presets.len() >= 5, "{v:?} 预设太少（{} 条）", presets.len());
            // 至少有一条是这家私有族的指令（`^ $ # ! Z *` 之一）
            assert!(
                presets.iter().any(|p| p.cmd.contains('^')
                    || p.cmd.contains('$')
                    || p.cmd.contains('#')
                    || p.cmd.contains('!')
                    || p.cmd.contains('*')
                    || p.cmd.contains("+Z")),
                "{v:?} 的预设里没有任何私有族指令"
            );
        }
    }

    #[test]
    fn new_vendors_refuse_usb_mode_write() {
        use crate::modem::vendor::{UsbnetWrite, Vendor};
        // 新增 8 家的 USB 切换参数都没逐档取证：必须拒绝写入、只允许查看
        for v in [
            Vendor::Huawei,
            Vendor::Neoway,
            Vendor::Telit,
            Vendor::Thales,
            Vendor::Sierra,
            Vendor::Foxconn,
            Vendor::Gosuncn,
            Vendor::Openluat,
        ] {
            match v.spec().usbnet_write(1) {
                UsbnetWrite::Deny(why) => assert!(!why.is_empty(), "{v:?} 的拒绝原因不能为空"),
                UsbnetWrite::Allow => panic!("{v:?} 不该允许写 USB 模式（未取证）"),
            }
        }
    }
}
