//! 型号 → 支持频段（QModem 频段表）。
//!
//! ## 数据来源
//!
//! 逐字取自 QModem 的 `application/qmodem/files/usr/share/qmodem/modem_support.json`
//! （github.com/FUjr/QModem，MPL-2.0），经其 `docs/support_list.md` 同源的整理口径
//! 收敛为 [`modem_bands.json`](modem_bands.json)。本文件（JSON + 解析）按 MPL-2.0 使用；
//! 仓库其余代码仍是 MIT。
//!
//! 每个型号四组频段：
//!
//! | 字段 | 含义 |
//! | --- | --- |
//! | `wcdma` | WCDMA/3G 频段号（B1/B5/B8…） |
//! | `lte` | LTE 频段号（B1/B3/B41…） |
//! | `nr_nsa` | NR **NSA**（EN-DC）频段号（n41/n78/n79…） |
//! | `nr_sa` | NR **SA** 频段号 |
//!
//! ## 为什么值得单独记一份（而不是每次去问模组）
//!
//! 1. **锁频段页的可选项**：没有它，用户得自己记得 n78 还是 n41；有了它就能把
//!    「本型号支持的频段」直接列成可勾选的芯片（见 [`groups`]）。
//! 2. **与模组自报能力对照**：`BandLock::capable` 是**运行期读**的（固件不报就只能显示 `-`），
//!    这份是**出厂口径**；两者不一致（固件被裁频段、或上游写错）正是排查时要看的信息。
//! 3. **上游缺数据也要如实记**：92 个型号里有 3 个 QModem 没写频段 —— 查得到记录但频段为空，
//!    界面显示「上游没写」，**不能当成「不支持任何频段」**。
//!
//! ## 匹配规则
//!
//! 与支持清单（[`catalog::lookup`](super::catalog::lookup)）同一套：去分隔符归一 +
//! **最长关键字优先**；≤4 字符的短名（`S10`）要求词边界，≥5 字符允许 ATI 连写匹配
//! （`RM520NGLAAR03A03M4G`）。别名（如 `QogirN6Pro_PS` → `f50-mu300`）与主名同权。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// 内嵌频段表（27K，编译期进二进制；不依赖 `/usr/share` 下的文件）。
const BANDS_JSON: &str = include_str!("modem_bands.json");

/// 一个型号的支持频段（四组，均升序去重；空 = 上游没写 / 该制式不支持）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModemBands {
    pub model: String,
    /// 上游 manufacturer（`quectel` / `fibocom` / `huawei`…）。
    pub vendor: String,
    /// 上游 platform（`qualcomm` / `unisoc` / `mediatek` / `hisilicon` / `lte`…）。
    pub platform: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wcdma: Vec<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lte: Vec<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nr_nsa: Vec<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nr_sa: Vec<i32>,
    /// 同一型号的**其它固件上报名**（厂家/固件写法不同）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// 技术域（决定取哪一组、以及界面上的 3GPP 前缀）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tech {
    Wcdma,
    Lte,
    /// NR 合并口径（NSA ∪ SA）—— 设备侧只有一套 NR 频段列表时用。
    Nr,
    /// NR NSA 单列（EN-DC）—— 只有分两套列表的方言才有。
    NrNsa,
}

impl Tech {
    /// 锁网状态里的技术域名（`band_lte` / `band_nr` / `band_nr_nsa` 的 `tech` 字段）。
    pub fn as_str(self) -> &'static str {
        match self {
            Tech::Wcdma => "WCDMA",
            Tech::Lte => "LTE",
            Tech::Nr => "NR",
            Tech::NrNsa => "NRNSA",
        }
    }

    /// 3GPP 频段标签前缀（LTE/WCDMA 用 `B`，NR 用 `n`）。
    pub fn prefix(self) -> &'static str {
        match self {
            Tech::Nr | Tech::NrNsa => "n",
            _ => "B",
        }
    }

    /// 界面/说明里的短名。
    pub fn label(self) -> &'static str {
        match self {
            Tech::Wcdma => "WCDMA",
            Tech::Lte => "LTE",
            Tech::Nr => "NR",
            Tech::NrNsa => "NR-NSA",
        }
    }

    /// 说明文案里的口径（NR 要写清是 NSA/SA 并集）。
    fn note_label(self) -> &'static str {
        match self {
            Tech::Nr => "NR（NSA/SA 并集）",
            other => other.label(),
        }
    }
}

