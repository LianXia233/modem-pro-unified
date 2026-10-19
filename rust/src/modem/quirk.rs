//! 逐型号「怪癖」表：同一条 AT 在不同型号上语义不同时的例外。
//!
//! ## 为什么厂商级不够
//!
//! `vendor/<厂商>.rs` 描述的是「这一家的方言」；但同一个厂商底下，**只有部分型号**
//! 支持某条指令，或者同一条指令的返回值含义不一样：
//!
//! - 移远 `AT+CBC`：RM520 / EM05 支持（三字段），别的一代没验过 —— 不能给整家开；
//! - 广和通 `AT+GTLADC`：只有 FM160 的温度走这条，`AT+QTEMP` 直接 ERROR；
//! - 鼎桥 MT5700 / 移远 RM500U 的 `AT+CCLK?` 时区后缀不可信（报的就是 UTC，
//!   再按 +32 去减会净差 8 小时）。
//!
//! 这类知识只能按型号登记，所以单独一张表：**加一个型号只改数据文件**，
//! 不用动厂商模块，也不用在解析代码里塞 `if model.contains(...)`。
//!
//! ## 数据来源
//!
//! 逐条取自上游 Go 版 `module/modelquirk/table.go`（MIT），每条都带实测依据
//! （固件版本 + 原始应答），原样保留在 `note` 里 —— 这张表的价值一半在值、
//! 一半在「为什么」，删掉 note 以后没人敢改。
//!
//! ## 边界
//!
//! 只登记**解析语义**上的差异。指令本身「有没有」属于能力探测（`vendor_cmds` /
//! 候选列表）的职责，不要混进来 —— 否则两处都能决定发什么，出问题没法定位。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const QUIRKS_JSON: &str = include_str!("modem_quirks.json");

/// 一条型号特例。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Quirk {
    /// 型号子串（大小写不敏感），如 `RM520` / `FM350` / `MT5700`。
    #[serde(rename = "match")]
    pub match_key: String,
    /// 供电电压指令（空 = 用默认 `AT+CBC`）。
    #[serde(default)]
    pub voltage_cmd: String,
    /// 模组温度指令（空 = 用厂商模块的候选）。
    #[serde(default)]
    pub temp_cmd: String,
    /// ICCID 指令（空 = 用默认候选 `AT+CCID` / `AT+QCCID` / `AT^ICCID?`）。
    #[serde(default)]
    pub iccid_cmd: String,
    /// `AT+CCLK?` 的时区后缀不可信（报的值本身就是 UTC）。
    #[serde(default)]
    pub ignore_cclk_tz: bool,
    /// 实测依据（固件版本 + 原始应答）。
    #[serde(default)]
    pub note: String,
}

fn table() -> &'static Vec<Quirk> {
    static CELL: OnceLock<Vec<Quirk>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut v: Vec<Quirk> = serde_json::from_str(QUIRKS_JSON).unwrap_or_else(|e| {
            log::error!("型号怪癖表解析失败: {e}");
            Vec::new()
        });
        // 长关键字优先：`T99W` 与 `T99W640` 同时命中时取更具体的
        v.sort_by(|a, b| b.match_key.len().cmp(&a.match_key.len()));
        v
    })
}

/// 全表（只读，测试/诊断用）。
pub fn all() -> &'static [Quirk] {
    table().as_slice()
}

/// 按「型号串」查特例。
///
/// `hint` 可以是 `ATI` 原文、`AT+CGMM` 结果或清单里的型号名 —— 表里登记的就是
/// 型号子串（`RM520`、`FM350`），所以直接做大小写不敏感的子串匹配。
/// 长关键字优先，避免 `SRM` 抢走 `SRM825` 更具体的条目（本表暂无后者，规则先立住）。
pub fn for_model(hint: &str) -> Option<&'static Quirk> {
    let up = hint.to_ascii_uppercase();
    if up.trim().is_empty() {
        return None;
    }
    table()
        .iter()
        .find(|q| !q.match_key.is_empty() && up.contains(&q.match_key.to_ascii_uppercase()))
}

