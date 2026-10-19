//! 支持模块清单（按型号）：USB / PCIe 形态各自支持哪些数据协议。
//!
//! ## 数据来源
//!
//! 逐行取自 QModem 的 `docs/support_list.md`（github.com/FUjr/QModem，MPL-2.0），
//! 并补上它的指令脚本里「切到某个模式」的 AT（`usb_at` / `pcie_at`）。
//! 本文件（JSON + 解析）按 MPL-2.0 使用；仓库其余代码仍是 MIT。
//!
//! ## 为什么要有这张表
//!
//! 厂商方言表只解决「同一条 AT 怎么写」，解决不了「这个型号到底支不支持这一档」：
//!
//! - 移远通用取值表里有 NCM（值 5），但 EP06 / EM05 / EC25 这一代没有 NCM；
//! - FM350 系（MTK）USB 只有 **RNDIS**，PCIe 才是 MBIM；
//! - Foxconn T99W640 **完全没有 USB 形态**（只有 PCIe MBIM）。
//!
//! 所以 `/api/modules`、USB 模式下拉、PCIe 视图都以这张**按型号**的表为准；
//! 不在表里的型号**不猜**，由调用方明确标注「不在支持清单里」。
//!
//! ## 口径
//!
//! 模式取值一律小写（`qmi/gobinet/ecm/mbim/rndis/ncm/rmnet/auto`），与 QModem 原文一致。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// 内嵌清单（41K，编译期进二进制；避免运行时依赖 `/usr/share` 文件）。
const CATALOG_JSON: &str = include_str!("modem_catalog.json");

/// 模式取值 → 显示名。
pub fn kind_label(kind: &str) -> String {
    match kind {
        "qmi" => "QMI".into(),
        "gobinet" => "GobiNet".into(),
        "ecm" => "ECM".into(),
        "mbim" => "MBIM".into(),
        "rndis" => "RNDIS".into(),
        "ncm" => "NCM".into(),
        "rmnet" => "RMNet".into(),
        "auto" => "自动".into(),
        "eap" => "EAP".into(),
        "ppp" => "PPP".into(),
        other => other.to_uppercase(),
    }
}

/// 厂商标识 → 中文名（清单/页面用）。
pub fn vendor_label(vendor: &str) -> String {
    match vendor {
        "openluat" => "合宙 openluat".into(),
        "huawei" => "华为 Huawei".into(),
        "quectel" => "移远 Quectel".into(),
        "fibocom" => "广和通 Fibocom".into(),
        "meig" => "美格 MeiG".into(),
        "sierra" => "Sierra Wireless".into(),
        "telit" => "Telit".into(),
        "simcom" => "芯讯通 SIMCom".into(),
        "foxconn" => "富士康 Foxconn".into(),
        "thales" => "Thales".into(),
        "neoway" => "有方 Neoway".into(),
        "gosuncn" => "高新兴 Gosuncn".into(),
        other => other.to_string(),
    }
}

/// 平台列 → 中文名。
pub fn platform_label(platform: &str) -> String {
    match platform {
        "qualcomm" => "高通 Qualcomm".into(),
        "unisoc" => "展锐 Unisoc".into(),
        "mediatek" => "联发科 MediaTek".into(),
        "hisilicon" => "海思 HiSilicon".into(),
        "lte" => "高通 LTE".into(),
        "lte12" => "高通 LTE12".into(),
        "asrmicro" => "翱捷 ASR".into(),
        "" => "未识别".into(),
        other => other.to_string(),
    }
}

/// 厂商在清单里的排列顺序（= QModem 支持列表的排列）。
pub const VENDOR_ORDER: [&str; 12] = [
    "openluat", "huawei", "quectel", "fibocom", "meig", "sierra", "telit", "simcom", "foxconn",
    "thales", "neoway", "gosuncn",
];

/// 一种形态（USB / PCIe）下的模式切换指令。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModeAtSet {
    /// 查询当前模式 / 切换时的查询指令。
    #[serde(default)]
    pub query: String,
    /// 模式 → 完整切换指令。
    #[serde(default)]
    pub modes: BTreeMap<String, String>,
    /// 没给全的原因（「只支持查询」「靠驱动」等），界面直接显示。
    #[serde(default)]
    pub note: String,
}

