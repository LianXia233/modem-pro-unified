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
        "baiwang" => "Baiwang（QDC507）".into(),
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

/// 厂商在清单里的排列顺序（= QModem 支持列表的排列 + 本项目扩展）。
///
/// `baiwang`（大疆 QDC507）不在 QModem 清单里，来自参考实现
/// `ainuoyan/QDC507-Modem-Support`（`bb57bb7`），排在最后。
pub const VENDOR_ORDER: [&str; 13] = [
    "openluat", "huawei", "quectel", "fibocom", "meig", "sierra", "telit", "simcom", "foxconn",
    "thales", "neoway", "gosuncn", "baiwang",
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
    /// USB 厂商 ID（4 位小写十六进制）。
    ///
    /// 来自 QModem `modem_support.json` 的 `manufacturer_id` —— 同一张表里
    /// 「型号 ↔ VID」是**上游实测登记过的对应关系**，识别时用它做交叉校验：
    /// AT 应答说是 A 家的型号，而 USB VID 属于 B 家，说明要么型号名撞了、
    /// 要么设备被改过，这时不能直接按 A 家的方言发私有指令。
    ///
    /// 空 = 上游也没登记（`simcom_sim82xx` 那几条、以及 PCIe-only 的 T99W640）。
    #[serde(default)]
    pub vid: String,
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

/// 按 USB VID 找厂商。
///
/// 同一个 VID 底下可能挂着多个厂商的型号（最典型的是 `2c7c` 移远：底下分
/// 高通 / 展锐 / MTK 三个平台），所以取「该 VID 出现次数最多的厂商」，
/// 并配合 [`vendor_is_exclusive`] 判断这条推断能不能当证据用。
pub fn vendor_by_vid(vid: &str) -> Option<&'static str> {
    let v = vid.trim().to_ascii_lowercase();
    if v.is_empty() {
        return None;
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in catalog().iter().filter(|e| e.vid == v) {
        *counts.entry(e.vendor.as_str()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(name, n)| (*n, std::cmp::Reverse(*name)))
        .map(|(name, _)| name)
}

/// 这个 VID 在清单里是否只对应一家厂商。
///
/// `true` 时「VID → 厂商」可以当**确定证据**（如 `1e0e` SIMCom、`3466` 鼎桥）；
/// `false` 时只能当提示（如 `2c7c` 同时有移远/广和通/美格的登记型号）。
pub fn vendor_is_exclusive(vid: &str) -> bool {
    let v = vid.trim().to_ascii_lowercase();
    if v.is_empty() {
        return false;
    }
    let mut seen: Option<&str> = None;
    for e in catalog().iter().filter(|e| e.vid == v) {
        match seen {
            None => seen = Some(e.vendor.as_str()),
            Some(x) if x == e.vendor => {}
            Some(_) => return false,
        }
    }
    seen.is_some()
}

/// 该 VID 下登记过的全部型号（给「候选」用，不当作结论）。
pub fn models_by_vid(vid: &str) -> Vec<&'static Entry> {
    let v = vid.trim().to_ascii_lowercase();
    if v.is_empty() {
        return Vec::new();
    }
    catalog().iter().filter(|e| e.vid == v).collect()
}

/// 型号主干：去掉尾部 1~3 位的区域/形态后缀（`-cn` / `-gl` / `_cn`）。
///
/// 为什么需要：`ATI` 常常只报型号主干（`MT5700M` / `MT5710`），而清单里登记的是
/// `mt5700m-cn` / `mt5710_cn` —— 只做「整名包含」会认不出这些型号，
/// 结果是「设备明明在清单里，页面却显示未识别」。
///
/// 只剥 1~3 位、且剩余部分至少 4 位：`em05-cn` → `em05`、`mt5710_cn` → `mt5710`，
/// 但 `s10` 这类短名不会被剥成更短的串去误伤别的型号。
fn stem(model: &str) -> &str {
    for sep in ['-', '_'] {
        if let Some(i) = model.rfind(sep) {
            let tail = &model[i + 1..];
            if i >= 4
                && !tail.is_empty()
                && tail.len() <= 3
                && tail.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return &model[..i];
            }
        }
    }
    model
}

