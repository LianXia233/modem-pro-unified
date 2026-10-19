//! 模组识别引擎：多维度取证 → 候选 → 结论（带证据与冲突）。
//!
//! ## 为什么要单独一层
//!
//! 早先的识别就是 `ATI` → 关键字 → `short_model_name()`，只有一条线索：
//!
//! - ATI 在有些固件上回 `ERROR`（展锐部分版本），识别直接落空；
//! - 型号名撞车时无从校验（`MT5700M` 同时是华为/鼎桥共线型号）；
//! - 认错了也看不出来 —— 页面只显示一个型号，不给依据。
//!
//! 现在把「设备在 USB 上是谁」和「设备自己说自己是谁」放在一起比对，
//! 按**可信度从高到低**取结论，并把每一条用到的证据、候选、冲突都记下来，
//! 前端可以直接展示「为什么认成这个型号」。
//!
//! ## 证据与优先级
//!
//! | 依据 | 来源 | 置信度 | 说明 |
//! | --- | --- | --- | --- |
//! | [`MatchedBy::PortRule`] | QModem `modem_port_rule.json` | 90 | `VID:PID` 精确登记，且与 AT 型号一致 |
//! | [`MatchedBy::CatalogModel`] | ATI/CGMM 文本命中清单 | 85 | 设备自报，最权威的单条证据 |
//! | [`MatchedBy::VendorKeyword`] | ATI 里的厂商关键字 | 55 | 只认到厂商，型号未知 |
//! | [`MatchedBy::UsbVid`] | 清单里的 `VID → 厂商` | 45 | 仅当该 VID 在清单里只对应一家厂商 |
//! | [`MatchedBy::NoMatch`] | — | 0 | 未知设备，只保留 3GPP 通用能力 |
//!
//! ## 冲突处理
//!
//! 两条证据指向不同厂商时**不猜**：以「设备自报」为准，把另一条记进
//! `conflicts` 并下调置信度。这样做的理由是：私有 AT 指令是按厂商方言发的，
//! 认错厂商的代价（给不认识的固件发 `AT^SYSCFGEX` 之类）比「型号显示得保守」
//! 大得多。
//!
//! ## 红线
//!
//! 识别只用**只读**指令（`ATI` / `AT+CGMM` / `AT+CGMI` / `AT+CGMR`），
//! 不写 NV、不切模式、不重启模组。本模块不直接碰串口，输入由调用方给。

use serde::Serialize;

use crate::modem::catalog;
use crate::modem::ports;
use crate::modem::vendor::Vendor;

/// 设备在 USB 上的身份（从 sysfs 取，取不到就给 `None`）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsbIdentity {
    /// 4 位小写十六进制。
    pub vid: String,
    /// 4 位小写十六进制。
    pub pid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub product: String,
    /// sysfs 的拓扑路径（`2-1`），多设备时用来区分。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bus_path: String,
    /// 这台设备暴露的接口号（`1.0` / `1.2` …），与端口规则同一口径。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<String>,
}

impl UsbIdentity {
    /// VID/PID 任一缺失都算「没有 USB 身份」—— PCIe 模组、纯串口设备都走这条。
    pub fn is_empty(&self) -> bool {
        self.vid.trim().is_empty() || self.pid.trim().is_empty()
    }

    /// `"2c7c:0801"`。
    pub fn id_text(&self) -> String {
        format!("{}:{}", self.vid.trim(), self.pid.trim())
    }
}

/// 结论是**靠哪条证据**得出的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    /// `VID:PID` 在 QModem 端口规则里登记过。
    PortRule,
    /// AT 应答里的型号命中支持清单。
    CatalogModel,
    /// 只认出厂商（ATI 关键字）。
    VendorKeyword,
    /// 只从 USB VID 推断出厂商。
    UsbVid,
    /// 没有任何可用证据。
    NoMatch,
}

impl MatchedBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            MatchedBy::PortRule => "port_rule",
            MatchedBy::CatalogModel => "catalog_model",
            MatchedBy::VendorKeyword => "vendor_keyword",
            MatchedBy::UsbVid => "usb_vid",
            MatchedBy::NoMatch => "no_match",
        }
    }

    /// 给人看的一句话（界面「识别依据」一列）。
    pub fn label(&self) -> &'static str {
        match self {
            MatchedBy::PortRule => "USB 端口规则 + AT 型号",
            MatchedBy::CatalogModel => "AT 型号命中支持清单",
            MatchedBy::VendorKeyword => "ATI 厂商关键字",
            MatchedBy::UsbVid => "USB VID 推断",
            MatchedBy::NoMatch => "未识别",
        }
    }
}

