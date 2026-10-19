//! 厂商 × 芯片平台的「取值表变体」。
//!
//! 同一个厂商、不同平台，**同一条指令的同一个数字含义可能相反**（典型：美格
//! `AT+SER=2` 在高通是 ECM、在展锐是 NCM；广和通 USB 模式 高通 32/30/18/24、
//! 展锐 34/40/38/36、联发科只有 41）。这张表把「本机该用哪一档」收敛成数据，
//! 供 AT 指令面（`atplan`）与排障显示使用。
//!
//! 数据来源：Go 上游 `module/variant/variant.go` 的 `PlatformVariants`
//! （证据链见各条 `evidence`），平台识别与 QModem `support_list.md` 对齐。
//!
//! ⚠️ 本模块只做**判定与展示**：真正写 USB/IMEI/锁网的实现仍在各厂商模块里
//! （`vendor/<厂商>.rs` 与 `model/*`），避免把行为分散到两张表。

use crate::modem::catalog;
use crate::modem::vendor::Vendor;

/// 厂商家族（谁有专属取值表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Generic,
    Quectel,
    Fibocom,
    Simcom,
    Meig,
    /// 华为 / 鼎桥（巴龙）
    Huawei,
    Gosuncn,
}

impl Family {
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Generic => "generic",
            Family::Quectel => "quectel",
            Family::Fibocom => "fibocom",
            Family::Simcom => "simcom",
            Family::Meig => "meig",
            Family::Huawei => "huawei",
            Family::Gosuncn => "gosuncn",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Family::Generic => "通用",
            Family::Quectel => "移远",
            Family::Fibocom => "广和通",
            Family::Simcom => "芯讯通",
            Family::Meig => "美格",
            Family::Huawei => "华为/鼎桥",
            Family::Gosuncn => "高新兴",
        }
    }
}

/// 「用哪张取值表」的平台档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// 认不出平台
    Unknown,
    Qualcomm,
    Unisoc,
    MediaTek,
    HiSilicon,
    /// QModem 里 platform=lte 的单 LTE 老模组
    LegacyLte,
    /// 本家族的兜底行
    Any,
}

impl Flavor {
    pub fn as_str(self) -> &'static str {
        match self {
            Flavor::Unknown => "",
            Flavor::Qualcomm => "qualcomm",
            Flavor::Unisoc => "unisoc",
            Flavor::MediaTek => "mediatek",
            Flavor::HiSilicon => "hisilicon",
            Flavor::LegacyLte => "lte",
            Flavor::Any => "*",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Flavor::Unknown => "未识别平台",
            Flavor::Qualcomm => "高通",
            Flavor::Unisoc => "展锐",
            Flavor::MediaTek => "联发科",
            Flavor::HiSilicon => "海思巴龙",
            Flavor::LegacyLte => "lte（单 LTE 老款）",
            Flavor::Any => "其它平台",
        }
    }
}

/// 一条变体：家族 × 平台档。
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub family: Family,
    pub flavor: Flavor,
    /// 界面显示名（如「广和通 · 展锐」）
    pub label: &'static str,
    /// 依据（排查「为什么写错值」时先看它）
    pub evidence: &'static str,
    /// USB 模式方言
    pub usb: &'static str,
    /// IMEI 方言
    pub imei: &'static str,
    /// 组网模式方言
    pub mode: &'static str,
    /// 锁网方言
    pub lock: &'static str,
}

const U: &str = "—";

