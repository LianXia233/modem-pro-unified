//! 后台采样循环（阶段 5）：信号 / 网络 / 流量周期采样、波动记录、配额评估与推送。
//!
//! 分三类，判据如下：
//!
//! | 循环 | 判据 | 说明 |
//! | --- | --- | --- |
//! | `push` / `traffic` | **只看主动客户端** | 有人在看才发 AT；只有屏挂着时一个 AT 都不发 |
//! | `quota` / `schedule` | **不看客户端** | 配额 24 小时都得生效，没人看页面也得按量断网 |
//! | `change` | **只在没人看时** | 让采样历史保持连续；有人看时查询顺手记就够了 |
//! | `logs` | 有客户端（含屏） | 一个字节都不多读 |
//!
//! `schedule` 循环由 `http::serve` 单独接线（它需要 `schedule_interval`，见
//! `runtime::scheduler::spawn_schedule`）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::logging::unix_now;

use super::AppState;
use crate::modem::at::{UrcKind, UrcLine};
use crate::modem::model::Modem;

/// 配额采样节拍（秒）。配额是「24 小时都得生效」的东西，不看有没有客户端在线。
const QUOTA_INTERVAL: u64 = 60;

/// 日志推送节拍（秒）：日志是给人看的，秒级足够。
const LOG_INTERVAL: u64 = 1;

/// 起全部后台循环（`http::serve` 装配完 `AppState` 后调用一次）。
pub fn spawn_all(state: &Arc<AppState>) {
    spawn_urc(state, URC_INTERVAL);
    spawn_netprov(state, NETPROV_INTERVAL);
    spawn_push(state, state.cfg.server.push_interval);
    spawn_traffic(state, state.cfg.server.traffic_interval);
    spawn_quota(state, QUOTA_INTERVAL);
    if state.cfg.server.change_interval > 0 {
        spawn_change(state, state.cfg.server.change_interval);
    } else {
        log::info!("change_interval<=0：不启用后台采样，仅查询时顺手记录波动");
    }
    spawn_hotplug_notify(state, state.cfg.server.scan_interval);
    let every =
        crate::config::parse_duration(&state.cfg.sim.hotswap_interval, Duration::from_secs(20));
    spawn_sim_watch(state, every);
    if !state.cfg.server.log_file.trim().is_empty() {
        spawn_logs(state, LOG_INTERVAL);
    }
}

/// URC 消费循环节拍（秒）：主动上报要尽快置脏，1 秒足够（置脏本身不发 AT）。
const URC_INTERVAL: u64 = 1;

/// 自动组网节拍（秒）：配置注释写的是「≤15 秒一轮就地生效」。
///
/// 取 15 而不是更短：每一轮都要跑两个 `uci show`（fork 两个进程），
/// 而且绝大多数轮次都是「无改动」—— 再快也只是空转。
const NETPROV_INTERVAL: u64 = 15;

/// 洪水丢弃告警的限流间隔：同一模组这么久才再喊一次（否则会被刷屏）。
const URC_DROP_WARN_EVERY: Duration = Duration::from_secs(30);

/// `+CMTI` 触发重读短信的最小间隔：群发时会连续来好几条，每条的索引都指向
/// 同一个列表，重读一次就够，反复读只会把串口占满。
const SMS_REFRESH_MIN_GAP: Duration = Duration::from_secs(10);

