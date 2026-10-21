//! 运营商识别与预制 APN 映射表（**纯数据 + 纯函数**，无 IO、不发 AT）。
//!
//! ## 为什么需要它
//!
//! 部分模组**必须显式下发 APN** 才能拨号：`AT+CGDCONT?` 里没有可用上下文、
//! 「沿用模组配置」拨上去就是不通，而页面明明能看到运营商（名称 / PLMN）——
//! 之前自动拨号只认「模组里已存的上下文」，没人把运营商对应的 APN 写进去。
//! 本模块给出可维护的映射表与识别逻辑，供 [`super::apn_auto`] 的决策使用。
//!
//! ## 识别优先级（与需求口径一致）
//!
//! 1. **运营商名称**（`AT+COPS?` 的 `<oper>`）：最直观；
//! 2. **MCC/MNC（PLMN）**（`AT+QNWINFO` / `COPS format=2`）：数字最可靠；
//!    名称与 PLMN 都认出但**指向不同运营商**时取 PLMN（显示名可能带漫游地）；
//! 3. **SIM 卡归属**（ICCID 前缀，即发卡行 IIN）：只在前两级都拿不到时兜底 ——
//!    物联网卡 / 企业卡常由代理发卡，IIN 只能作为**中等置信**的证据；
//! 4. 全部失败 ⇒ `CarrierMatch::unknown()`，调用方**不得**动现有 APN。
//!
//! ## 覆盖范围（2026-10-21 口径）
//!
//! 中国移动 / 中国联通 / 中国电信 / 中国广电四大运营商的常见名称变体、
//! `MCC 460` 下的在网 MNC、以及三家主要 IIN 前缀。广电的 MNC 是 `46015`，
//! IIN 前缀公开资料较少，只列 `898615` 一条（置信最低，宁可漏认不可错认）。
//! 想补充变体：改下面的表即可，不要在调用点写 if —— 表就是给人维护的。

/// 中国大陆四大运营商（本表只认这四家；其它一律「未知」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    /// 中国移动
    Cmcc,
    /// 中国联通
    Cucc,
    /// 中国电信
    Cect,
    /// 中国广电
    Cbnet,
}

impl Carrier {
    /// 运营商中文名（日志 / 页面步骤记录用）。
    pub fn label(self) -> &'static str {
        match self {
            Carrier::Cmcc => "中国移动",
            Carrier::Cucc => "中国联通",
            Carrier::Cect => "中国电信",
            Carrier::Cbnet => "中国广电",
        }
    }

    /// 该运营商的**默认互联网 APN**（需求指定的预制值）。
    ///
    /// ⚠️ 物联网卡 / 企业卡 / 专用套餐不适用（`cmiot` / 定制 APN 等）——
    /// 那类卡通常模组里已存上下文或由用户显式配置，决策层会优先沿用而
    /// 不会走到这条默认值（见 `apn_auto::decide` 的优先级表）。
    pub fn default_apn(self) -> &'static str {
        match self {
            Carrier::Cmcc => "cmnet",
            Carrier::Cucc => "3gnet",
            Carrier::Cect => "ctnet",
            Carrier::Cbnet => "cbnet",
        }
    }
}

/// 识别证据的强度（`None` = 四级证据都没有，不能据此动 APN）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Evidence {
    /// 没有任何证据
    None,
    /// 只有 ICCID 前缀（发卡行 IIN）：中等置信
    Iccid,
    /// PLMN（MCC/MNC）：高置信
    Plmn,
    /// 运营商名称：高置信（与 PLMN 冲突时以 PLMN 裁决，见 [`identify`]）
    Name,
}

/// 一次识别的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierMatch {
    /// 认出的运营商；`None` = 识别失败（调用方保留现状）。
    pub carrier: Option<Carrier>,
    /// 用的是哪级证据（决策层据此决定敢不敢写 APN）。
    pub evidence: Evidence,
    /// 一句话说明（写进拨号步骤与日志，排障时能看出「凭什么叫 cmnet」）。
    pub note: String,
}

impl CarrierMatch {
    pub fn unknown(note: impl Into<String>) -> Self {
        Self {
            carrier: None,
            evidence: Evidence::None,
            note: note.into(),
        }
    }