const VARIANTS: &[Variant] = &[
    // ---------------- 广和通 ----------------
    Variant { family: Family::Fibocom, flavor: Flavor::MediaTek, label: "广和通 · 联发科",
        evidence: "vendor/fibocom.sh：MTK 只认 rndis=41；IMEI 走 AT+EGMREXT",
        usb: "fibocom-mtk", imei: "EGMREXT", mode: "fibocom", lock: "FibocomMTK" },
    Variant { family: Family::Fibocom, flavor: Flavor::Unisoc, label: "广和通 · 展锐",
        evidence: "vendor/fibocom.sh：ecm=34 / mbim=40 / rndis=38 / ncm=36；锁网走 AT+GTACT",
        usb: "fibocom-unisoc", imei: "GTSN", mode: "fibocom", lock: "FibocomMTK" },
    Variant { family: Family::Fibocom, flavor: Flavor::LegacyLte, label: "广和通 · lte（NL668/NL678）",
        evidence: "NL668/NL678：IMEI 走 AT+LCTSN；USB 沿用高通码表",
        usb: "fibocom-qualcomm", imei: "LCTSN", mode: "fibocom", lock: "VendorOrQNW" },
    Variant { family: Family::Fibocom, flavor: Flavor::Qualcomm, label: "广和通 · 高通",
        evidence: "vendor/fibocom.sh：qmi=32 / ecm=18 / mbim=30 / rndis=24；IMEI 走 AT+GTSN",
        usb: "fibocom-qualcomm", imei: "GTSN", mode: "fibocom", lock: "VendorOrQNW" },
    Variant { family: Family::Fibocom, flavor: Flavor::Any, label: "广和通 · 其它平台（兜底 = 高通语义）",
        evidence: "fibocom USB/IMEI 的 default 分支 = 高通码表 + AT+GTSN",
        usb: "fibocom-qualcomm", imei: "GTSN", mode: "fibocom", lock: "Default" },
    // ---------------- 华为 / 鼎桥 ----------------
    Variant { family: Family::Huawei, flavor: Flavor::HiSilicon, label: "华为 · 海思巴龙",
        evidence: "vendor/huawei.sh：ecm=0 / ncm=4；0|2=ecm、6=rndis、7=mbim；IMEI 用 at^phynum",
        usb: "huawei-hisilicon", imei: "PHYNUM", mode: "huawei", lock: "MT5700" },
    Variant { family: Family::Huawei, flavor: Flavor::Unisoc, label: "华为 · 展锐平台（MH5000-82）",
        evidence: "vendor/huawei.sh 的 unisoc 分支：set_mode 0=rndis / 1=ecm / 2=ncm",
        usb: "huawei-unisoc", imei: "PHYNUM", mode: "huawei", lock: "QNW" },
    Variant { family: Family::Huawei, flavor: Flavor::Any, label: "华为 · 其它平台（兜底 = 巴龙）",
        evidence: "huaweiUSB 的 else 分支 = 海思巴龙码表",
        usb: "huawei-hisilicon", imei: "PHYNUM", mode: "huawei", lock: "Default" },
    // ---------------- 美格 ----------------
    Variant { family: Family::Meig, flavor: Flavor::Qualcomm, label: "美格 · 高通",
        evidence: "vendor/meig.sh：AT+SER=<n>,1 的 qualcomm 映射 2=ECM、3=RNDIS",
        usb: "meig-qualcomm", imei: U, mode: "meig", lock: "VendorOrQNW" },
    Variant { family: Family::Meig, flavor: Flavor::Unisoc, label: "美格 · 展锐",
        evidence: "vendor/meig.sh：非高通平台 2=NCM、3=RNDIS（同数字不同含义）",
        usb: "meig-other", imei: U, mode: "meig", lock: "QNW" },
    Variant { family: Family::Meig, flavor: Flavor::Any, label: "美格 · 其它平台（兜底）",
        evidence: "meigUSB 的 else 分支 = 非高通码表",
        usb: "meig-other", imei: U, mode: "meig", lock: "Default" },
    // ---------------- 芯讯通 ----------------
    Variant { family: Family::Simcom, flavor: Flavor::Qualcomm, label: "芯讯通 · 高通（AT+CUSBCFG）",
        evidence: "vendor/simcom.sh：高通平台读 AT+CUSBCFG? 的 USBID",
        usb: "simcom-qualcomm", imei: U, mode: "simcom", lock: "VendorOrQNW" },
    Variant { family: Family::Simcom, flavor: Flavor::Any, label: "芯讯通 · 其它平台（AT$MYCONFIG）",
        evidence: "vendor/simcom.sh：lte 平台 AT$MYCONFIG=\"USBNETMODE\"（0=rndis/1=ecm/2=auto）",
        usb: "simcom-legacy", imei: U, mode: "simcom", lock: "Default" },
    // ---------------- 高新兴 ----------------
    Variant { family: Family::Gosuncn, flavor: Flavor::Qualcomm, label: "高新兴 · 高通",
        evidence: "vendor/gosuncn.sh：高通平台 AT+ZSWITCH 的取值表",
        usb: "gosuncn", imei: U, mode: "gosuncn-qualcomm", lock: "Default" },
    Variant { family: Family::Gosuncn, flavor: Flavor::Any, label: "高新兴 · 其它平台（lte 等）",
        evidence: "gosuncn 的 default 分支（ZBAND 锁频段）",
        usb: "gosuncn", imei: U, mode: "gosuncn-other", lock: "Default" },
    // ---------------- 移远（只有 lte 老款单独一档） ----------------
    Variant { family: Family::Quectel, flavor: Flavor::LegacyLte, label: "移远 · lte 旧款 4G（EM05/EC20/EC25/EG25…）",
        evidence: "vendor/quectel.sh：lte 分支锁频段走 AT+QCFG=\"band\",0,<hex>,0；模式走 QNWPREFCFG lte_band",
        usb: U, imei: "EGMR", mode: "quectel-lte", lock: "QCFG-BAND" },
    // ---------------- 通用 ----------------
    Variant { family: Family::Generic, flavor: Flavor::Qualcomm, label: "通用 · 高通",
        evidence: "厂商自带 QNW 方言",
        usb: U, imei: U, mode: U, lock: "VendorOrQNW" },
    Variant { family: Family::Generic, flavor: Flavor::Unisoc, label: "通用 · 展锐",
        evidence: "QNW 方言 + 展锐候选",
        usb: U, imei: U, mode: U, lock: "QNW" },
    Variant { family: Family::Generic, flavor: Flavor::MediaTek, label: "通用 · 联发科",
        evidence: "MTK 方言（FibocomMTK 探测链）",
        usb: U, imei: U, mode: U, lock: "FibocomMTK" },
    Variant { family: Family::Generic, flavor: Flavor::HiSilicon, label: "通用 · 海思巴龙",
        evidence: "MT5700 方言",
        usb: U, imei: U, mode: U, lock: "MT5700" },
    Variant { family: Family::Generic, flavor: Flavor::LegacyLte, label: "通用 · lte 旧款",
        evidence: "QCFG band 位图写法",
        usb: U, imei: U, mode: U, lock: "QCFG-BAND" },
    Variant { family: Family::Generic, flavor: Flavor::Any, label: "通用 · 未识别平台",
        evidence: "只发厂商自带锁网指令，不做平台特判",
        usb: U, imei: U, mode: U, lock: "VendorOnly" },
];