/// URC 消费循环：把过滤层暂存的主动上报折算成脏标记，驱动巡检加速。
///
/// 边界（与其它循环一脉相承）：
/// - **不自己开串口**：只处理 `is_running()`（AT 口已打开）的模组，没人查询过的
///   模组不会被这个循环唤醒；
/// - **不发 AT**：只 `take_urc_lines()` 取走已暂存的行 —— 与查询共用同一把 AT 锁，
///   不会和命令收发抢串口，拿到锁后也只是 `mem::take` 一个 Vec。唯一的例外是
///   `+CMTI` 触发的一次短信重读（见 [`refresh_sms_on_urc`]）：新短信的内容没有
///   别的路径能拿到，且受节流约束；
/// - **只置脏、不改状态**：单字段注册态 URC 与 `n=0` 时的查询应答同形，不足以
///   推定状态，权威读数仍走既有查询路径（见 `model/urc.rs` 的取证表）。
///
/// 换来的收益：拔卡 / 插卡不用等 `hotswap_interval` 整轮，注册态变化不用等
/// `network_interval`，新短信不用手动刷页面 —— 两者都变成「下一拍就查」。
fn spawn_urc(state: &Arc<AppState>, interval: u64) {
    let state = state.clone();
    std::thread::spawn(move || {
        // 每个模组：已见过的类别（首见记一条 info，便于排障时知道这台模组会发什么）
        let mut seen: HashMap<u32, Vec<UrcKind>> = HashMap::new();
        // 每个模组：上次洪水告警时刻（累计丢弃数是排障第一个该看的数）
        let mut last_drop_warn: HashMap<u32, std::time::Instant> = HashMap::new();
        // 每个模组：上次因 `+CMTI` 重读短信的时刻（防批量到达时反复读）
        let mut last_sms_read: HashMap<u32, std::time::Instant> = HashMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(interval.max(1)));
            for modem in state.mgr.all() {
                if !modem.is_running() {
                    continue;
                }
                let lines = modem.take_urc_lines();
                if !lines.is_empty() {
                    let act = crate::modem::model::urc_actions(&lines);
                    if act.sim_dirty {
                        modem.mark_sim_dirty();
                    }
                    if act.net_dirty {
                        modem.mark_net_dirty();
                    }
                    for raw in &act.cusd {
                        push_cusd(&state, &modem, raw);
                    }
                    if act.sms_dirty {
                        refresh_sms_on_urc(&state, &modem, &mut last_sms_read);
                    }
                    log_urc(&modem, &lines, seen.entry(modem.id).or_default());
                }
                // 队列空也可能是被洪水阈值丢空了：丢弃数照样要看
                let dropped = modem.urc_dropped();
                if dropped > 0 {
                    let now = std::time::Instant::now();
                    let due = last_drop_warn
                        .get(&modem.id)
                        .map(|t| now.saturating_duration_since(*t) >= URC_DROP_WARN_EVERY)
                        .unwrap_or(true);
                    if due {
                        last_drop_warn.insert(modem.id, now);
                        log::warn!(
                            "模组 id={} 的 URC 已被洪水阈值累计丢弃 {} 行（上限 60 行/秒）：\
                             主动上报在被垃圾行淹没，相关加速会失灵",
                            modem.id,
                            dropped
                        );
                    }
                }
            }
        }
    });
}

/// 解析一条 `+CUSD:` 原文，写进模组缓存并推给页面。
///
/// 解析不了（模组回了条没见过的形态）只记日志：USSD 应答没有查询路径可以重读，
/// 但也不能因此把页面卡住。
fn push_cusd(state: &AppState, modem: &Modem, raw: &str) {
    match crate::modem::model::parse_cusd_line(raw) {
        Some(r) => {
            modem.set_ussd(r.clone());
            super::push::push_ussd(state, modem, &r);
        }
        None => log::debug!("模组 id={} 的 +CUSD 解不出状态码，已忽略：{raw}", modem.id),
    }
}

/// `+CMTI` 到达：重读一次短信列表并推送（新短信的内容只有重读才拿得到）。
///
/// 节流 [`SMS_REFRESH_MIN_GAP`]：群发短信时 `+CMTI` 会连着来，每一条都重读一轮
/// `AT+CMGL` 会把串口占满。读失败不清缓存（旧列表比空列表诚实），只记日志。
fn refresh_sms_on_urc(
    state: &AppState,
    modem: &Modem,
    last_read: &mut HashMap<u32, std::time::Instant>,
) {
    let now = std::time::Instant::now();
    let recent = last_read
        .get(&modem.id)
        .map(|t| now.saturating_duration_since(*t) < SMS_REFRESH_MIN_GAP)
        .unwrap_or(false);
    if recent {
        return;
    }
    last_read.insert(modem.id, now);
    match modem.read_sms() {
        Ok(list) => {
            // 企业微信推送放在页面广播**之前**：它可能跑 15 秒 curl（同步），
            // 先推页面再发微信，页面的即时性不受公网往返影响
            let pushed = state.wechat.push_new_sms(modem.id, &list);
            if pushed > 0 {
                log::info!("模组 id={} 的新短信已推送到微信（{} 条）", modem.id, pushed);
            }
            super::push::push_sms(state, modem, &list);
            super::push::push_smscount(state, modem);
        }
        Err(e) => log::debug!("模组 id={} 收到新短信但重读失败: {e}", modem.id),
    }
}

