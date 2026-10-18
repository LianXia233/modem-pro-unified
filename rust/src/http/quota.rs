//! 流量配额的查询、累计与超限动作（供 `handlers/quota*.rs` 复用）。
//!
//! 与短信闸门（`smsauth.rs`）同级：这里放的是**域逻辑**，handler 只管取参数与回信封。
//!
//! 口径要点：
//!   · 用量是**差分/积分**出来的：模组给的是开机至今的累计值，本周期用量由
//!     「当前累计 − 上次累计」累加，模组重启（计数器归零）时把当前值当增量；
//!   · 超限时**当下就动手**（停用数据连接 / 关射频）。后台采样循环起来后会按
//!     `block_interval` 反复压制「模组自己又拨上了」，当前由每次查询顺手评估；
//!   · 只动**上网用**的 PDP 上下文（IMS / sos 不能停，见 model/quota.rs）。

use std::sync::Arc;

use crate::config::{QuotaConfig, ACTION_BLOCK_RF, ACTION_WARN};
use crate::http::AppState;
use crate::logging::{timestamp, unix_now};
use crate::modem::model::{quota, Modem, QuotaSettings, QuotaState, TrafficSample};
use crate::runtime::store::{QuotaRecord, QuotaStore};

/// 当前生效的配额设置（json 里存过的优先，否则取配置段默认值）。
pub fn settings_of(state: &AppState, modem_id: u32) -> QuotaSettings {
    settings_with(&state.quota, &state.cfg.quota, modem_id)
}

/// 同 [`settings_of`]，但直接吃台账与配置段（便于按「模组 + 台账」单测）。
pub(crate) fn settings_with(store: &QuotaStore, cfg: &QuotaConfig, modem_id: u32) -> QuotaSettings {
    let mut s = store
        .get(modem_id)
        .settings
        .unwrap_or_else(|| QuotaSettings::from_config(cfg));
    normalize_settings(&mut s);
    s
}

/// 校正本周期已用流量（界面「校正已用流量」）：按当前上下行比例拆到两个方向。
pub fn set_used(state: &AppState, modem: &Arc<Modem>, total: u64) -> QuotaState {
    set_used_with(&state.quota, &state.cfg.quota, modem, total)
}

/// 同 [`set_used`]，但直接吃台账与配置段。
pub(crate) fn set_used_with(
    store: &QuotaStore,
    cfg: &QuotaConfig,
    modem: &Arc<Modem>,
    total: u64,
) -> QuotaState {
    let mut rec = store.get(modem.id);
    let sum = rec.used_up.saturating_add(rec.used_down);
    let (up, down) = if sum == 0 {
        // 没有历史比例可依：整包记到下行（下行通常占大头），总比记在上行更接近实情
        (0, total)
    } else {
        let up = total.saturating_mul(rec.used_up) / sum;
        (up, total.saturating_sub(up))
    };
    rec.used_up = up;
    rec.used_down = down;
    store.put(modem.id, rec);
    // 校正后可能当场超量（也可能不再超量），走一次评估让断网/恢复同步生效
    evaluate_with(store, cfg, modem)
}

/// 一次评估：采样 → 滚动周期 → 累计用量 → 超限动作 → 落盘 → 出状态。
pub fn evaluate(state: &AppState, modem: &Arc<Modem>) -> QuotaState {
    evaluate_with(&state.quota, &state.cfg.quota, modem)
}

/// 同 [`evaluate`]，但直接吃台账与配置段。
pub(crate) fn evaluate_with(
    store: &QuotaStore,
    cfg: &QuotaConfig,
    modem: &Arc<Modem>,
) -> QuotaState {
    let id = modem.id;
    let mut rec = store.get(id);
    let s = settings_with(store, cfg, id);

    let sample = modem.traffic();
    if !sample.supported {
        // 没有计数器就没法计量：如实告诉界面，不动数据连接
        return QuotaState::idle(id, &s, false, sample.note.clone());
    }

    let now = unix_now();
    roll_period(&mut rec, &s, now);
    accumulate(&mut rec, &sample, now);
    let note = enforce(modem, &s, &mut rec, now, cfg);
    store.put(id, rec.clone());

    QuotaState::build(
        id,
        &s,
        rec.used_up,
        rec.used_down,
        rec.period_start,
        rec.period_end,
        rec.blocked,
        rec.suspended,
        rec.blocked_at.clone(),
        sample.estimated,
        true,
        note,
    )
}