/// 型号串 → 芯片平台（先查支持清单，再按关键字兜底）。
///
/// 清单里每个型号都带 `platform`；不在清单里的按型号关键字判（FM350 → MTK 等）。
pub fn platform_for(vendor: Vendor, model: &str) -> String {
    if let Some(e) = catalog::lookup(model) {
        if !e.platform.is_empty() {
            return e.platform.clone();
        }
    }
    let _ = vendor;
    platform_by_token(model)
}

/// 关键字 → 平台（顺序敏感：更具体的写在前面）。
fn platform_by_token(model: &str) -> String {
    let u = model.to_uppercase();
    if u.trim().is_empty() {
        return String::new();
    }
    const TOKENS: &[(&str, &str)] = &[
        ("FM350R", "mediatek"),
        ("RW350R", "mediatek"),
        ("FM350", "mediatek"),
        ("MT5700", "hisilicon"),
        ("MT5710", "hisilicon"),
        ("MH5000-31", "hisilicon"),
        ("MH5000-82", "unisoc"),
        ("RG801", "hisilicon"),
        ("A8200", "asrmicro"),
        ("FM650", "unisoc"),
        ("L610", "unisoc"),
        ("RM500U", "unisoc"),
        ("RG200U", "unisoc"),
        ("SRM821", "unisoc"),
        ("N510M", "unisoc"),
        ("AIR724", "unisoc"),
        ("FM150", "qualcomm"),
        ("FM160", "qualcomm"),
        ("FM170", "qualcomm"),
        ("FM190W", "qualcomm"),
        ("NL668", "qualcomm"),
        ("NL678", "qualcomm"),
        ("RM500Q", "qualcomm"),
        ("RM500S", "qualcomm"),
        ("RM502Q", "qualcomm"),
        ("RM505Q", "qualcomm"),
        ("RM510Q", "qualcomm"),
        ("RM520N", "qualcomm"),
        ("RM520F", "qualcomm"),
        ("RM521F", "qualcomm"),
        ("RM530N", "qualcomm"),
        ("RM551E", "qualcomm"),
        ("RG500Q", "qualcomm"),
        ("RG501Q", "qualcomm"),
        ("RG520N", "qualcomm"),
        ("RG520F", "qualcomm"),
        ("RG530F", "qualcomm"),
        ("RG650V", "qualcomm"),
        ("AG598", "qualcomm"),
        ("MH8001", "qualcomm"),
        ("EM05", "qualcomm"),
        ("EM12", "qualcomm"),
        ("EM120", "qualcomm"),
        ("EM160", "qualcomm"),
        ("EP06", "qualcomm"),
        ("EC20", "qualcomm"),
        ("EC21", "qualcomm"),
        ("EC25", "qualcomm"),
        ("EG25", "qualcomm"),
        ("EC200", "qualcomm"),
        ("SRM815", "qualcomm"),
        ("SRM825", "qualcomm"),
        ("SIM8200", "qualcomm"),
        ("SIM8202", "qualcomm"),
        ("SIM8380", "qualcomm"),
        ("D8200", "qualcomm"),
        ("EM9190", "qualcomm"),
        ("EM9291", "qualcomm"),
        ("MC7354", "qualcomm"),
        ("MC7355", "qualcomm"),
        ("FN990", "qualcomm"),
        ("T77W", "qualcomm"),
        ("T99W", "qualcomm"),
        ("MV32", "qualcomm"),
        ("GM800", "qualcomm"),
    ];
    for (key, platform) in TOKENS {
        if u.contains(key) {
            return (*platform).to_string();
        }
    }
    String::new()
}

