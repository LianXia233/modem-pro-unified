//! `GET /api/lock/status?modem_id=N[&refresh=1]` —— 锁频段 / 锁小区状态。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let refresh = query.get("refresh").map(|v| v == "1").unwrap_or(false);
    response::ok(req, serde_json::to_value(modem.lock_status(refresh))?)
}