/// 查电压指令的**候选顺序**：特例在前，默认 `AT+CBC` 兜底。
///
/// 兜底放在后面而不是省掉：特例表里的指令也可能因固件版本变化失效（上游证据只对应
/// 某一版固件），失败后仍值得试一次标准写法。
pub fn voltage_cmds(hint: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    if let Some(q) = for_model(hint) {
        if !q.voltage_cmd.is_empty() {
            out.push(q.voltage_cmd.as_str());
        }
    }
    if !out.contains(&"AT+CBC") {
        out.push("AT+CBC");
    }
    out
}

/// 查 ICCID 指令的候选顺序：特例 → 默认三条。
pub fn iccid_cmds(hint: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    if let Some(q) = for_model(hint) {
        if !q.iccid_cmd.is_empty() {
            out.push(q.iccid_cmd.as_str());
        }
    }
    for c in ["AT+CCID", "AT+QCCID", "AT^ICCID?"] {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// 型号特例里的温度指令（没有就 `None`，由厂商候选接管）。
pub fn temp_cmd(hint: &str) -> Option<&'static str> {
    for_model(hint).and_then(|q| {
        if q.temp_cmd.is_empty() {
            None
        } else {
            Some(q.temp_cmd.as_str())
        }
    })
}

/// `AT+CCLK?` 的时区后缀是否不可信。
pub fn ignore_cclk_tz(hint: &str) -> bool {
    for_model(hint).map(|q| q.ignore_cclk_tz).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_the_upstream_quirks() {
        assert_eq!(all().len(), 19, "上游 Go 版 table.go 共 19 条");
        // 每条都必须有实测依据
        assert!(
            all().iter().all(|q| !q.note.trim().is_empty()),
            "没有 note 的特例等于没有依据，不许进表"
        );
    }

    #[test]
    fn voltage_is_model_scoped_not_vendor_scoped() {
        // 只有登记过的型号才多给一条；同厂其它型号仍走 AT+CBC
        assert_eq!(voltage_cmds("RM520N-GL"), vec!["AT+CBC"]);
        assert_eq!(voltage_cmds("EM05"), vec!["AT+CBC"]);
        // Sierra / 富士康 走私有指令，标准写法兜底在后
        assert_eq!(voltage_cmds("EM9190"), vec!["AT!PCVOLT?", "AT+CBC"]);
        assert_eq!(voltage_cmds("T99W175"), vec!["AT!PCVOLT?", "AT+CBC"]);
        // 未登记型号：只给标准写法，不瞎加
        assert_eq!(voltage_cmds("NOT-A-MODEM"), vec!["AT+CBC"]);
    }

    #[test]
    fn iccid_uses_private_command_first() {
        assert_eq!(
            iccid_cmds("AIR724UG"),
            vec!["AT+ICCID", "AT+CCID", "AT+QCCID", "AT^ICCID?"]
        );
        assert_eq!(iccid_cmds("N510M")[0], "AT$MYCCID");
        assert_eq!(iccid_cmds("RM520N-GL")[0], "AT+CCID");
    }

    #[test]
    fn temperature_comes_from_quirk_when_registered() {
        assert_eq!(temp_cmd("FM160-EAU"), Some("AT+GTLADC"));
        assert_eq!(temp_cmd("MT5700M-CN"), None, "鼎桥温度走厂商模块，不在本表");
        assert_eq!(temp_cmd("RM520N-GL"), None);
    }

    #[test]
    fn cclk_timezone_is_ignored_for_known_bad_firmwares() {
        assert!(ignore_cclk_tz("RM500U-CNV"));
        assert!(ignore_cclk_tz("RM520N-GL"));
        assert!(ignore_cclk_tz("MT5700M-CN"));
        assert!(!ignore_cclk_tz("FM160-EAU"), "FM160 的 +32 是对的");
        assert!(!ignore_cclk_tz("RM502Q-GL"));
    }

    #[test]
    fn match_is_case_insensitive_and_longest_first() {
        assert!(for_model("at! rm520n-gl").is_some());
        // 空串不匹配任何条目（否则会给所有型号套上第一条特例）
        assert!(for_model("").is_none());
        assert!(for_model("   ").is_none());
    }
}