impl ModemBands {
    /// 上游给这个型号写了频段吗（false = 记录存在但没有频段数据）。
    pub fn has_bands(&self) -> bool {
        !(self.wcdma.is_empty()
            && self.lte.is_empty()
            && self.nr_nsa.is_empty()
            && self.nr_sa.is_empty())
    }

    /// 是否支持 NR（NSA 或 SA 任一有值）。
    pub fn has_nr(&self) -> bool {
        !self.nr_nsa.is_empty() || !self.nr_sa.is_empty()
    }

    /// 该制式下**可锁**的频段号（升序去重）。
    ///
    /// - `Lte` → `lte`
    /// - `Nr` → `nr_nsa ∪ nr_sa`（同一台模组这两栏常常一样，但不假设 —— 并集才是「支持」）
    /// - `NrNsa` → **只取 `nr_nsa`**（能分开设的方言才走到这里，不并 SA）
    /// - `Wcdma` → `wcdma`
    pub fn supported(&self, tech: Tech) -> Vec<i32> {
        match tech {
            Tech::Lte => self.lte.clone(),
            Tech::Nr => merge(&[&self.nr_nsa, &self.nr_sa]),
            Tech::NrNsa => self.nr_nsa.clone(),
            Tech::Wcdma => self.wcdma.clone(),
        }
    }

    /// 该制式下界面显示的频段标签（`B1 B3 B41` / `n41 n78`）。
    pub fn keys(&self, tech: Tech) -> Vec<String> {
        self.supported(tech)
            .into_iter()
            .map(|n| band_key(tech, n))
            .collect()
    }

    /// 给界面/接口用的一句话（含来源与四组数量；上游没写频段时明确说「上游没写」）。
    pub fn note(&self) -> String {
        if !self.has_bands() {
            return "Modem-Pro 频段表（modem_bands.json）里这个型号没写频段（上游缺数据，不代表不支持）\
                    —— 以设备自报的「设备能力」为准"
                .to_string();
        }
        format!(
            "Modem-Pro 频段表（modem_bands.json）：LTE {} 个 · NR-SA {} 个 · NR-NSA {} 个 · WCDMA {} 个",
            self.lte.len(),
            self.nr_sa.len(),
            self.nr_nsa.len(),
            self.wcdma.len()
        )
    }

    /// 四组频段的紧凑展示（`LTE B1 B3 … · NR n41 n78`），空的不显示。
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        for (label, bands, prefix) in [
            ("WCDMA", &self.wcdma, "B"),
            ("LTE", &self.lte, "B"),
            ("NR-NSA", &self.nr_nsa, "n"),
            ("NR-SA", &self.nr_sa, "n"),
        ] {
            if !bands.is_empty() {
                let list: Vec<String> = bands.iter().map(|n| format!("{prefix}{n}")).collect();
                parts.push(format!("{label} {}", list.join(" ")));
            }
        }
        parts.join(" · ")
    }
}

/// 频段号 → 3GPP 标签（LTE `B3`、NR `n78`、WCDMA `B1`）。
pub fn band_key(tech: Tech, band: i32) -> String {
    format!("{}{band}", tech.prefix())
}