    /// 认出了运营商吗（`None` 与 [`Evidence::None`] 都算失败）。
    pub fn matched(&self) -> bool {
        self.carrier.is_some() && self.evidence != Evidence::None
    }

    /// 证据够不够强（名称 / PLMN；ICCID 只在「模组里本来就没有 APN」时使用）。
    pub fn strong(&self) -> bool {
        self.evidence >= Evidence::Plmn
    }
}

/// 运营商**名称**变体表（匹配方式：小写化后 `contains`，所以只收完整词，避免误命中）。
const NAME_TABLE: &[(&str, Carrier)] = &[
    // 移动
    ("cmcc", Carrier::Cmcc),
    ("china mobile", Carrier::Cmcc),
    ("chinamobile", Carrier::Cmcc),
    ("中国移动", Carrier::Cmcc),
    // 联通
    ("unicom", Carrier::Cucc),
    ("china unicom", Carrier::Cucc),
    ("中国联通", Carrier::Cucc),
    // 电信（`chn-ct` 是多款固件实测的显示名）
    ("china telecom", Carrier::Cect),
    ("chn-ct", Carrier::Cect),
    ("telecom", Carrier::Cect),
    ("中国电信", Carrier::Cect),
    // 广电（不能收 "cbn"：contains 会误命中预制 APN "cbnet"）
    ("中国广电", Carrier::Cbnet),
    ("china broadcast", Carrier::Cbnet),
    ("cbg", Carrier::Cbnet),
];

/// PLMN（`MCC 460` + 在网 MNC）→ 运营商。
const PLMN_TABLE: &[(&str, Carrier)] = &[
    // 移动：00/02/04/07/08（07 含 GSM/UMTS/LTE 复用，08 为 TD 补充）
    ("46000", Carrier::Cmcc),
    ("46002", Carrier::Cmcc),
    ("46004", Carrier::Cmcc),
    ("46007", Carrier::Cmcc),
    ("46008", Carrier::Cmcc),
    // 联通：01/06/09
    ("46001", Carrier::Cucc),
    ("46006", Carrier::Cucc),
    ("46009", Carrier::Cucc),
    // 电信：03/05/11（11 为电信 LTE/NR 在网段）
    ("46003", Carrier::Cect),
    ("46005", Carrier::Cect),
    ("46011", Carrier::Cect),
    // 广电：15
    ("46015", Carrier::Cbnet),
];

/// ICCID 前缀（发卡行 IIN 前 6 位）→ 运营商。**只作兜底证据**。
const ICCID_TABLE: &[(&str, Carrier)] = &[
    ("898600", Carrier::Cmcc),
    ("898602", Carrier::Cmcc),
    ("898604", Carrier::Cmcc),
    ("898607", Carrier::Cmcc),
    ("898608", Carrier::Cmcc),
    ("898601", Carrier::Cucc),
    ("898606", Carrier::Cucc),
    ("898609", Carrier::Cucc),
    ("898603", Carrier::Cect),
    ("898611", Carrier::Cect),
    // 广电公开 IIN 资料少，宁可漏认（识别不出就保留现状，不会写错 APN）
    ("898615", Carrier::Cbnet),
];

/// 预制 APN → 运营商（含各家历史 WAP 接入点：用于判定「现有 APN 是不是某家的
/// 预制值」，决策层据此区分「用户自定义 APN（绝不覆盖）」与「预制 APN（可校正）」）。
const PRESET_APN_TABLE: &[(&str, Carrier)] = &[
    ("cmnet", Carrier::Cmcc),
    ("cmwap", Carrier::Cmcc),
    ("3gnet", Carrier::Cucc),
    ("3gwap", Carrier::Cucc),
    ("wonet", Carrier::Cucc),
    ("unet", Carrier::Cucc),
    ("ctnet", Carrier::Cect),
    ("ctwap", Carrier::Cect),
    ("cbnet", Carrier::Cbnet),
];

