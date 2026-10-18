//! `GET /api/netinfo?modem_id=N` —— 查询网络接入信息（原样回厂商应答行，`|` 连接）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    match modem.net_info() {
        Ok(text) => response::ok(req, serde_json::Value::String(text)),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
