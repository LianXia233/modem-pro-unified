//! `GET /api/modem/status?modem_id=N` —— SIM 卡与模组状态。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let allow_roaming = state.allow_roaming();
    match modem.modem_status(allow_roaming) {
        Ok(v) => response::ok(req, serde_json::to_value(v)?),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