/// 保存设置并立即按新口径评估（可能当下就断网 / 恢复）。
pub fn save_settings(state: &AppState, modem: &Arc<Modem>, s: QuotaSettings) -> QuotaState {
    save_settings_with(&state.quota, &state.cfg.quota, modem, s)
}

/// 同 [`save_settings`]，但直接吃台账与配置段。
pub(crate) fn save_settings_with(
    store: &QuotaStore,
    cfg: &QuotaConfig,
    modem: &Arc<Modem>,
    s: QuotaSettings,
) -> QuotaState {
    let mut rec = store.get(modem.id);
    rec.settings = Some(s);
    // 上下行开关 / 上限改了以后，超限与否要重新判定，旧的断网状态不再算数
    rec.blocked = false;
    rec.blocked_at = None;
    rec.last_block_at = 0;
    store.put(modem.id, rec);
    evaluate_with(store, cfg, modem)
}

/// 清零并开始新周期：用量归零、解除限制、恢复数据连接。
pub fn reset_period(state: &AppState, modem: &Arc<Modem>) -> QuotaState {
    reset_period_with(&state.quota, &state.cfg.quota, modem)
}

/// 同 [`reset_period`]，但直接吃台账与配置段。
pub(crate) fn reset_period_with(
    store: &QuotaStore,
    cfg: &QuotaConfig,
    modem: &Arc<Modem>,
) -> QuotaState {
    let id = modem.id;
    let mut rec = store.get(id);
    let s = settings_with(store, cfg, id);

    restore(modem, &s, &mut rec);
    rec.used_up = 0;
    rec.used_down = 0;
    rec.suspended = false;
    rec.blocked = false;
    rec.blocked_at = None;
    rec.last_block_at = 0;
    let now = unix_now();
    let (start, end) = quota::period_bounds(s.reset_day, now);
    rec.period_start = if s.reset_day > 0 { start } else { now };
    rec.period_end = end;

    // 模组的累计计数器也一起清（速率式没有可清的计数器，只重置本地基准），
    // 清完立刻重新采样当基线，否则下一轮差分会把清零前的量又加回来。
    let _ = modem.reset_traffic_counter();
    let sample = modem.traffic();
    rec.last_tx = sample.tx_bytes;
    rec.last_rx = sample.rx_bytes;
    rec.last_rate_up = sample.tx_rate_bps;
    rec.last_rate_down = sample.rx_rate_bps;
    rec.last_sample_at = now;
    store.put(id, rec.clone());

    QuotaState::build(
        id,
        &s,
        0,
        0,
        rec.period_start,
        rec.period_end,
        false,
        false,
        None,
        sample.estimated,
        sample.supported,
        None,
    )
}

/// 本周期不再限制：恢复数据连接，并且这一周期内不再按上限断网。
pub fn unblock(state: &AppState, modem: &Arc<Modem>) -> QuotaState {
    unblock_with(&state.quota, &state.cfg.quota, modem)
}

/// 同 [`unblock`]，但直接吃台账与配置段。
pub(crate) fn unblock_with(
    store: &QuotaStore,
    cfg: &QuotaConfig,
    modem: &Arc<Modem>,
) -> QuotaState {
    let id = modem.id;
    let mut rec = store.get(id);
    let s = settings_with(store, cfg, id);
    restore(modem, &s, &mut rec);
    rec.suspended = true;
    rec.blocked = false;
    rec.blocked_at = None;
    rec.last_block_at = 0;
    store.put(id, rec.clone());

    let sample = modem.traffic();
    QuotaState::build(
        id,
        &s,
        rec.used_up,
        rec.used_down,
        rec.period_start,
        rec.period_end,
        false,
        true,
        None,
        sample.estimated,
        sample.supported,
        Some("本周期已解除限制，用量会继续累计".to_string()),
    )
}

/// 台账里保存的设置再归一一次（老文件 / 手改过的 json 也能安全读入）。
fn normalize_settings(s: &mut QuotaSettings) {
    let mut c = QuotaConfig {
        enabled: s.enabled,
        limit_bytes: s.limit_bytes,
        count_up: s.count_up,
        count_down: s.count_down,
        action: s.action.clone(),
        reset_day: s.reset_day,
        ..Default::default()
    };
    c = c.normalized();
    s.limit_bytes = c.limit_bytes;
    s.reset_day = c.reset_day;
    s.action = c.action().to_string();
}

