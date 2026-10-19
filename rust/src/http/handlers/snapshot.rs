//! `GET /api/snapshot?modem_id=N` —— 页面首屏用的**缓存快照**。
//!
//! 一个请求拿齐「概览要显示的东西」，而且**一条 AT 都不发**：
//!
//! | 块 | 来源 |
//! | --- | --- |
//! | `modems` | 模组管理器（内存） |
//! | `blocks.signal` / `network` / `cells` / `lock` | [`StateCache`]（后台采样 / 同步接口 / 异步任务写进去的） |
//! | `ages` | 每块数据的年龄（毫秒），没采过为 `null` |
//! | `stale` | 年龄超过该块的「新鲜阈值」 → 页面显示「数据有 N 秒旧」 |
//! | `dial` | `Modem::dial_status()`（本地状态，不碰串口） |
//! | `tasks` | 该模组正在跑的任务（页面显示进度与取消按钮） |
//!
//! 页面拿到后立刻渲染，再按需提交 `signal.refresh` / `network.refresh` 等任务；
//! 任务完成后重新拉一次快照即可 —— 全程没有请求在等串口。
//!
//! ⚠️ 这不是「实时值」的替代品：要权威值就必须有人去读模组。本接口只保证
//! 「旧值 + 明确的年龄」，让页面不会把 10 分钟前的信号当成现在的。

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use tiny_http::Request;

use crate::error::Result;
use crate::http::cache::StateCache;
use crate::http::{response, AppState};

/// 每块数据的「新鲜」阈值（毫秒）：超过就标 `stale=true`。
///
/// 取值理由：信号/网络是秒级观测（采样间隔 5~60s），小区/锁网是「用户手动触发」
/// 的低频数据，给更宽的窗口，免得一进页面就满屏「过期」。
fn fresh_ms(kind: &str) -> u64 {
    match kind {
        "signal" => 30_000,
        "network" => 60_000,
        "cells" => 30 * 60_000,
        "lock" => 30 * 60_000,
        _ => 5 * 60_000,
    }
}

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    // 「页面还开着」的信号：LuCI 走 HTTP 轮询（它连不上 WS），后台采样循环
    // 靠这个判断有没有人在看（见 AppState::note_http_seen 的注释）。
    state.note_http_seen();

    let modems = state.mgr.all();
    let wanted = query
        .get("modem_id")
        .and_then(|s| s.trim().parse::<u32>().ok());
    let modem = match wanted {
        Some(id) => modems.iter().find(|m| m.id == id).cloned(),
        None => modems.first().cloned(),
    };

    let modems_view: Vec<Value> = modems
        .iter()
        .map(|m| serde_json::to_value(m.view()).unwrap_or(Value::Null))
        .collect();

    let Some(modem) = modem else {
        return response::ok(
            req,
            json!({
                "ts": crate::logging::timestamp(),
                "modem_id": Value::Null,
                "modems": modems_view,
                "blocks": {},
                "ages": {},
                "stale": {},
                "dial": Value::Null,
                "tasks": [],
            }),
        );
    };

    let mut blocks = Map::new();
    let mut ages = Map::new();
    let mut stale = Map::new();
    for kind in StateCache::KINDS {
        match state.cache.get(kind, modem.id) {
            Some((v, age)) => {
                blocks.insert((*kind).to_string(), v);
                ages.insert((*kind).to_string(), json!(age));
                stale.insert((*kind).to_string(), json!(age > fresh_ms(kind)));
            }
            None => {
                ages.insert((*kind).to_string(), Value::Null);
                stale.insert((*kind).to_string(), json!(true));
            }
        }
    }

    // 正在跑的任务（只列该模组的）：页面据此显示进度条 / 取消按钮，
    // 并知道「现在别重复点」。
    let tasks: Vec<Value> = state
        .tasks
        .list(Some(modem.id), true)
        .into_iter()
        .filter_map(|t| serde_json::to_value(t).ok())
        .collect();

    // 拨号状态是本地推断（接口名 / 地址 / 网关），**一条 AT 都不发**，
    // 所以可以顺手归档：这样 `ages.dial` / `stale.dial` 才是真的，
    // 而不是一个「永远 null、永远过期」的占位块。
    let dial = json!({ "state": modem.dial_status() });
    state.cache.put("dial", modem.id, dial.clone());

    response::ok(
        req,
        json!({
            "ts": crate::logging::timestamp(),
            "modem_id": modem.id,
            "modems": modems_view,
            "blocks": blocks,
            "ages": ages,
            "stale": stale,
            "dial": dial,
            "tasks": tasks,
        }),
    )
}