/// 频段表全部条目（按 QModem 原表顺序）。
pub fn entries() -> &'static [ModemBands] {
    static CELL: OnceLock<Vec<ModemBands>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut v: Vec<ModemBands> = serde_json::from_str(BANDS_JSON)
            .expect("modem_bands.json 解析失败（字段名打错会被 deny_unknown_fields 拦下）");
        assert!(!v.is_empty(), "modem_bands.json 里没有任何型号");
        for b in &mut v {
            b.wcdma = merge(&[&b.wcdma]);
            b.lte = merge(&[&b.lte]);
            b.nr_nsa = merge(&[&b.nr_nsa]);
            b.nr_sa = merge(&[&b.nr_sa]);
        }
        v
    })
}

/// 统计：型号总数 / 有频段数据的型号数 / 支持 NR 的型号数。
pub fn stats() -> (usize, usize, usize) {
    let mut with_bands = 0;
    let mut with_nr = 0;
    for b in entries() {
        if b.has_bands() {
            with_bands += 1;
        }
        if b.has_nr() {
            with_nr += 1;
        }
    }
    (entries().len(), with_bands, with_nr)
}

/// 按型号串取支持频段（ATI 原文 / 短名 / QModem 写法 / 别名都认）。
///
/// - `None` → 型号不在频段表里（既不知道支持什么，也不知道不支持什么）
/// - `Some(b)` 且 `!b.has_bands()` → 型号在表里，但**上游没写频段**
pub fn lookup(model: &str) -> Option<&'static ModemBands> {
    let u = model.trim().to_uppercase();
    if u.is_empty() {
        return None;
    }
    let ki = key_index();
    let flat = flatten(&u);
    let hit = ki
        .order
        .iter()
        .copied()
        .find(|&i| token_hit(&u, &ki.keys[i]))
        .or_else(|| {
            // ② 去分隔符后再试（ATI 常见连写），≥5 字符才允许，避免短名误伤
            ki.order.iter().copied().find(|&i| {
                let tok = flatten(&ki.keys[i]);
                tok.chars().count() >= 5 && flat.contains(&tok)
            })
        })?;
    entries().get(ki.owner[hit])
}

/// 型号 → 「清单口径」说明（型号没登记时也给出明确说法，不返回空串）。
pub fn note_for(model: &str) -> String {
    match lookup(model) {
        Some(b) => b.note(),
        None => "Modem-Pro 频段表（modem_bands.json）里没登记这个型号 —— \
                 以设备自报的「设备能力」为准"
            .to_string(),
    }
}

/// 清单声明的频段 + 说明（贴到锁网状态上用）。
pub struct Declared {
    pub bands: Vec<i32>,
    pub note: String,
}

/// 取某型号在某制式下的「出厂口径」频段。
///
/// 三种「没有频段」要有三种说法，别都显示成 `-`：
///
/// | 情况 | 说法 |
/// | --- | --- |
/// | 型号不在频段表里 | 上游没登记这个型号 |
/// | 在表里但没写频段 | 上游没写（不代表不支持） |
/// | 在表里有频段、但该制式为空 | 该制式没有记录（如纯 LTE 老模组没有 NR） |
pub fn declared(model: &str, tech: Tech) -> Declared {
    let Some(b) = lookup(model) else {
        return Declared {
            bands: Vec::new(),
            note: "Modem-Pro 频段表里没登记这个型号 —— 以设备自报的「设备能力」为准".into(),
        };
    };
    if !b.has_bands() {
        return Declared {
            bands: Vec::new(),
            note: b.note(),
        };
    }
    let bands = b.supported(tech);
    let note = if bands.is_empty() {
        format!(
            "Modem-Pro 频段表里这个型号没有 {} 频段记录",
            tech.as_str()
        )
    } else {
        // 中文排版：`NR（NSA/SA 并集）` 后面不再多一个空格，`LTE 频段` 仍保留空格
        let gap = if tech.note_label().ends_with('）') {
            ""
        } else {
            " "
        };
        format!(
            "Modem-Pro 频段表（出厂口径）：支持 {} 个 {}{gap}频段，可与上方「设备能力」对照",
            bands.len(),
            tech.note_label()
        )
    };
    Declared { bands, note }
}

// ====================== 锁频段页面的「按模块支持自动排版」 ======================