/// 周期滚动：到了重置日就把用量清零并开始新周期（同时自动解除限制）。
fn roll_period(rec: &mut QuotaRecord, s: &QuotaSettings, now: i64) {
    let (want_start, want_end) = quota::period_bounds(s.reset_day, now);
    if s.reset_day > 0 {
        let expired = rec.period_start <= 0 || rec.period_end <= 0 || now >= rec.period_end;
        rec.period_start = want_start;
        rec.period_end = want_end;
        if expired {
            rec.used_up = 0;
            rec.used_down = 0;
            rec.suspended = false;
            rec.blocked = false;
            rec.blocked_at = None;
            rec.last_block_at = 0;
        }
    } else if rec.period_start <= 0 {
        // 不自动重置：起点就是第一次记账的时刻
        rec.period_start = now;
        rec.period_end = 0;
    }
}

/// 把这一次采样算进本周期用量。
fn accumulate(rec: &mut QuotaRecord, s: &TrafficSample, now: i64) {
    let first = rec.last_sample_at <= 0;
    if s.kind == crate::modem::model::KIND_RATE {
        if !first {
            let dt = now - rec.last_sample_at;
            rec.used_up = rec
                .used_up
                .saturating_add(quota::integrate(s.tx_rate_bps, dt));
            rec.used_down = rec
                .used_down
                .saturating_add(quota::integrate(s.rx_rate_bps, dt));
        }
        rec.last_rate_up = s.tx_rate_bps;
        rec.last_rate_down = s.rx_rate_bps;
    } else if first {
        // 第一次只建基线：不能把「开机至今的总量」当成这一周期的用量
        rec.last_tx = s.tx_bytes;
        rec.last_rx = s.rx_bytes;
    } else {
        rec.used_up = rec
            .used_up
            .saturating_add(quota::delta(s.tx_bytes, rec.last_tx));
        rec.used_down = rec
            .used_down
            .saturating_add(quota::delta(s.rx_bytes, rec.last_rx));
        rec.last_tx = s.tx_bytes;
        rec.last_rx = s.rx_bytes;
    }
    rec.last_sample_at = now;
}

/// 超限评估与动作；返回给界面的说明文案（正常时为 None）。
fn enforce(
    modem: &Modem,
    s: &QuotaSettings,
    rec: &mut QuotaRecord,
    now: i64,
    cfg: &QuotaConfig,
) -> Option<String> {
    let up = if s.count_up { rec.used_up } else { 0 };
    let down = if s.count_down { rec.used_down } else { 0 };
    let used = up.saturating_add(down);
    let limit = s.limit_bytes.max(0) as u64;
    let over = limit > 0 && used >= limit;

    if !s.enabled {
        // 关掉配额时把之前断掉的连接放回来：不然用户只是想关功能，网却没了
        if rec.blocked {
            restore(modem, s, rec);
        }
        return None;
    }
    if !over {
        if rec.blocked {
            restore(modem, s, rec);
        }
        return None;
    }
    if s.action == ACTION_WARN {
        return Some("已达上限（配置为只提醒，未断网）".to_string());
    }
    if rec.suspended {
        return Some("本周期已解除限制，不再按上限断网".to_string());
    }
    // 已经断过网的，按间隔再压一次（模组可能自己又拨上了）
    let due = rec.last_block_at <= 0 || now - rec.last_block_at >= cfg.block_interval;
    if rec.blocked && !due {
        return None;
    }

    let result = if s.action == ACTION_BLOCK_RF {
        modem.set_airplane(true).map(|_| Vec::new())
    } else {
        modem.block_data()
    };
    match result {
        Ok(_) => {
            if !rec.blocked {
                log::warn!(
                    "模组 {} 本周期流量 {} 已达上限 {}，已执行「{}」",
                    modem.id,
                    quota::fmt_bytes(used),
                    quota::fmt_bytes(limit),
                    action_text(&s.action)
                );
            }
            rec.blocked = true;
            rec.last_block_at = now;
            if rec.blocked_at.is_none() {
                rec.blocked_at = Some(timestamp());
            }
            None
        }
        Err(e) => Some(format!(
            "已超量，但执行「{}」失败：{e}",
            action_text(&s.action)
        )),
    }
}

/// 恢复数据连接（按当时用的动作反向操作）。
fn restore(modem: &Modem, s: &QuotaSettings, rec: &mut QuotaRecord) {
    if s.action == ACTION_BLOCK_RF {
        let _ = modem.set_airplane(false);
    } else {
        let _ = modem.unblock_data();
    }
    rec.blocked = false;
    rec.blocked_at = None;
    rec.last_block_at = 0;
}

