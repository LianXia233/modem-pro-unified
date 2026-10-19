//! `POST /api/schedule/delete` —— 删除一条任务（`{"id":"..."}`）。

use std::collections::HashMap;

use serde::Deserialize;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, schedule, AppState};

#[derive(Deserialize)]
struct DeleteReq {
    #[serde(default)]
    id: String,
}

pub fn handle(mut req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    let body = request::read_body(&mut req);
    let parsed: DeleteReq = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(e) => return response::fail(req, -1, &format!("请求体应为 JSON：{e}")),
    };
    if !state.sched.delete(parsed.id.trim()) {
        return response::fail(req, -1, "任务不存在（可能已被删除）");
    }
    log::info!("定时任务已删除：{}", parsed.id.trim());
    let v = schedule::view(state);
    schedule::push_view(state, &v);
    response::json_body(req, v)
}
