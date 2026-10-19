//! `POST /api/schedule/toggle` —— 启用 / 停用一条任务（`{"id":"...","enabled":true}`）。

use std::collections::HashMap;

use serde::Deserialize;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, schedule, AppState};

#[derive(Deserialize)]
struct ToggleReq {
    #[serde(default)]
    id: String,
    #[serde(default)]
    enabled: bool,
}

pub fn handle(mut req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    let body = request::read_body(&mut req);
    let parsed: ToggleReq = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(e) => return response::fail(req, -1, &format!("请求体应为 JSON：{e}")),
    };
    let Some(mut task) = state
        .sched
        .snapshot()
        .into_iter()
        .find(|t| t.id == parsed.id.trim())
    else {
        return response::fail(req, -1, "任务不存在（可能已被删除）");
    };
    task.enabled = parsed.enabled;
    if let Err(e) = state.sched.upsert(&mut task) {
        return response::fail(req, -1, &e.to_string());
    }
    log::info!(
        "定时任务 {} 已{}：{}",
        task.id,
        if parsed.enabled { "启用" } else { "停用" },
        task.name
    );
    let v = schedule::view(state);
    schedule::push_view(state, &v);
    response::json_body(req, v)
}