/// 归一 PLMN：去空白与引号、去分隔符，只留数字（兼容 `460-11` / `"46011"` 两种写法）。
fn norm_plmn(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// 归一 APN：小写去空白（`CNET` / ` CTNET ` 都算 ctnet）。
fn norm_apn(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn by_name(s: Option<&str>) -> Option<Carrier> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    let low = s.to_ascii_lowercase();
    NAME_TABLE
        .iter()
        .find(|(k, _)| low.contains(k))
        .map(|(_, c)| *c)
}

fn by_plmn(s: Option<&str>) -> Option<Carrier> {
    let p = norm_plmn(s?);
    // 只要 MCC 460 的完整 PLMN（5~6 位）；空 / 非国内 PLMN 不认
    if !(p.starts_with("460") && p.len() >= 5) {
        return None;
    }
    PLMN_TABLE
        .iter()
        .find(|(k, _)| p.starts_with(k))
        .map(|(_, c)| *c)
}

fn by_iccid(s: Option<&str>) -> Option<Carrier> {
    let s = s?.trim();
    if s.len() < 6 {
        return None;
    }
    ICCID_TABLE
        .iter()
        .find(|(k, _)| s.starts_with(k))
        .map(|(_, c)| *c)
}

/// 这是某家运营商的**预制** APN 吗（cmnet / 3gnet / ctnet / cbnet 及各家 WAP 变体）。
///
/// 自定义 APN（企业卡 / 专用套餐 / `cmiot` 之类）返回 `None` ——
/// 决策层把它当「用户配置」对待，绝不覆盖。
pub fn apn_carrier(apn: &str) -> Option<Carrier> {
    let a = norm_apn(apn);
    PRESET_APN_TABLE
        .iter()
        .find(|(k, _)| *k == a)
        .map(|(_, c)| *c)
}

/// 综合识别：名称 → PLMN → ICCID，逐级兜底；冲突以 PLMN 裁决。
///
/// - `operator`：`AT+COPS?` 的 `<oper>`（format=0/1 的显示名）；
/// - `plmn`：`AT+QNWINFO` 或 COPS format=2 的数字串；
/// - `iccid`：SIM 身份缓存里的 ICCID（没有就传 `None`）。
pub fn identify(operator: Option<&str>, plmn: Option<&str>, iccid: Option<&str>) -> CarrierMatch {
    let by_name = by_name(operator);
    let by_plmn = by_plmn(plmn);
    match (by_name, by_plmn) {
        (Some(c), Some(p)) => {
            if c == p {
                CarrierMatch {
                    carrier: Some(c),
                    evidence: Evidence::Name,
                    note: format!("名称与 PLMN 双重确认：{}（{c:?}）", c.label()),
                }
            } else {
                // 显示名与 PLMN 指向不同运营商：信数字（名称可能带漫游地或固件写死）
                CarrierMatch {
                    carrier: Some(p),
                    evidence: Evidence::Plmn,
                    note: format!(
                        "名称「{}」与 PLMN 不一致，按 PLMN 认定 {}",
                        operator.unwrap_or(""),
                        p.label()
                    ),
                }
            }
        }
        (Some(c), None) => CarrierMatch {
            carrier: Some(c),
            evidence: Evidence::Name,
            note: format!("按运营商名称识别：{}", c.label()),
        },
        (None, Some(c)) => CarrierMatch {
            carrier: Some(c),
            evidence: Evidence::Plmn,
            note: format!("按 MCC/MNC（{}）识别：{}", plmn.unwrap_or(""), c.label()),
        },
        (None, None) => match by_iccid(iccid) {
            Some(c) => CarrierMatch {
                carrier: Some(c),
                evidence: Evidence::Iccid,
                note: format!(
                    "名称与 PLMN 都没认出来，按 ICCID 前缀识别：{}（中等置信）",
                    c.label()
                ),
            },
            None => CarrierMatch::unknown(
                "运营商名称 / MCC-MNC / ICCID 都没认出来，保留现有 APN 配置不动",
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_carriers_map_to_documented_apns() {
        // 需求指定：移动 cmnet / 联通 3gnet / 电信 ctnet / 广电 cbnet
        assert_eq!(Carrier::Cmcc.default_apn(), "cmnet");
        assert_eq!(Carrier::Cucc.default_apn(), "3gnet");
        assert_eq!(Carrier::Cect.default_apn(), "ctnet");
        assert_eq!(Carrier::Cbnet.default_apn(), "cbnet");
        assert_eq!(Carrier::Cmcc.label(), "中国移动");
    }

    #[test]
    fn identifies_by_name_variants() {
        // 名称变体：大小写 / 中英文 / 固件常见写法
        for (name, want) in [
            ("CMCC", Carrier::Cmcc),
            ("China Mobile", Carrier::Cmcc),
            ("中国移动", Carrier::Cmcc),
            ("中国联通", Carrier::Cucc),
            ("CHN-UNICOM", Carrier::Cucc),
            ("China Telecom", Carrier::Cect),
            ("CHN-CT", Carrier::Cect),
            ("中国广电", Carrier::Cbnet),
        ] {
            let m = identify(Some(name), None, None);
            assert_eq!(m.carrier, Some(want), "{name}");
            assert_eq!(m.evidence, Evidence::Name);
        }
        // 空串 / 未知名称不算证据
        let m = identify(Some("  "), None, None);
        assert_eq!(m.carrier, None);
        let m = identify(Some("Vodafone"), None, None);
        assert_eq!(m.carrier, None, "海外运营商不在表内");
    }

    #[test]
    fn identifies_by_plmn() {
        for (plmn, want) in [
            ("46000", Carrier::Cmcc),
            ("46007", Carrier::Cmcc),
            ("46001", Carrier::Cucc),
            ("46003", Carrier::Cect),
            ("46011", Carrier::Cect),
            ("46015", Carrier::Cbnet),
        ] {
            let m = identify(None, Some(plmn), None);
            assert_eq!(m.carrier, Some(want), "{plmn}");
            assert_eq!(m.evidence, Evidence::Plmn);
        }
        // 带分隔符 / 引号也认
        let m = identify(None, Some("460-11"), None);
        assert_eq!(m.carrier, Some(Carrier::Cect));
        // 非国内 PLMN 不认
        let m = identify(None, Some("310260"), None);
        assert_eq!(m.carrier, None);
        // 短 MCC 不认
        let m = identify(None, Some("46"), None);
        assert_eq!(m.carrier, None);
    }

    #[test]
    fn identifies_by_iccid_only_as_fallback() {
        let m = identify(None, None, Some("89860404102390000000"));
        assert_eq!(m.carrier, Some(Carrier::Cmcc));
        assert_eq!(m.evidence, Evidence::Iccid);
        // 名称在时轮不到 ICCID
        let m = identify(Some("CMCC"), None, Some("8986031111111111111"));
        assert_eq!(m.carrier, Some(Carrier::Cmcc));
        assert_eq!(m.evidence, Evidence::Name);
        // 全都没有 ⇒ 未知（调用方不得动 APN）
        let m = identify(None, None, None);
        assert!(!m.matched());
        assert_eq!(m.evidence, Evidence::None);
    }

    #[test]
    fn plmn_wins_when_it_contradicts_the_name() {
        // 显示名与 PLMN 冲突：信数字（名称可能带漫游地）
        let m = identify(Some("CMCC"), Some("46011"), None);
        assert_eq!(m.carrier, Some(Carrier::Cect));
        assert_eq!(m.evidence, Evidence::Plmn);
        assert!(m.note.contains("不一致"));
        // 一致时是双重确认，仍然 Name 级
        let m = identify(Some("CMCC"), Some("46000"), None);
        assert_eq!(m.carrier, Some(Carrier::Cmcc));
        assert_eq!(m.evidence, Evidence::Name);
    }

    #[test]
    fn preset_apn_lookup() {
        // 预制表：默认值 + 各家 WAP 变体都能反查
        for (apn, want) in [
            ("cmnet", Carrier::Cmcc),
            ("CMNET", Carrier::Cmcc),
            (" cmwap ", Carrier::Cmcc),
            ("3gnet", Carrier::Cucc),
            ("ctnet", Carrier::Cect),
            ("cbnet", Carrier::Cbnet),
        ] {
            assert_eq!(apn_carrier(apn), Some(want), "{apn}");
        }
        // 自定义 APN 一律 None（决策层把它当用户配置，绝不覆盖）
        assert_eq!(apn_carrier("cmiot5g"), None);
        assert_eq!(apn_carrier("my.company.apn"), None);
        assert_eq!(apn_carrier(""), None);
    }
}
