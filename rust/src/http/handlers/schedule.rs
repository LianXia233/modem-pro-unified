//! `GET /api/schedule` —— 定时列表 + 可选动作 + 目标模组下拉（页面一次拉全）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, schedule, AppState};

pub fn handle(req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    response::ok(req, serde_json::to_value(schedule::view(state))?)
}