/// 锁频段页面的一组可选频段。
///
/// 界面**不用下拉选制式**：本型号支持哪几组（WCDMA / LTE / NR-NSA / NR-SA），
/// 就排几组，每组下面把频段列成勾选项（见 [`groups`]）。
#[derive(Debug, Clone, Serialize)]
pub struct BandGroup {
    /// 稳定标识（`wcdma` / `lte` / `nr` / `nr_nsa` / `nr_sa`）。
    pub key: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// 下发锁定时用哪个技术域（`LTE` / `NR` / `NRNSA`）；空 = 本组不可锁。
    pub tech: String,
    pub lockable: bool,
    /// 本组展示的频段（清单 ∪ 设备上报，升序去重）。
    pub bands: Vec<i32>,
    /// 其中来自清单的部分（界面上可标「出厂口径」）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub declared: Vec<i32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// 排版所需的运行期状态（全部是只读切片，避免与锁网模块互相依赖）。
#[derive(Debug, Clone, Copy, Default)]
pub struct BandState<'a> {
    /// 清单口径（`lookup` 的结果）。
    pub declared: Option<&'a ModemBands>,
    /// 本机方言的 NR 是否分 NSA / SA 两套列表（`Dialect::nsa_independent()`）。
    pub separate_nsa: bool,
    pub lte_capable: &'a [i32],
    pub lte_current: &'a [i32],
    pub nr_capable: &'a [i32],
    pub nr_current: &'a [i32],
    pub nsa_capable: &'a [i32],
    pub nsa_current: &'a [i32],
}

