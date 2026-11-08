//! 拨号门禁：把「这一轮为什么不拨 / 为什么停」变成可枚举的**原因代码**。
//!
//! ## 为什么需要单独一个模块
//!
//! 「不拨」的理由此前只以中文短句的形式塞进 `DialState.last_error`，前端能显示、
//! 但没法按原因做判断（比如「漫游被拦」和「用户自己关的」在页面上长得一样）。
//! 上游 `module/kernel/hostdial*.go` 的 `hostDialStopReason` 用**有序判据**返回
//! 唯一原因：漫游 → 模组未运行 → 飞行模式 → 手动断开 → 配额。本模块按同一顺序
//! 重写，并落成稳定代码（`stop_reason`），文本仍写给人看。
//!
//! ## 为什么要有等级（而不是一律硬拦）
//!
//! 漫游 / 配额 / 飞行模式在本仓库都已各有处置（漫游只给界面提示、配额由后台
//! 循环关射频）。直接照抄上游的硬拦截会**改变现有用户的拨号时机** —— 一台
//! 在境外漫游但套餐支持的模组会突然拨不出去。所以这里做成显式等级：
//!
//! | 等级 | 额外判据 | 默认 |
//! | --- | --- | --- |
//! | [`GateLevel::Off`] | 无 | 是（与改造前行为一致） |
//! | [`GateLevel::Roam`] | 漫游 | |
//! | [`GateLevel::All`] | 漫游 + 飞行模式 + 配额 | |
//!
//! 判据命中时调用方写 `stop_reason` 并记日志；未命中时行为与旧版一字不差。

/// 门禁等级（配置 `[net] dial_gate`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GateLevel {
    /// 只保留原有的两条判据（`host_dial` / 拨号总开关）与手动断开。
    #[default]
    Off,
    /// 额外按漫游策略拦截。
    Roam,
    /// 再加班飞行模式与流量配额。
    All,
}

impl GateLevel {
    /// 解析配置值。**未知值一律归 `Off`**：配置写错不该让链路突然拨不出去。
    pub fn parse(raw: &str) -> GateLevel {
        match raw.trim().to_ascii_lowercase().as_str() {
            "roam" | "roaming" => GateLevel::Roam,
            "all" | "full" | "strict" => GateLevel::All,
            _ => GateLevel::Off,
        }
    }

    /// 规范字符串（`normalize` 落盘用）。
    pub fn as_str(self) -> &'static str {
        match self {
            GateLevel::Off => "off",
            GateLevel::Roam => "roam",
            GateLevel::All => "all",
        }
    }

    /// 漫游判据是否参与。
    pub fn checks_roam(self) -> bool {
        matches!(self, GateLevel::Roam | GateLevel::All)
    }

    /// 飞行模式 / 配额判据是否参与。
    pub fn checks_extras(self) -> bool {
        matches!(self, GateLevel::All)
    }
}

/// 一条停止原因：稳定代码 + 给人看的文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    /// 稳定代码（前端/ubus 按它做判断，不要按文本匹配）。
    pub code: &'static str,
    /// 中文说明，与既有 `last_error` 口径一致。
    pub text: String,
}

impl Stop {
    fn new(code: &'static str, text: impl Into<String>) -> Stop {
        Stop {
            code,
            text: text.into(),
        }
    }
}

/// 判定所需事实。`None` = 「这一档没查」（门禁等级没要求，或查询失败）。
///
/// 查询失败用 `None` 而不是 `Some(false)`：拿不到飞行模式/漫游状态时**不能**
/// 当作「没开」静默放行，也不能当作「开了」误拦 —— 只有确定命中才拦。
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub host_dial: bool,
    pub dial_enabled: bool,
    pub manual_stop: bool,
    pub airplane_on: Option<bool>,
    pub quota_blocked: Option<bool>,
    pub roam_blocked: Option<bool>,
}