impl ModeAtSet {
    /// 切到某个模式的完整指令；没登记返回空。
    pub fn at_for_mode(&self, kind: &str) -> Option<&str> {
        self.modes.get(kind).map(String::as_str)
    }

    pub fn commands(&self) -> Vec<&str> {
        self.modes.values().map(String::as_str).collect()
    }
}

/// 清单里的一行。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Entry {
    pub vendor: String,
    pub model: String,
    #[serde(default)]
    pub platform: String,
    /// USB 形态支持的数据协议；空 = 该型号没有 USB 形态。
    #[serde(default)]
    pub usb: Vec<String>,
    /// PCIe 形态支持的数据协议；空 = 不支持 PCIe。
    #[serde(default)]
    pub pcie: Vec<String>,
    #[serde(default, rename = "usb_at")]
    pub usb_at: Option<ModeAtSet>,
    #[serde(default, rename = "pcie_at")]
    pub pcie_at: Option<ModeAtSet>,
}

impl Entry {
    pub fn vendor_label(&self) -> String {
        vendor_label(&self.vendor)
    }

    pub fn platform_label(&self) -> String {
        platform_label(&self.platform)
    }

    pub fn supports_usb(&self) -> bool {
        !self.usb.is_empty()
    }

    pub fn supports_pcie(&self) -> bool {
        !self.pcie.is_empty()
    }

    pub fn usb_text(&self) -> String {
        kinds_text(&self.usb)
    }

    pub fn pcie_text(&self) -> String {
        kinds_text(&self.pcie)
    }

    pub fn usb_command_for(&self, kind: &str) -> Option<&str> {
        self.usb_at.as_ref().and_then(|s| s.at_for_mode(kind))
    }

    pub fn pcie_command_for(&self, kind: &str) -> Option<&str> {
        self.pcie_at.as_ref().and_then(|s| s.at_for_mode(kind))
    }

    /// 这台型号的首选 USB 模式（清单顺序就是优先级）。
    pub fn preferred_usb(&self) -> Option<&str> {
        self.usb.first().map(String::as_str)
    }
}

/// `["qmi","ecm"]` → `"QMI / ECM"`。
pub fn kinds_text(kinds: &[String]) -> String {
    if kinds.is_empty() {
        return "—".into();
    }
    kinds
        .iter()
        .map(|k| kind_label(k))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn catalog() -> &'static Vec<Entry> {
    static CELL: OnceLock<Vec<Entry>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON).unwrap_or_else(|e| {
            // 内嵌数据解析失败属于构建缺陷：记一条错误，退化为空表（页面显示 0 个型号）
            log::error!("支持清单解析失败: {e}");
            Vec::new()
        })
    })
}

/// 全量清单。
pub fn entries() -> &'static [Entry] {
    catalog().as_slice()
}

/// `(型号数, 有 USB 的型号数, 有 PCIe 的型号数, 厂商数)`。
pub fn stats() -> (usize, usize, usize, usize) {
    let list = catalog();
    let usb = list.iter().filter(|e| e.supports_usb()).count();
    let pcie = list.iter().filter(|e| e.supports_pcie()).count();
    let vendors = list
        .iter()
        .map(|e| e.vendor.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    (list.len(), usb, pcie, vendors)
}

/// 每个厂商的型号数（按 [`VENDOR_ORDER`]，清单里没出现的厂商不列）。
pub fn vendor_counts() -> Vec<(&'static str, String, usize)> {
    VENDOR_ORDER
        .iter()
        .map(|v| {
            let n = catalog().iter().filter(|e| e.vendor == *v).count();
            (*v, vendor_label(v), n)
        })
        .filter(|(_, _, n)| *n > 0)
        .collect()
}

/// 型号关键字（去重、按长度降序，保证 `fm350-gl` 先于 `fm350` 命中）。
fn model_keys() -> &'static Vec<String> {
    static CELL: OnceLock<Vec<String>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut keys: Vec<String> = catalog().iter().map(|e| e.model.clone()).collect();
        keys.sort();
        keys.dedup();
        keys.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        keys
    })
}

