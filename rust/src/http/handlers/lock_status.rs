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
    let value = serde_json::to_value(modem.lock_status(refresh))?;
    // 进缓存（refresh=1 是「真查了」；refresh=0 走模组内缓存，也一并归档）
    state.cache.put("lock", modem.id, value.clone());
    response::ok(req, value)
}