/// 有序判定（顺序对齐上游 `hostDialStopReason`：漫游 → 飞行模式 → 手动断开 → 配额）。
///
/// 返回 `None` = 可以拨/可以继续跑。
pub fn evaluate(level: GateLevel, f: &Facts) -> Option<Stop> {
    if !f.host_dial {
        return Some(Stop::new("host_dial_off", "host_dial 关闭"));
    }
    if !f.dial_enabled {
        return Some(Stop::new("dial_disabled", "拨号总开关关闭"));
    }
    if level.checks_roam() && f.roam_blocked == Some(true) {
        return Some(Stop::new("roaming", "国际漫游未授权"));
    }
    if level.checks_extras() && f.airplane_on == Some(true) {
        return Some(Stop::new("airplane", "飞行模式已开启"));
    }
    if f.manual_stop {
        return Some(Stop::new("manual_stop", "用户手动断开"));
    }
    if level.checks_extras() && f.quota_blocked == Some(true) {
        return Some(Stop::new("quota", "流量配额已用尽"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Facts {
        Facts {
            host_dial: true,
            dial_enabled: true,
            manual_stop: false,
            airplane_on: Some(false),
            quota_blocked: Some(false),
            roam_blocked: Some(false),
        }
    }

    #[test]
    fn parse_unknown_falls_back_to_off() {
        // 配置写错一律放行：门禁不能因为拼写错误而变严。
        assert_eq!(GateLevel::parse(""), GateLevel::Off);
        assert_eq!(GateLevel::parse("of"), GateLevel::Off);
        assert_eq!(GateLevel::parse("strict"), GateLevel::All);
        assert_eq!(GateLevel::parse(" Roam "), GateLevel::Roam);
        assert_eq!(GateLevel::Off.as_str(), "off");
    }

    #[test]
    fn off_level_keeps_legacy_behaviour() {
        // 等级 off 时，漫游/飞行/配额一律不参与 —— 与改造前一致。
        let mut f = ok();
        f.roam_blocked = Some(true);
        f.airplane_on = Some(true);
        f.quota_blocked = Some(true);
        assert_eq!(evaluate(GateLevel::Off, &f), None);
    }

    #[test]
    fn roam_level_only_checks_roaming() {
        let mut f = ok();
        f.airplane_on = Some(true);
        f.quota_blocked = Some(true);
        assert_eq!(evaluate(GateLevel::Roam, &f), None);
        f.roam_blocked = Some(true);
        assert_eq!(evaluate(GateLevel::Roam, &f).unwrap().code, "roaming");
    }

    #[test]
    fn all_level_checks_extras_and_keeps_order() {
        let mut f = ok();
        f.airplane_on = Some(true);
        f.quota_blocked = Some(true);
        f.roam_blocked = Some(true);
        // 上游顺序：漫游优先于飞行模式。
        assert_eq!(evaluate(GateLevel::All, &f).unwrap().code, "roaming");
        f.roam_blocked = Some(false);
        assert_eq!(evaluate(GateLevel::All, &f).unwrap().code, "airplane");
        f.airplane_on = Some(false);
        f.manual_stop = true;
        // 手动断开优先于配额（上游同顺序）。
        assert_eq!(evaluate(GateLevel::All, &f).unwrap().code, "manual_stop");
        f.manual_stop = false;
        assert_eq!(evaluate(GateLevel::All, &f).unwrap().code, "quota");
    }

    #[test]
    fn unknown_facts_never_block() {
        // 查不到状态（None）不等于命中。
        let mut f = ok();
        f.airplane_on = None;
        f.quota_blocked = None;
        f.roam_blocked = None;
        assert_eq!(evaluate(GateLevel::All, &f), None);
    }

    #[test]
    fn legacy_switches_always_apply() {
        let mut f = ok();
        f.host_dial = false;
        assert_eq!(evaluate(GateLevel::Off, &f).unwrap().code, "host_dial_off");
        let mut f = ok();
        f.dial_enabled = false;
        assert_eq!(evaluate(GateLevel::Off, &f).unwrap().code, "dial_disabled");
    }
}