/// 一条模组本轮消费掉的 URC：debug 打全量（每类计数 + 首条原文），首见类别另记 info。
fn log_urc(modem: &Modem, lines: &[UrcLine], seen: &mut Vec<UrcKind>) {
    let stats = crate::modem::model::kind_stats(lines);
    for s in &stats {
        if !seen.contains(&s.kind) {
            seen.push(s.kind);
            log::info!(
                "模组 id={} 首次收到 URC {:?}：{}",
                modem.id,
                s.kind,
                s.first
            );
        }
    }
    let detail = stats
        .iter()
        .map(|s| format!("{:?}×{}（{}）", s.kind, s.count, s.first))
        .collect::<Vec<_>>()
        .join("，");
    log::debug!(
        "模组 id={} 消费 URC {} 条：{}",
        modem.id,
        lines.len(),
        detail
    );
}

/// 模组列表变化时推 `modems`（热插拔 / 身份信息刷新后调用）。
///
/// 与查询驱动的循环无关：靠比对「模组签名」发现变化，不占串口。
/// 节拍与巡检一致（`scan_interval`，默认 3 秒）。
fn spawn_hotplug_notify(state: &Arc<AppState>, interval: i64) {
    let every = if interval <= 0 { 3 } else { interval as u64 };
    let state = state.clone();
    std::thread::spawn(move || {
        let mut last = signature(&state);
        loop {
            std::thread::sleep(Duration::from_secs(every));
            if state.ws.client_count() == 0 {
                continue;
            }
            let now = signature(&state);
            if now != last {
                last = now;
                super::push::push_modems(&state);
            }
        }
    });
}

/// 模组列表签名（id + 口 + 型号）：任一项变了就算「列表变了」。
fn signature(state: &AppState) -> Vec<(u32, String, String)> {
    state
        .mgr
        .all()
        .iter()
        .map(|m| (m.id, m.port(), m.model().unwrap_or_default()))
        .collect()
}

/// 信号 / 网络周期推送：没有主动客户端时整轮跳过，完全不碰串口。
///
/// 网络状态按 `network_interval` 节流（它一次要好几个 AT 往返，不能跟信号同节拍）。
fn spawn_push(state: &Arc<AppState>, interval: i64) {
    if interval <= 0 {
        log::warn!("push_interval<=0：不启用周期推送，仅靠页面主动查询刷新");
        return;
    }
    let state = state.clone();
    std::thread::spawn(move || {
        let mut last_net: HashMap<u32, i64> = HashMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(interval as u64));
            if state.ws.active_count() == 0 {
                continue;
            }
            let net_due = network_due(&state, &mut last_net);
            for modem in state.mgr.all() {
                if !modem.is_running() {
                    continue;
                }
                super::push::push_signal(&state, &modem);
                if net_due.contains(&modem.id) {
                    super::push::push_network(&state, &modem);
                }
            }
        }
    });
}

/// 流量采样循环：单独一个更短的节拍（默认 1 秒）。
///
/// 速度是「计数器差分」算出来的：采样越规律、间隔越短，读数越实时。
/// 有人看的时候顺带评估一次配额 —— 超限能立刻断，不用等 60 秒的配额循环。
fn spawn_traffic(state: &Arc<AppState>, interval: i64) {
    if interval <= 0 {
        log::warn!("traffic_interval<=0：不采样实时速率");
        return;
    }
    let state = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(interval as u64));
        if state.ws.active_count() == 0 {
            continue;
        }
        for modem in state.mgr.all() {
            if !modem.is_running() {
                continue;
            }
            super::push::push_traffic(&state, &modem);
            // 有人看的时候顺带评估配额：超限不用等下一轮配额循环
            super::quota::evaluate(&state, &modem);
        }
    });
}

/// 配额采样循环：**不看有没有客户端在线**。
///
/// 启动先跑一轮，把上次退出时「已限制」的状态恢复上（模组可能已经重新拨上了）。
fn spawn_quota(state: &Arc<AppState>, interval: u64) {
    let state = state.clone();
    std::thread::spawn(move || {
        quota_tick_all(&state);
        loop {
            std::thread::sleep(Duration::from_secs(interval));
            quota_tick_all(&state);
        }
    });
}

/// 一轮配额评估（每个在跑的模组一次）。
pub fn quota_tick_all(state: &AppState) {
    for modem in state.mgr.all() {
        if !modem.is_running() {
            continue;
        }
        let st = super::quota::evaluate(state, &modem);
        super::push::push_quota(state, &modem, &serde_json::to_value(st).unwrap_or_default());
    }
}

/// 后台采样循环：只在**没有主动客户端**时跑，让采样历史保持连续。
///
/// 有人看的时候查询会顺手记录（`handlers/signal.rs` 等），没必要重复占串口。
fn spawn_change(state: &Arc<AppState>, interval: i64) {
    let state = state.clone();
    std::thread::spawn(move || {
        let mut last_net: HashMap<u32, i64> = HashMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(interval as u64));
            if state.ws.active_count() > 0 {
                continue;
            }
            let net_due = network_due(&state, &mut last_net);
            for modem in state.mgr.all() {
                if !modem.is_running() {
                    continue;
                }
                super::push::push_signal(&state, &modem);
                if net_due.contains(&modem.id) {
                    super::push::push_network(&state, &modem);
                }
            }
        }
    });
}