/// 把本机的支持频段整理成「可勾选的组」，供锁频段页直接渲染。
///
/// 规则（与上游一致）：
///
/// 1. 只出**有频段**的组 —— 纯 LTE 老模组不会出现空的 NR 组；
/// 2. 频段列表 = 清单 ∪ 设备能力 ∪ **设备当前锁定的频段** ——
///    清单是「能勾什么」的全集（少了它，取消勾选下发后那个频段就从列表消失），
///    后两样是「固件实际认什么」；
/// 3. NR **只出一组**：设备侧只有一条锁频段指令、一套 NR 列表（清单的 NSA / SA 两栏
///    只是上游口径，下发时合并）—— 拆两组会互相联动，看着像 bug；
///    除非本机方言确实分两套（`separate_nsa`），那才各出一组、各自下发；
/// 4. WCDMA 只展示、不可锁 —— 各厂商脚本里都没有 3G 锁频指令，不编。
pub fn groups(st: &BandState<'_>) -> Vec<BandGroup> {
    let (mut wcdma, mut lte, mut nsa, mut sa) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    if let Some(d) = st.declared {
        wcdma = d.wcdma.clone();
        lte = d.lte.clone();
        nsa = d.nr_nsa.clone();
        sa = d.nr_sa.clone();
    }

    let mut out = Vec::new();
    if !wcdma.is_empty() {
        out.push(BandGroup {
            key: "wcdma".into(),
            title: "WCDMA（3G）".into(),
            label: "WCDMA".into(),
            tech: String::new(),
            lockable: false,
            bands: wcdma.clone(),
            declared: wcdma.clone(),
            note: "只展示：各厂商脚本里都没有 3G 锁频指令，本项目不编".into(),
        });
    }

    let all_lte = merge(&[&lte, st.lte_capable, st.lte_current]);
    if !all_lte.is_empty() {
        let extra = subtract(&all_lte, &lte);
        out.push(BandGroup {
            key: "lte".into(),
            title: "LTE（4G）".into(),
            label: "LTE".into(),
            tech: Tech::Lte.as_str().into(),
            lockable: true,
            bands: all_lte,
            declared: merge(&[&lte]),
            note: if extra.is_empty() {
                String::new()
            } else {
                "含设备上报的频段（当前锁定 / 固件自报能力），Modem-Pro 清单里没有这几档".into()
            },
        });
    }

    if st.separate_nsa {
        // 本机方言的 NR 是**两套**列表（移远/高通 QNWPREFCFG、芯讯通 AT+CSYSSEL）：
        // NSA 与 SA 各一组、各自独立勾选与下发。
        let nsa_txt = label_list(&nsa, "n");
        let sa_txt = label_list(&sa, "n");
        let differ = nsa_txt != sa_txt;

        let all_nsa = merge(&[&nsa, st.nsa_capable, st.nsa_current]);
        if !all_nsa.is_empty() {
            let mut note = "本机 NR 分 NSA / SA 两套列表，本组只管 NSA".to_string();
            if differ && !nsa_txt.is_empty() {
                note.push_str(&format!("（出厂口径 {nsa_txt}）"));
            }
            out.push(BandGroup {
                key: "nr_nsa".into(),
                title: "NR · NSA（EN-DC）".into(),
                label: "NR-NSA".into(),
                tech: Tech::NrNsa.as_str().into(),
                lockable: true,
                bands: all_nsa,
                declared: merge(&[&nsa]),
                note,
            });
        }
        let all_sa = merge(&[&sa, st.nr_capable, st.nr_current]);
        if !all_sa.is_empty() {
            out.push(BandGroup {
                key: "nr_sa".into(),
                title: "NR · SA".into(),
                label: "NR-SA".into(),
                tech: Tech::Nr.as_str().into(),
                lockable: true,
                bands: all_sa,
                declared: merge(&[&sa]),
                note: if differ && !sa_txt.is_empty() {
                    format!("出厂口径 {sa_txt}")
                } else {
                    String::new()
                },
            });
        }
        return out;
    }

    let nr_declared = merge(&[&nsa, &sa]);
    let all_nr = merge(&[&nr_declared, st.nr_capable, st.nr_current]);
    if !all_nr.is_empty() {
        let nsa_txt = label_list(&nsa, "n");
        let sa_txt = label_list(&sa, "n");
        let mut note = "NSA / SA 在设备侧是同一套频段（只有一条锁频段指令）".to_string();
        if !nsa_txt.is_empty() && !sa_txt.is_empty() && nsa_txt != sa_txt {
            note = format!("NSA {nsa_txt} / SA {sa_txt} ⇒ 设备侧共用一套，这里取并集");
        }
        if !subtract(&all_nr, &nr_declared).is_empty() {
            note.push_str("；含设备上报的频段（当前锁定 / 固件自报能力）");
        }
        out.push(BandGroup {
            key: "nr".into(),
            title: "NR（5G）".into(),
            label: "NR".into(),
            tech: Tech::Nr.as_str().into(),
            lockable: true,
            bands: all_nr,
            declared: nr_declared,
            note,
        });
    }
    out
}

// ====================== 内部工具 ======================

/// 型号 → 匹配键的下标表（型号与别名同权）。
struct KeyIndex {
    keys: Vec<String>,
    /// 每个 key 指向的条目下标（别名与主名指向同一条）。
    owner: Vec<usize>,
    /// 按 key 长度降序（最长优先）的下标顺序。
    order: Vec<usize>,
}

fn key_index() -> &'static KeyIndex {
    static CELL: OnceLock<KeyIndex> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut keys = Vec::new();
        let mut owner = Vec::new();
        for (i, b) in entries().iter().enumerate() {
            keys.push(b.model.to_uppercase());
            owner.push(i);
            for a in &b.aliases {
                keys.push(a.to_uppercase());
                owner.push(i);
            }
        }
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by(|a, b| keys[*b].len().cmp(&keys[*a].len()).then_with(|| a.cmp(b)));
        KeyIndex { keys, owner, order }
    })
}

/// token 是否出现在 `u`（已大写）里；≤4 字符要求词边界。
fn token_hit(u: &str, tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    if tok.chars().count() <= 4 {
        word_hit(u, tok)
    } else {
        u.contains(tok)
    }
}

