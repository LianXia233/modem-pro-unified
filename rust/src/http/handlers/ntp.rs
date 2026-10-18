//! `GET /api/ntp` —— NTP 校时状态（页面「时间同步」卡片一次拉全）。
//!
//! 字段与 WS `ntp` 消息同形（`Status::to_json`），见 runtime/ntp.rs。
//! 未启用时也如实返回（`enabled=false`、`source="system"`），页面照常展示。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

pub fn handle(req: Request, _state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    let data = crate::runtime::ntp::state().lock().unwrap().to_json();
    response::ok(req, data)
}
