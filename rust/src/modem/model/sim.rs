//! SIM 热插拔：模组带电时拔卡 / 插卡 / 换卡的发现与处置。
//!
//! 两条路互补，缺一不可：
//!   1. **兜底轮询** `AT+CPIN?` + ICCID/IMSI：兜住「悄悄换了卡、固件却不发 `+CPIN:` URC」的机型；
//!   2. **URC 置脏**：`+GTSIM`（广和通 MTK 的 SIM 状态上报）被过滤层吞下后，由
//!      `http::loops::spawn_urc` 调 `mark_sim_dirty()`，巡检下一拍（2 秒内）就去
//!      核实，不用干等一个轮询周期。注意置脏入口**不是** `+CPIN:` —— 它的主动上报与
//!      `AT+CPIN?` 应答完全同形，过滤层刻意不吞（吞了会让 SIM 查询永远空应答）。
//!
//! ⚠️ 只有 **ICCID/IMSI 确实变了** 才算换卡：拔卡/插卡/读卡抖动都可能是假动作，
//! 拿它们去复位模组会变成无限复位循环（真机事故：模组每 2~3 分钟掉一次线）。
//! ⚠️ 读不到确定答复（`AT+CPIN?` 无应答）时**不判任何事件**：模组复位 / USB 重新枚举期间
//! CPIN 一定读不到，把它当「拔卡」同样会引出无限复位。

use std::time::{Duration, Instant};

use serde::Serialize;

/// 巡检节拍：只决定「多久看一眼 dirty / 到没到点」，不决定发 AT 的频率。
pub const SIM_WATCH_TICK: Duration = Duration::from_secs(2);

/// 两次「换卡动作」之间至少隔这么久（挡住反复插拔与读卡抖动）。
pub const SIM_COOLDOWN: Duration = Duration::from_secs(30);

/// 事件类型：卡被拔了。
pub const SIM_REMOVED: &str = "removed";
/// 事件类型：原本没卡，插上了。
pub const SIM_INSERTED: &str = "inserted";
/// 事件类型：换了另一张卡。
pub const SIM_SWAPPED: &str = "swapped";

/// SIM 卡身份：用来回答「还是不是同一张卡」。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SimIdentity {
    /// 卡在不在位（`AT+CPIN?` 拿得到状态文本就算在位，可能只是锁了 PIN）。
    pub present: bool,
    /// `AT+CPIN?` 的状态文本：`READY` / `SIM PIN` / `SIM PUK` …
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub iccid: String,
    /// 只在读不到 ICCID 的固件上当身份（换卡时它同样会变）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub imsi: String,
}

/// `+CPIN` 的状态文本是否表示「卡在位」（3GPP TS 27.007）。
///
/// `READY` / `SIM PIN` / `SIM PUK` 都算在位（只是可能锁着）；拿不到 `+CPIN`
/// （多数固件这时回 `+CME ERROR: 10` = SIM not inserted）才算不在位；
/// 少数固件用 `SIM REMOVED` 这类文本表示不在位。
pub fn cpin_present(state: &str) -> bool {
    let s = state.trim().to_uppercase();
    if s.is_empty() {
        return false;
    }
    for bad in [
        "REMOVED",
        "NOT INSERTED",
        "INSERTED-NOT",
        "ABSENT",
        "NO SIM",
    ] {
        if s.contains(bad) {
            return false;
        }
    }
    true
}

/// 是不是换了一张卡：**只比 ICCID / IMSI，且两边都非空才比**。
///
/// 读卡偶发失败会回空串，拿空值去比会把「读失败」误判成「换卡」。
pub fn sim_identity_differs(a: &SimIdentity, b: &SimIdentity) -> bool {
    if !a.iccid.is_empty() && !b.iccid.is_empty() && a.iccid != b.iccid {
        return true;
    }
    if !a.imsi.is_empty() && !b.imsi.is_empty() && a.imsi != b.imsi {
        return true;
    }
    false
}

/// 把「上一个 / 这一个」的身份分类成事件（纯函数，便于用例覆盖）。
///
/// - 在位 → 不在位：`removed`；
/// - 不在位 → 在位且身份变了：`swapped`；身份没变（或没读到过旧身份）算 `inserted`
///   （没读到过 ICCID 的固件只能喊一声插卡，**绝不能当换卡去复位模组**）；
/// - 都在位且身份变了：`swapped`。
pub fn sim_hotswap_event(prev: &SimIdentity, cur: &SimIdentity) -> &'static str {
    if prev.present && !cur.present {
        return SIM_REMOVED;
    }
    if !prev.present && cur.present {
        return if sim_identity_differs(prev, cur) {
            SIM_SWAPPED
        } else {
            SIM_INSERTED
        };
    }
    if prev.present && cur.present && sim_identity_differs(prev, cur) {
        return SIM_SWAPPED;
    }
    ""
}