/// 一次识别的完整结果。
#[derive(Debug, Clone, Serialize)]
pub struct Identify {
    /// 厂商标识（与 `Vendor::as_str()` 同口径）。
    pub vendor: &'static str,
    /// 清单里的型号（小写原名）；没认出来是 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// AT 原始应答（排查用，已压成一行）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model_raw: String,
    pub matched_by: MatchedBy,
    /// 依据的可读说明。
    pub matched_by_text: &'static str,
    /// 0~100，越高越确定。
    pub confidence: u8,
    /// 同 VID 下登记过的其它型号（只作提示，不作结论）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
    /// 证据之间互相矛盾的地方。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    /// 实际用到的每一条依据，按使用顺序。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// 设备的 USB 身份（有就带出来，页面显示 `VID:PID`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb: Option<UsbIdentity>,
}

impl Identify {
    /// 未识别时的兜底（只保留通用 3GPP 能力）。
    pub fn unknown(raw: &str) -> Self {
        Self {
            vendor: Vendor::Unknown.as_str(),
            model: None,
            model_raw: raw.to_string(),
            matched_by: MatchedBy::NoMatch,
            matched_by_text: MatchedBy::NoMatch.label(),
            confidence: 0,
            candidates: Vec::new(),
            conflicts: Vec::new(),
            evidence: Vec::new(),
            usb: None,
        }
    }

    /// 型号是否已确认（页面据此决定要不要显示厂商私有功能）。
    pub fn model_confirmed(&self) -> bool {
        self.model.is_some() && self.confidence >= 70
    }
}

/// 厂商「族」：清单里 `huawei` 同时含华为与鼎桥（`MT5700M` 是共线型号），
/// 比对冲突时这两家不算矛盾；其余厂商各自独立。
fn same_family(a: Vendor, b: Vendor) -> bool {
    use Vendor::*;
    if a == b {
        return true;
    }
    matches!(
        (a, b),
        (Huawei, TDTech) | (TDTech, Huawei)
    )
}

/// 清单里的厂商标识 → `Vendor`，并处理共线型号的归属。
///
/// `MT5700M` / `MT5710` 在清单里挂在 `huawei` 名下，但它们的 ATI 与指令面
/// 都是鼎桥那一套（`AT^MONSC` / `AT^CHIPTEMP`），项目既有归属也是鼎桥 ——
/// 这里按型号纠一次，避免「清单说华为、方言用鼎桥」的错位。
fn vendor_of_entry(e: &catalog::Entry) -> Vendor {
    let m = catalog::flatten(&e.model);
    if e.vendor == "huawei" && (m.starts_with("mt57") || m.starts_with("mh5000")) {
        // MH5000 系列同样分华为/鼎桥两版：清单里归 huawei 的那些是鼎桥固件
        return Vendor::TDTech;
    }
    Vendor::from_key(&e.vendor).unwrap_or(Vendor::Unknown)
}