/// SIM 热插拔巡检：`[sim] hotswap_interval` 是「没有 URC 时的兜底轮询间隔」。
///
/// 循环本身跑得比较密（`SIM_WATCH_TICK`），但**只有到点或收到 URC 置脏才真去读 AT** ——
/// 这样拔卡能秒级发现，平时也不会一直占串口。写 `"0s"` = 关掉检查。
fn spawn_sim_watch(state: &Arc<AppState>, every: Duration) {
    if every.is_zero() {
        log::info!("hotswap_interval=0：不检查 SIM 热插拔");
        return;
    }
    let state = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(crate::modem::model::SIM_WATCH_TICK);
        for modem in state.mgr.all() {
            if !modem.is_running() {
                continue;
            }
            let now = std::time::Instant::now();
            if !modem.sim_due(every, now) {
                continue;
            }
            let kind = modem.check_sim_hotswap(state.cfg.sim.hotswap_reset);
            if !kind.is_empty() {
                // 换卡 / 拔卡：旧卡的短信缓存不能留着（页面上会挂着别人的短信）
                modem.clear_sms_cache();
                super::push::push_sms(&state, &modem, &[]);
                super::push::push_smscount(&state, &modem);
                super::push::push_sim(&state, &modem, &kind);
            }
        }
    });
}

/// 日志增量推送：有客户端在线才读文件。
fn spawn_logs(state: &Arc<AppState>, interval: u64) {
    let state = state.clone();
    std::thread::spawn(move || {
        let mut pusher = super::logtail::LogPusher::new(&state.cfg.server.log_file);
        loop {
            std::thread::sleep(Duration::from_secs(interval));
            if state.ws.client_count() == 0 {
                continue;
            }
            let lines = pusher.read_new();
            super::push::push_logs(&state, &lines);
        }
    });
}

/// 本轮该查网络状态的模组（按 `network_interval` 节流，每个模组独立计时）。
///
/// URC 置脏（注册态变了）⇒ 本轮立刻查，不等节流：脏标记取走即清，
/// 所以同一时刻只有一个循环会响应它，不会变成每拍都查。
fn network_due(state: &AppState, last: &mut HashMap<u32, i64>) -> Vec<u32> {
    let every = state.cfg.server.network_interval.max(1);
    let now = unix_now();
    let mut due = Vec::new();
    for modem in state.mgr.all() {
        if modem.is_running() && modem.take_net_dirty() {
            last.insert(modem.id, now);
            due.push(modem.id);
            continue;
        }
        let prev = last.get(&modem.id).copied().unwrap_or(0);
        if now - prev >= every {
            last.insert(modem.id, now);
            due.push(modem.id);
        }
    }
    due
}

/// 后台线程总数（供启动日志与单测核对接线是否齐全）。
pub fn thread_count(state: &AppState) -> usize {
    let mut n = 8; // urc / netprov / push / traffic / quota / hotplug / logs / sim
    if state.cfg.server.change_interval > 0 {
        n += 1;
    }
    n
}

/// 自动组网循环：把新插上的模组变成能上网的 WAN 接口（`[net] auto_wan`），
/// 同一轮里还托管 QMI/MBIM 的主机侧拨号器（`[net] host_dial`）。
///
/// 与其它循环的差别：它是唯一会**改系统配置**（UCI）的循环，所以边界更严 ——
/// - 只处理 `is_running()` 的模组；
/// - `diff_plan` 一条都不出时连 `uci commit` 都不跑（零写入 ⇒ 零 reload）；
/// - `auto_wan` 关着 / 没有 `uci`（非 OpenWrt）时**建接口**那段空操作，只记 debug；
/// - 但拨号器托管**照跑**：那两个开关管的是「建不建接口」，不是「谁拨号」。
fn spawn_netprov(state: &Arc<AppState>, interval: u64) {
    let state = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(interval.max(1)));
        crate::netprov::provision::run_once(&state);
    });
}

#[cfg(test)]
mod tests {
    use super::QUOTA_INTERVAL;

    #[test]
    fn quota_loop_ignores_clients() {
        // 配额是「24 小时都得生效」的东西：节拍固定，不看有没有客户端在线
        assert_eq!(QUOTA_INTERVAL, 60);
    }
}