/// 厂商家族判定（与上游 `FamilyFor` 同序）。
pub fn family_for(vendor: Vendor, model: &str) -> Family {
    match vendor {
        Vendor::Quectel => return Family::Quectel,
        Vendor::Fibocom => return Family::Fibocom,
        Vendor::Simcom => return Family::Simcom,
        Vendor::Meig => return Family::Meig,
        Vendor::TDTech => return Family::Huawei,
        _ => {}
    }
    let u = model.to_uppercase();
    if u.contains("MH5000") || u.contains("MT5700") || u.contains("MT5710") || u.contains("S10") {
        return Family::Huawei;
    }
    if vendor == Vendor::Huawei {
        return Family::Huawei;
    }
    if u.contains("ME3630") || u.contains("GM800") || vendor == Vendor::Gosuncn {
        return Family::Gosuncn;
    }
    Family::Generic
}

/// 平台档位判定（lte 旧款优先于芯片平台）。
pub fn flavor_for(vendor: Vendor, model: &str) -> Flavor {
    let u = model.to_uppercase();
    // 移远旧款 4G：EM12 用词边界，EM120K 是 lte12 不算
    let quectel_legacy = ["EM05", "EP06", "EC20", "EC21", "EC25", "EG25", "EC200"]
        .iter()
        .any(|k| u.contains(k))
        || token_at_word(&u, "EM12");
    if quectel_legacy {
        return Flavor::LegacyLte;
    }
    if vendor == Vendor::Fibocom && (u.contains("NL668") || u.contains("NL678")) {
        return Flavor::LegacyLte;
    }
    match platform_for(vendor, model).as_str() {
        "qualcomm" | "lte" | "lte12" => Flavor::Qualcomm,
        "unisoc" => Flavor::Unisoc,
        "mediatek" => Flavor::MediaTek,
        "hisilicon" => Flavor::HiSilicon,
        _ => Flavor::Unknown,
    }
}