/// 型号归一：只留小写字母与数字（`RM500Q-GL` → `rm500qgl`）。
pub fn flatten(model: &str) -> String {
    model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 按型号查清单项（最长关键字优先；`FM350-GL` 命中 `fm350-gl` 而不是 `fm350`）。
pub fn lookup(model: &str) -> Option<&'static Entry> {
    let flat = flatten(model);
    if flat.is_empty() {
        return None;
    }
    for key in model_keys() {
        let k = flatten(key);
        if k.len() < 3 {
            continue; // `s10` 这类短名容易误伤，宁可不认
        }
        if flat.contains(&k) {
            if let Some(e) = catalog().iter().find(|e| e.model == *key) {
                return Some(e);
            }
        }
    }
    None
}

/// 型号的 USB 模式；不在清单里返回 `None`（调用方**不要**猜）。
pub fn usb_modes(model: &str) -> Option<&'static [String]> {
    lookup(model).map(|e| e.usb.as_slice())
}

/// 型号的 PCIe 模式；不在清单里返回 `None`。
pub fn pcie_modes(model: &str) -> Option<&'static [String]> {
    lookup(model).map(|e| e.pcie.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_the_full_qmodem_list() {
        let (models, usb, pcie, vendors) = stats();
        assert_eq!(models, 91, "QModem support_list 共 91 个型号");
        assert_eq!(pcie, 41, "其中 41 个有 PCIe 形态");
        assert_eq!(vendors, 12);
        assert!(usb >= 89, "绝大多数型号有 USB 形态");
        // 厂商顺序与清单一致
        assert_eq!(vendor_counts().first().map(|v| v.0), Some("openluat"));
    }

    #[test]
    fn lookup_is_longest_first_and_normalised() {
        let e = lookup("RM500Q-GL").expect("rm500q-gl");
        assert_eq!(e.vendor, "quectel");
        assert_eq!(e.preferred_usb(), Some("qmi"));
        assert!(e.pcie.contains(&"mbim".to_string()));

        // 带厂商前缀的 ATI 字符串也能命中
        let e = lookup("Fibocom FM350-GL").expect("fm350-gl");
        assert_eq!(e.model, "fm350-gl");
        assert_eq!(e.usb, vec!["rndis".to_string()], "350 的 USB 是 RNDIS");
        assert_eq!(e.pcie, vec!["mbim".to_string()], "350 的 PCIe 是 MBIM");

        // 没有 USB 形态的型号
        let e = lookup("T99W640").expect("t99w640");
        assert!(!e.supports_usb());
        assert_eq!(e.pcie, vec!["mbim".to_string()]);
    }

    #[test]
    fn unknown_model_returns_none_never_guesses() {
        assert!(lookup("NOT-A-REAL-MODEM-9000").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn mode_at_comes_from_qmodem_scripts() {
        let e = lookup("RM500Q-GL").expect("rm500q-gl");
        assert_eq!(
            e.usb_command_for("qmi"),
            Some("AT+QCFG=\"usbnet\",0"),
            "切换指令必须原样来自 QModem 脚本"
        );
        assert_eq!(
            e.usb_at.as_ref().map(|s| s.query.as_str()),
            Some("AT+QCFG=\"usbnet\"")
        );
        // PCIe 形态只登记了「没有切换指令」的说明
        let note = &e.pcie_at.as_ref().expect("pcie_at").note;
        assert!(!note.is_empty(), "没有指令时必须写清原因");
    }

    #[test]
    fn text_helpers_are_human_readable() {
        assert_eq!(kinds_text(&["qmi".into(), "ecm".into()]), "QMI / ECM");
        assert_eq!(kinds_text(&[]), "—");
        assert_eq!(vendor_label("quectel"), "移远 Quectel");
        assert_eq!(platform_label("mediatek"), "联发科 MediaTek");
    }
}