/// 所有可能命中这个型号的清单项（整名优先，其次主干），按可信度排序。
///
/// 给识别引擎用：命中多个时说明「只认到型号族、区域版本不确定」，此时
/// 结论仍可用（同族方言一致），但候选要一并列出来。
pub fn candidates_for(model: &str) -> Vec<&'static Entry> {
    let flat = flatten(model);
    if flat.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<&'static Entry> = Vec::new();
    // ① 整名包含（长名优先，`fm350-gl` 先于 `fm350`）
    for key in model_keys() {
        let k = flatten(key);
        if k.len() < 3 {
            continue;
        }
        if flat.contains(&k) {
            if let Some(e) = catalog().iter().find(|e| e.model == *key) {
                if !out.iter().any(|x| x.model == e.model) {
                    out.push(e);
                }
            }
        }
    }
    if !out.is_empty() {
        return out;
    }
    // ② 主干包含：`MT5700M` → `mt5700m-cn`；主干少于 5 位不参与（太容易误伤）
    for key in model_keys() {
        let k = flatten(stem(key));
        if k.len() < 5 {
            continue;
        }
        if flat.contains(&k) {
            if let Some(e) = catalog().iter().find(|e| e.model == *key) {
                if !out.iter().any(|x| x.model == e.model) {
                    out.push(e);
                }
            }
        }
    }
    out
}

/// 按型号查清单项（最长关键字优先；`FM350-GL` 命中 `fm350-gl` 而不是 `fm350`）。
///
/// 整名不中时退回主干匹配（见 [`stem`]），因此 `MT5700M` 也能命中 `mt5700m-cn`。
pub fn lookup(model: &str) -> Option<&'static Entry> {
    candidates_for(model).into_iter().next()
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
    fn catalog_is_the_qmodem_list_plus_documented_extensions() {
        let (models, usb, pcie, vendors) = stats();
        // 91 个来自 QModem support_list；qdc507 是参考实现登记过的扩展
        assert_eq!(models, 92, "QModem 91 + QDC507 扩展 1");
        assert_eq!(pcie, 41, "其中 41 个有 PCIe 形态");
        assert_eq!(vendors, 13, "QModem 12 家 + 百旺（QDC507）");
        assert!(usb >= 89, "绝大多数型号有 USB 形态");
        // 厂商顺序与清单一致
        assert_eq!(vendor_counts().first().map(|v| v.0), Some("openluat"));
        // 扩展条目：不在 QModem 清单、有实机依据
        let e = lookup("QDC507GLEFM21").expect("qdc507 扩展条目");
        assert_eq!(e.vendor, "baiwang");
        assert_eq!(e.usb, vec!["ecm".to_string()], "QDC507 只有 ECM");
        assert!(e.pcie.is_empty(), "QDC507 没有 PCIe 形态");
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
    fn vid_is_migrated_and_drives_vendor_lookup() {
        // 87/92 条登记了 VID（其余 5 条上游本来就没有）
        let with_vid = entries().iter().filter(|e| !e.vid.is_empty()).count();
        assert_eq!(with_vid, 87, "VID 迁移自 QModem 的 manufacturer_id");

        assert_eq!(vendor_by_vid("2c7c"), Some("quectel"));
        assert_eq!(vendor_by_vid("0e8d"), Some("fibocom"));
        assert_eq!(vendor_by_vid("1e0e"), None, "清单里没有的 VID 不猜");

        // 独占 VID 可当证据，多厂商 VID 只能当提示。
        // 2c7c 曾经「名下全是移远」，QDC507（实机 2c7c:0125）进来之后共享该 VID
        // —— 这正是「VID 只能当提示、不能当证据」的活例。
        assert!(!vendor_is_exclusive("2c7c"), "2c7c 现在挂移远与百旺两家");
        assert!(!vendor_is_exclusive("1782"), "1782 挂广和通/美格/合宙三家");
        assert!(!vendor_is_exclusive(""), "空 VID 不是证据");

        // 型号自带的 VID 与端口规则用的是同一口径（小写 4 位）
        let e = lookup("FM350-GL").expect("fm350-gl");
        assert_eq!(e.vid, "0e8d");
        assert_eq!(e.vid.len(), 4);
    }

    #[test]
    fn text_helpers_are_human_readable() {
        assert_eq!(kinds_text(&["qmi".into(), "ecm".into()]), "QMI / ECM");
        assert_eq!(kinds_text(&[]), "—");
        assert_eq!(vendor_label("quectel"), "移远 Quectel");
        assert_eq!(platform_label("mediatek"), "联发科 MediaTek");
    }
}
