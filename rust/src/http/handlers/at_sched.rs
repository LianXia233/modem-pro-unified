//! `GET /api/at/sched` —— 串口调度状态（排障用）。
//!
//! ## 这个接口回答什么问题
//!
//! 用户报「AT 卡」/「信号值不动」/「点刷新半天没反应」时，光看日志推不出来。
//! 这里给出**每组优先级各被服务了多少次、各等了多久、被限流拦了多少次**，
//! 一眼就能分清是哪一种毛病：
//!
//! | 现象 | 说明 |
//! | --- | --- |
//! | `bulk.served` 暴涨、`signal.wait_ms` 很大 | 冷门命令在挤信号 → 该加缓存或降级 |
//! | `throttled` 持续增长 | 限流闸门在起作用（这是**好事**，说明它挡住了灌爆） |
//! | `waiting > 2` 且 `max_wait_ms` 很大 | 有慢命令占着串口（扫网 / USSD / 短信列表） |
//! | `busy: true` 长期不变 | 卡在某条命令上，去看日志里的超时 |
//!
//! ## 契约
//!
//! ```json
//! {"modem_id":0,
//!  "limits":{"gap_ms":60,"window_ms":10000,"per_window":40},
//!  "waiting":0,"busy":false,"max_wait_ms":312,
//!  "user":  {"served":3,"wait_ms":12,"throttled":0},
//!  "signal":{"served":120,"wait_ms":860,"throttled":4},
//!  "data":  {"served":30,"wait_ms":90,"throttled":0},
//!  "bulk":  {"served":5,"wait_ms":1200,"throttled":2}}
//! ```
//!
//! ⚠️ `waiting` 为 `usize::MAX` 时表示「调度器正卡在限流 sleep 里、拿不到快照」，
//! 不是真的有天文数字个等待者 —— 见 `Scheduler::stats` 的注释。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    // 页面在看：后台采样循环据此判断「有人在盯」
    state.note_http_seen();
    let Some(modem) = modem_by_query(state, _query) else {
        return response::fail(req, -1, "modem not found");
    };
    let st = modem.at_sched_stats();
    let lim = modem.at_sched_limits();
    // 命令节奏（每条 AT 之间 25ms）：拿不到锁（正在发命令）就给 null，
    // 不为了一个统计数字去排队等串口。
    let pacing = modem.at_pacing_stats();
    let (open_fails, open_skipped, open_cooldown_ms) = modem.at_open_stats();
    response::json_body(
        req,
        serde_json::json!({
            "modem_id": modem.id,
            "limits": {
                "gap_ms": lim.gap.as_millis() as u64,
                "window_ms": lim.window.as_millis() as u64,
                "per_window": lim.per_window,
            },
            // 命令级节奏：cmd_gap_ms 是每条 AT 之间的硬性间隔。
            //
            // 拿不到 AT 口锁时（正在发命令）退回编译期常量 —— 这个值本来就是
            // 固定的 30ms，没必要因为「此刻忙」就让页面看不到它。
            "cmd_gap_ms": pacing
                .map(|p| p.0)
                .unwrap_or(crate::modem::at::DEFAULT_GAP.as_millis() as u64),
            // 打开失败冷却：失败多 = 模组没就绪；skipped 多 = 冷却闸在挡
            "open": {
                "fails": open_fails,
                "skipped": open_skipped,
                "cooldown_ms": open_cooldown_ms,
            },
            "pacing": pacing.map(|p| serde_json::json!({
                "throttled": p.1,
                "waited_ms": p.2,
            })),
            "waiting": st.waiting,
            "busy": st.busy,
            "max_wait_ms": st.max_wait_ms,
            "user": st.user,
            "signal": st.signal,
            "data": st.data,
            "bulk": st.bulk,
        }),
    )
}