/// 执行识别。
///
/// - `usb`：设备在 USB 上的身份，取不到给 `None`（PCIe / 纯串口）；
/// - `at_lines`：`ATI` 应答按行切好；ATI 失败时调用方可传 `AT+CGMM` 的结果。
pub fn identify(usb: Option<&UsbIdentity>, at_lines: &[String]) -> Identify {
    let raw = at_lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");

    let mut ev: Vec<String> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();
    let mut candidates: Vec<String> = Vec::new();

    let at_vendor = Vendor::detect(at_lines);
    if at_vendor != Vendor::Unknown {
        ev.push(format!("ATI 关键字 → {}", at_vendor.as_str()));
    } else if !raw.is_empty() {
        ev.push("ATI 应答里没有已知厂商关键字".to_string());
    } else {
        ev.push("ATI 无应答".to_string());
    }

    // ① AT 文本里有没有清单型号
    let at_entry = catalog::lookup(&raw);
    if let Some(e) = at_entry {
        ev.push(format!(
            "AT 型号 {} 命中清单（{} / {}）",
            e.model, e.vendor, e.platform
        ));
    }

    // ② USB 端口规则
    let mut rule: Option<&'static ports::PortRule> = None;
    let mut rule_entry: Option<&'static catalog::Entry> = None;
    if let Some(u) = usb {
        if !u.is_empty() {
            ev.push(format!("USB 身份 {}", u.id_text()));
            if let Some(r) = ports::lookup(&u.vid, &u.pid) {
                rule = Some(r);
                ev.push(format!(
                    "端口规则 {} → {}（AT 接口 {}）",
                    u.id_text(),
                    if r.model.is_empty() { "未标型号" } else { &r.model },
                    if r.at_interfaces.is_empty() {
                        "不限".to_string()
                    } else {
                        r.at_interfaces.join(",")
                    }
                ));
                if !r.model.is_empty() {
                    rule_entry = catalog::lookup(&r.model);
                }
            } else {
                ev.push("端口规则里没有这个 VID:PID".to_string());
            }
        }
    }

    // ③ VID → 厂商（只在该 VID 唯一时当证据）
    let vid_vendor = usb
        .filter(|u| !u.is_empty())
        .and_then(|u| catalog::vendor_by_vid(&u.vid));
    let vid_exclusive = usb
        .filter(|u| !u.is_empty())
        .map(|u| catalog::vendor_is_exclusive(&u.vid))
        .unwrap_or(false);
    if let (Some(v), Some(u)) = (vid_vendor, usb.filter(|u| !u.is_empty())) {
        if vid_exclusive {
            ev.push(format!("VID {} 在清单里只对应 {}", u.vid, v));
        } else {
            ev.push(format!("VID {} 对应多家厂商（最近一次取 {}）", u.vid, v));
            candidates.extend(
                catalog::models_by_vid(&u.vid)
                    .into_iter()
                    .map(|e| e.model.clone())
                    .take(8),
            );
        }
    }

    // ④ 定结论：型号优先取「设备自报」，其次取「端口规则登记」
    let model_entry = at_entry.or(rule_entry);
    let model = model_entry.map(|e| e.model.clone());

    let mut vendor = if at_vendor != Vendor::Unknown {
        at_vendor
    } else if let Some(e) = model_entry {
        vendor_of_entry(e)
    } else if rule.is_some() {
        // 规则里有型号但清单没有（如 gm800 / srm825xeu）：按 VID 推厂商
        vid_vendor
            .and_then(Vendor::from_key)
            .unwrap_or(Vendor::Unknown)
    } else if vid_exclusive {
        vid_vendor
            .and_then(Vendor::from_key)
            .unwrap_or(Vendor::Unknown)
    } else {
        Vendor::Unknown
    };

    // ⑤ 冲突：设备自报 vs 清单归属 / USB VID
    if let Some(e) = model_entry {
        let by_catalog = vendor_of_entry(e);
        if at_vendor != Vendor::Unknown && !same_family(at_vendor, by_catalog) {
            conflicts.push(format!(
                "ATI 关键字判为 {}，但型号 {} 在清单里属 {}",
                at_vendor.as_str(),
                e.model,
                by_catalog.as_str()
            ));
        }
        if let (Some(v), Some(u)) = (vid_vendor, usb.filter(|u| !u.is_empty())) {
            if let Some(mv) = Vendor::from_key(v) {
                if !same_family(mv, by_catalog) {
                    conflicts.push(format!(
                        "USB VID {} 属 {}，而型号 {} 属 {}",
                        u.vid,
                        mv.as_str(),
                        e.model,
                        by_catalog.as_str()
                    ));
                    // VID 冲突时以「设备自报」为准，但厂商降到清单归属，
                    // 避免用错方言发私有指令
                    if vendor == mv {
                        vendor = by_catalog;
                    }
                }
            }
        }
    }
    if let (Some(a), Some(r)) = (at_entry, rule_entry) {
        if a.model != r.model {
            conflicts.push(format!(
                "端口规则登记 {}，AT 自报 {}（以 AT 为准）",
                r.model, a.model
            ));
        }
    }

    // ⑥ 置信度
    let (matched_by, mut confidence) = if rule_entry.is_some() && at_entry.is_some() {
        (MatchedBy::PortRule, 90u8)
    } else if at_entry.is_some() {
        (MatchedBy::CatalogModel, 85)
    } else if rule_entry.is_some() {
        // 只有端口规则：VID:PID 是精确登记的，型号可信；但没经设备自报核对，低 10 分
        (MatchedBy::PortRule, 80)
    } else if rule.is_some() {
        (MatchedBy::PortRule, 70)
    } else if at_vendor != Vendor::Unknown {
        (MatchedBy::VendorKeyword, 55)
    } else if vid_exclusive && vendor != Vendor::Unknown {
        (MatchedBy::UsbVid, 45)
    } else {
        (MatchedBy::NoMatch, 0)
    };
    // 有冲突就掉一档：结论仍然给出，但明确标「不确定」
    if !conflicts.is_empty() {
        confidence = confidence.saturating_sub(15);
    }
    // 型号不在清单里的，别把置信度写成「已确认」
    if model.is_none() && matched_by == MatchedBy::VendorKeyword {
        confidence = confidence.min(55);
    }

    Identify {
        vendor: vendor.as_str(),
        model,
        model_raw: raw,
        matched_by,
        matched_by_text: matched_by.label(),
        confidence,
        candidates,
        conflicts,
        evidence: ev,
        usb: usb.cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn usb(vid: &str, pid: &str) -> UsbIdentity {
        UsbIdentity {
            vid: vid.into(),
            pid: pid.into(),
            ..Default::default()
        }
    }

    #[test]
    fn at_model_wins_and_is_confirmed() {
        let id = identify(None, &lines(&["Quectel", "RM520N-GL", "Revision: RM520NGLAAR03A03M4G"]));
        assert_eq!(id.vendor, "quectel");
        assert_eq!(id.model.as_deref(), Some("rm520n-gl"));
        assert_eq!(id.matched_by, MatchedBy::CatalogModel);
        assert!(id.model_confirmed());
        assert!(id.conflicts.is_empty());
    }

    #[test]
    fn port_rule_plus_at_is_the_strongest_evidence() {
        let u = usb("2c7c", "0801");
        let id = identify(Some(&u), &lines(&["Quectel", "RM520N-GL"]));
        assert_eq!(id.matched_by, MatchedBy::PortRule);
        assert_eq!(id.confidence, 90);
        assert_eq!(id.model.as_deref(), Some("rm520n-gl"));
        assert!(id.evidence.iter().any(|e| e.contains("端口规则")));
    }

    #[test]
    fn port_rule_alone_identifies_model() {
        // 设备 ATI 回 ERROR（展锐部分固件），只剩 USB 身份
        let u = usb("2c7c", "0801");
        let id = identify(Some(&u), &lines(&["ERROR"]));
        assert_eq!(id.model.as_deref(), Some("rm520n-gl"));
        assert_eq!(id.matched_by, MatchedBy::PortRule);
        assert_eq!(id.confidence, 80);
    }

    #[test]
    fn vid_conflict_keeps_device_self_report_but_downgrades() {
        // VID 是移远(2c7c)，但设备说自己是被清单归到别家的型号：
        // 用 ec25 举不出跨厂商的例子，这里直接构造「型号归 quectel、VID 归 simcom」
        let u = usb("1e0e", "9011");
        let id = identify(Some(&u), &lines(&["Quectel", "RM520N-GL"]));
        // 结论仍按设备自报（quectel），但必须记冲突并掉置信度
        assert_eq!(id.vendor, "quectel");
        assert!(!id.conflicts.is_empty(), "跨厂商必须记冲突");
        assert!(id.confidence < 85);
    }

    #[test]
    fn huawei_family_is_not_a_conflict() {
        // MT5700M 在清单里挂 huawei，ATI 判为 tdtech：同族，不算冲突
        let id = identify(None, &lines(&["TDTECH", "MT5700M"]));
        assert!(id.conflicts.is_empty(), "华为/鼎桥共线型号不应报冲突");
        assert_eq!(id.vendor, "tdtech");
        assert_eq!(id.model.as_deref(), Some("mt5700m-cn"));
    }

    #[test]
    fn unknown_device_has_no_guesswork() {
        let id = identify(None, &lines(&["ATI", "Some Unknown Module"]));
        assert_eq!(id.vendor, "unknown");
        assert_eq!(id.model, None);
        assert_eq!(id.confidence, 0);
        assert_eq!(id.matched_by, MatchedBy::NoMatch);
        assert!(!id.model_confirmed());
    }

    #[test]
    fn non_exclusive_vid_only_gives_candidates() {
        // 1782 底下挂着广和通 / 美格 / 合宙三家（清单里实测如此），
        // 只有 VID 时不能直接定厂商，只能给候选
        let u = usb("1782", "9999");
        let id = identify(Some(&u), &[]);
        assert_eq!(id.vendor, "unknown", "多厂商 VID 不足以定结论");
        assert!(!id.candidates.is_empty(), "至少给出候选型号");
        assert_eq!(id.confidence, 0);
        assert_eq!(id.matched_by, MatchedBy::NoMatch);
    }

    #[test]
    fn exclusive_vid_is_enough_for_vendor() {
        // 2c7c 在清单里只对应移远（44 个型号），且这个 PID 没有端口规则
        let u = usb("2c7c", "9999");
        let id = identify(Some(&u), &[]);
        assert_eq!(id.matched_by, MatchedBy::UsbVid);
        assert_eq!(id.vendor, "quectel");
        assert_eq!(id.confidence, 45);
        assert_eq!(id.model, None, "只到厂商，不猜型号");
    }

    #[test]
    fn raw_is_kept_for_diagnosis() {
        let id = identify(None, &lines(&["Fibocom", "FM350-GL"]));
        assert!(id.model_raw.contains("FM350-GL"));
        assert_eq!(id.model.as_deref(), Some("fm350-gl"));
        assert_eq!(id.vendor, "fibocom");
    }
}
