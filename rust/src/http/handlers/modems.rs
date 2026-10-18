//! `GET /api/modems[?discover=1]` —— 模组列表。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    if query.get("discover").map(|v| v == "1" || v == "true") == Some(true) {
        state.mgr.discover(true);
    }
    response::ok(req, serde_json::to_value(state.mgr.list())?)
}