/// 关键字是否**按词**出现：前后不能是字母或数字（避免 `S10` 命中 `S100`）。
fn word_hit(u: &str, tok: &str) -> bool {
    u.match_indices(tok).any(|(at, _)| {
        let before = u[..at]
            .chars()
            .next_back()
            .map_or(true, |c| !c.is_ascii_alphanumeric());
        let after = u[at + tok.len()..]
            .chars()
            .next()
            .map_or(true, |c| !c.is_ascii_alphanumeric());
        before && after
    })
}

/// 型号归一：只留大写字母与数字（`RM500Q-GL` → `RM500QGL`）。
fn flatten(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 合并多组频段：丢弃 ≤0，升序去重。
pub fn merge(lists: &[&[i32]]) -> Vec<i32> {
    let mut v: Vec<i32> = lists.iter().flat_map(|l| l.iter().copied()).collect();
    v.retain(|n| *n > 0);
    v.sort_unstable();
    v.dedup();
    v
}

/// `a` 里去掉 `b` 里有的（升序去重）。
fn subtract(a: &[i32], b: &[i32]) -> Vec<i32> {
    if a.is_empty() {
        return Vec::new();
    }
    let drop: std::collections::HashSet<i32> = b.iter().copied().collect();
    merge(&[a]).into_iter().filter(|n| !drop.contains(n)).collect()
}

/// 频段号列表 → `B1 B3 B41` / `n41 n78`。
fn label_list(bands: &[i32], prefix: &str) -> String {
    merge(&[bands])
        .into_iter()
        .map(|n| format!("{prefix}{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_the_full_qmodem_band_list() {
        let (models, with_bands, with_nr) = stats();
        assert_eq!(models, 92, "QModem 频段表共 92 条记录");
        assert_eq!(with_bands, 89, "其中 3 个（T99W175/373/640）上游没写频段");
        assert_eq!(with_nr, 68, "68 个支持 NR（NSA 或 SA）");
        // 频段号已归一：升序、去重、无 0
        for b in entries() {
            for list in [&b.wcdma, &b.lte, &b.nr_nsa, &b.nr_sa] {
                assert!(list.windows(2).all(|w| w[0] < w[1]), "{} 未升序去重", b.model);
                assert!(list.iter().all(|n| *n > 0));
            }
        }
    }

    #[test]
    fn fm350_and_mt5700_match_expected_bands() {
        // FM350-GL：USB RNDIS，LTE 19 档、NR NSA/SA 同一套 10 档
        let b = lookup("FM350-GL").expect("FM350-GL 在频段表里");
        assert_eq!(b.vendor, "fibocom");
        assert_eq!(b.platform, "mediatek");
        assert_eq!(b.lte.len(), 19);
        assert!(b.lte.contains(&41));
        assert_eq!(b.supported(Tech::Nr), vec![1, 3, 7, 8, 20, 28, 38, 40, 77, 78]);

        // 鼎桥 MT5700M-CN：NR SA 比 NSA 多 n28
        let b = lookup("MT5700M-CN").expect("MT5700M-CN 在频段表里");
        assert_eq!(b.nr_nsa, vec![41, 78, 79]);
        assert_eq!(b.nr_sa, vec![1, 28, 41, 78, 79]);
        assert_eq!(b.supported(Tech::NrNsa), vec![41, 78, 79]);
        assert_eq!(b.supported(Tech::Nr), vec![1, 28, 41, 78, 79]);
        assert_eq!(b.keys(Tech::Lte)[0], "B1");
    }

    #[test]
    fn model_match_is_longest_first_and_word_bounded() {
        // ATI 连写（去分隔符）也能命中
        assert_eq!(
            lookup("Revision: RM520NGLAAR03A03M4G").map(|b| b.model.as_str()),
            Some("rm520n-gl")
        );
        // 别名与主名同权
        assert_eq!(
            lookup("QogirN6Pro_PS").map(|b| b.model.as_str()),
            Some("f50-mu300")
        );
        // 短名要词边界：S10 不能命中 S100 / MS10
        assert_eq!(lookup("S10").map(|b| b.model.as_str()), Some("s10"));
        assert!(lookup("S100").is_none(), "S100 不该被 S10 命中");
        assert!(lookup("MS10").is_none(), "MS10 不该被 S10 命中");
        // 不在表里 → None（不猜）
        assert!(lookup("NOT-A-REAL-MODEM-XYZZY").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn declared_has_three_distinct_answers() {
        // ① 型号在表里、该制式有频段
        let d = declared("RM520N-GL", Tech::Lte);
        assert_eq!(d.bands.len(), 31);
        assert!(d.note.contains("出厂口径"));
        // ② 在表里但上游没写频段
        let d = declared("T99W640", Tech::Lte);
        assert!(d.bands.is_empty());
        assert!(d.note.contains("没写频段"), "{}", d.note);
        // ③ 不在表里
        let d = declared("XYZZY-9", Tech::Lte);
        assert!(d.bands.is_empty());
        assert!(d.note.contains("没登记"), "{}", d.note);
        // ④ 在表里、但该制式没有记录（纯 LTE 老模组没有 NR）
        let d = declared("S10", Tech::Nr);
        assert!(d.bands.is_empty());
        assert!(d.note.contains("没有 NR 频段记录"), "{}", d.note);
    }

    #[test]
    fn groups_follow_declared_and_runtime_state() {
        let b = lookup("RM520N-GL").expect("rm520n-gl");
        // 一套 NR 列表：只出 lte + nr 两组（外加 wcdma 只读）
        let st = BandState {
            declared: Some(b),
            separate_nsa: false,
            lte_capable: &[1, 3, 41],
            lte_current: &[3],
            nr_capable: &[78],
            nr_current: &[78],
            ..Default::default()
        };
        let g = groups(&st);
        let keys: Vec<&str> = g.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, vec!["wcdma", "lte", "nr"]);
        // 当前锁定值 / 设备能力都要并进可选列表（否则取消勾选后就勾不回来了）
        let lte = g.iter().find(|x| x.key == "lte").unwrap();
        assert!(lte.bands.contains(&3) && lte.bands.contains(&41));
        assert!(lte.lockable);
        // WCDMA 只读
        let w = g.iter().find(|x| x.key == "wcdma").unwrap();
        assert!(!w.lockable && w.tech.is_empty());

        // 分两套列表的方言：NSA / SA 各一组，tech 不同
        let st = BandState {
            declared: Some(b),
            separate_nsa: true,
            ..Default::default()
        };
        let g = groups(&st);
        let nsa = g.iter().find(|x| x.key == "nr_nsa").unwrap();
        let sa = g.iter().find(|x| x.key == "nr_sa").unwrap();
        assert_eq!(nsa.tech, "NRNSA");
        assert_eq!(sa.tech, "NR");
        assert_eq!(nsa.bands, b.nr_nsa);
        assert_eq!(sa.bands, b.nr_sa);
    }

    #[test]
    fn groups_work_without_declared_data() {
        // 清单没登记，但设备自报有能力/当前值 ⇒ 仍要能渲染（不能白页）
        let st = BandState {
            lte_current: &[3, 8],
            ..Default::default()
        };
        let g = groups(&st);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].key, "lte");
        assert_eq!(g[0].bands, vec![3, 8]);
        assert!(g[0].declared.is_empty());
        // 什么都没有 → 没有任何组（页面会解释原因）
        assert!(groups(&BandState::default()).is_empty());
    }

    #[test]
    fn text_and_note_are_human_readable() {
        let b = lookup("FM350-GL").unwrap();
        let t = b.text();
        assert!(t.starts_with("WCDMA B1"), "{t}");
        assert!(t.contains("B41"), "{t}");
        assert!(t.contains("NR-SA n1 n3 n7 n8 n20 n28 n38 n40 n77 n78"), "{t}");
        assert!(b.note().contains("LTE 19 个"));
    }
}