/// 关键字按词出现（前后不是字母/数字），用于 `S10` / `EM12` 这类短名。
fn token_at_word(u: &str, key: &str) -> bool {
    let mut from = 0usize;
    while let Some(pos) = u[from..].find(key) {
        let at = from + pos;
        let before_ok = at == 0 || !u.as_bytes()[at - 1].is_ascii_alphanumeric();
        let end = at + key.len();
        let after_ok = end >= u.len() || !u.as_bytes()[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = at + key.len();
    }
    false
}

/// 查本机该用哪一行变体（精确 → 家族兜底 → 通用同档 → 通用兜底）。
pub fn lookup(vendor: Vendor, model: &str) -> Option<&'static Variant> {
    let fam = family_for(vendor, model);
    let fl = flavor_for(vendor, model);
    if fl == Flavor::Unknown {
        return None;
    }
    let find = |f: Family, k: Flavor| VARIANTS.iter().find(|v| v.family == f && v.flavor == k);
    if let Some(v) = find(fam, fl) {
        return Some(v);
    }
    if fam != Family::Generic {
        if let Some(v) = find(fam, Flavor::Any) {
            return Some(v);
        }
        if let Some(v) = find(Family::Generic, fl) {
            return Some(v);
        }
    }
    find(Family::Generic, Flavor::Any)
}

/// 变体名；查不到时给「家族 · 平台档」。
pub fn name(vendor: Vendor, model: &str) -> String {
    match lookup(vendor, model) {
        Some(v) => v.label.to_string(),
        None => format!(
            "{} · {}",
            family_for(vendor, model).label(),
            flavor_for(vendor, model).label()
        ),
    }
}

/// 变体依据；查不到为空。
pub fn note(vendor: Vendor, model: &str) -> String {
    lookup(vendor, model)
        .map(|v| v.evidence.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_and_flavor_match_upstream() {
        assert_eq!(family_for(Vendor::Fibocom, "FM350-GL"), Family::Fibocom);
        assert_eq!(flavor_for(Vendor::Fibocom, "FM350-GL"), Flavor::MediaTek);
        assert_eq!(flavor_for(Vendor::Fibocom, "FM160-CN"), Flavor::Qualcomm);
        assert_eq!(family_for(Vendor::TDTech, "MT5700M-CN"), Family::Huawei);
        assert_eq!(flavor_for(Vendor::TDTech, "MT5700M-CN"), Flavor::HiSilicon);
        assert_eq!(family_for(Vendor::Quectel, "EC20"), Family::Quectel);
        assert_eq!(flavor_for(Vendor::Quectel, "EC20"), Flavor::LegacyLte);
        // EM120K 是 lte12，不能误判成 lte 旧款
        assert_eq!(flavor_for(Vendor::Quectel, "EM120K"), Flavor::Qualcomm);
    }

    #[test]
    fn variant_lookup_cascades_like_go() {
        // 广和通 · 联发科（精确命中）
        let v = lookup(Vendor::Fibocom, "FM350-GL").expect("fm350");
        assert_eq!(v.label, "广和通 · 联发科");
        assert_eq!(v.imei, "EGMREXT");
        // 移远 · 高通：本家族没有该档，落到「通用 · 高通」
        let v = lookup(Vendor::Quectel, "RM500Q-GL").expect("rm500q");
        assert_eq!(v.family, Family::Generic);
        assert_eq!(v.flavor, Flavor::Qualcomm);
        // 认不出平台 ⇒ 不给变体
        assert!(lookup(Vendor::Unknown, "XYZ-9999").is_none());
    }

    #[test]
    fn platform_tokens_cover_qmodem_list() {
        assert_eq!(platform_by_token("RM500Q-GL"), "qualcomm");
        assert_eq!(platform_by_token("FM350-GL"), "mediatek");
        assert_eq!(platform_by_token("MT5700M-CN"), "hisilicon");
        assert_eq!(platform_by_token("MH5000-82"), "unisoc");
        assert_eq!(platform_by_token(""), "");
    }
}