fn action_text(action: &str) -> &'static str {
    match action {
        ACTION_BLOCK_RF => "关闭射频",
        ACTION_WARN => "只提醒",
        _ => "停用数据连接",
    }
}

/// 台账入口（构造一次，放在 AppState 上）。
pub fn store_for(config_path: &str) -> Arc<QuotaStore> {
    Arc::new(QuotaStore::new(crate::runtime::store::quota_file(
        config_path,
    )))
}

#[cfg(test)]
mod tests {
    use super::{accumulate, action_text, normalize_settings, roll_period};
    use crate::modem::model::{QuotaSettings, TrafficSample, KIND_COUNTER, KIND_RATE};
    use crate::runtime::store::QuotaRecord;

    fn settings(reset_day: i64) -> QuotaSettings {
        QuotaSettings {
            enabled: true,
            limit_bytes: 1024,
            count_up: true,
            count_down: true,
            action: "block".to_string(),
            reset_day,
        }
    }

    #[test]
    fn bad_settings_are_normalized() {
        let mut s = settings(0);
        s.action = "乱写的".to_string();
        s.reset_day = 31;
        s.limit_bytes = -8;
        normalize_settings(&mut s);
        assert_eq!(s.action, "block");
        assert_eq!(s.reset_day, 0);
        assert_eq!(s.limit_bytes, 0);
    }

    #[test]
    fn period_rolls_and_clears_usage() {
        let mut rec = QuotaRecord {
            used_up: 100,
            used_down: 200,
            blocked: true,
            suspended: true,
            period_start: 1000,
            period_end: 2000,
            ..Default::default()
        };
        // 没到止点：只对齐区间，用量保留
        roll_period(&mut rec, &settings(1), 1500);
        assert_eq!((rec.used_up, rec.used_down), (100, 200));
        assert!(rec.blocked);

        // 过了止点：新周期，用量清零、限制解除
        roll_period(&mut rec, &settings(1), 5000);
        assert_eq!((rec.used_up, rec.used_down), (0, 0));
        assert!(!rec.blocked);
        assert!(!rec.suspended);
    }

    #[test]
    fn no_auto_reset_keeps_first_start() {
        let mut rec = QuotaRecord::default();
        roll_period(&mut rec, &settings(0), 1000);
        assert_eq!(rec.period_start, 1000);
        assert_eq!(rec.period_end, 0);
        roll_period(&mut rec, &settings(0), 9000);
        assert_eq!(rec.period_start, 1000);
    }

    #[test]
    fn counter_accumulates_by_difference() {
        let mut rec = QuotaRecord::default();
        let s = |tx: u64, rx: u64| TrafficSample {
            supported: true,
            kind: KIND_COUNTER.to_string(),
            tx_bytes: tx,
            rx_bytes: rx,
            ..Default::default()
        };
        // 第一次只建基线：不能把「开机至今的总量」算进本周期
        accumulate(&mut rec, &s(1000, 2000), 100);
        assert_eq!((rec.used_up, rec.used_down), (0, 0));
        assert_eq!((rec.last_tx, rec.last_rx), (1000, 2000));

        accumulate(&mut rec, &s(1500, 3000), 160);
        assert_eq!((rec.used_up, rec.used_down), (500, 1000));

        // 模组重启 / 计数器归零：当前值就是这一段的增量
        accumulate(&mut rec, &s(300, 100), 220);
        assert_eq!((rec.used_up, rec.used_down), (800, 1100));
    }

    #[test]
    fn rate_is_integrated_over_elapsed() {
        let mut rec = QuotaRecord {
            last_sample_at: 100,
            ..Default::default()
        };
        let s = TrafficSample {
            supported: true,
            kind: KIND_RATE.to_string(),
            estimated: true,
            tx_rate_bps: 8_000_000, // 1 MB/s
            rx_rate_bps: 16_000_000,
            ..Default::default()
        };
        accumulate(&mut rec, &s, 160);
        assert_eq!(rec.used_up, 60 * 1_000_000);
        assert_eq!(rec.used_down, 120 * 1_000_000);
    }

    #[test]
    fn action_labels() {
        assert_eq!(action_text("block"), "停用数据连接");
        assert_eq!(action_text("block_rf"), "关闭射频");
        assert_eq!(action_text("warn"), "只提醒");
        assert_eq!(action_text("未知"), "停用数据连接");
    }
}