/// 该不该为这个热插拔事件复位模组（`AT+CFUN=1,1`）。
///
/// **只有「确认换了另一张卡」才允许复位**，而且要用户在配置里显式打开
/// （`[sim] hotswap_reset`，默认 false）：复位会让模组重新枚举、掉线重连好几分钟，
/// 误判一次就是「模块反复出不来」。
pub fn should_reset_modem(kind: &str, cfg_reset: bool) -> bool {
    cfg_reset && kind == SIM_SWAPPED
}

/// 现在该不该去读一次 SIM 身份。
///
/// dirty（`mark_sim_dirty()` 被调过，来源见本模块头注）⇒ 立刻读，不等周期；
/// 否则按 `every` 节流；`every` 为零表示配置里关掉了检查。
pub fn should_check_sim(last: Option<Instant>, every: Duration, dirty: bool, now: Instant) -> bool {
    if dirty {
        return true;
    }
    if every.is_zero() {
        return false;
    }
    match last {
        None => true,
        Some(t) => now.saturating_duration_since(t) >= every,
    }
}

/// 冷却期未过？过了才允许再触发一次动作（true = 还在冷却）。
pub fn in_cooldown(cooldown_until: Option<Instant>, now: Instant) -> bool {
    match cooldown_until {
        Some(t) => now < t,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(present: bool, iccid: &str, imsi: &str) -> SimIdentity {
        SimIdentity {
            present,
            state: if present { "READY" } else { "" }.to_string(),
            iccid: iccid.to_string(),
            imsi: imsi.to_string(),
        }
    }

    #[test]
    fn cpin_present_cases() {
        assert!(cpin_present("READY"));
        assert!(cpin_present("SIM PIN"));
        assert!(cpin_present("sim puk"));
        assert!(!cpin_present(""));
        assert!(!cpin_present("SIM REMOVED"));
        assert!(!cpin_present("SIM not inserted"));
        assert!(!cpin_present("INSERTED-NOT-READY"));
    }

    #[test]
    fn identity_differs_needs_both_sides() {
        let a = id(true, "111", "46011");
        let b = id(true, "222", "46011");
        assert!(sim_identity_differs(&a, &b));
        // 一边空串不算差异（读卡偶发失败会回空）
        let c = id(true, "", "46011");
        assert!(!sim_identity_differs(&a, &c));
        assert!(!sim_identity_differs(&a, &a));
    }

    #[test]
    fn events_classified() {
        let have = id(true, "111", "46011");
        let none = id(false, "", "");
        let other = id(true, "222", "46011");
        assert_eq!(sim_hotswap_event(&have, &none), SIM_REMOVED);
        assert_eq!(sim_hotswap_event(&none, &have), SIM_INSERTED);
        // 没读到过旧身份（ICCID/IMSI 全空）⇒ 只能算插卡，绝不能当换卡去复位模组
        assert_eq!(sim_hotswap_event(&none, &other), SIM_INSERTED);
        assert_eq!(sim_hotswap_event(&have, &other), SIM_SWAPPED);
        assert_eq!(sim_hotswap_event(&have, &have), "");
    }

    #[test]
    fn only_confirmed_swap_resets() {
        assert!(should_reset_modem(SIM_SWAPPED, true));
        assert!(!should_reset_modem(SIM_SWAPPED, false));
        assert!(!should_reset_modem(SIM_INSERTED, true));
        assert!(!should_reset_modem(SIM_REMOVED, true));
    }

    #[test]
    fn check_schedule() {
        let now = Instant::now();
        assert!(should_check_sim(None, Duration::from_secs(20), false, now));
        assert!(should_check_sim(
            Some(now),
            Duration::from_secs(20),
            true,
            now
        ));
        assert!(!should_check_sim(
            Some(now),
            Duration::from_secs(20),
            false,
            now
        ));
        assert!(!should_check_sim(
            Some(now),
            Duration::ZERO,
            false,
            now + Duration::from_secs(600)
        ));
    }

    #[test]
    fn cooldown_window() {
        let now = Instant::now();
        assert!(!in_cooldown(None, now));
        assert!(in_cooldown(Some(now + SIM_COOLDOWN), now));
        assert!(!in_cooldown(Some(now), now + SIM_COOLDOWN));
    }
}
