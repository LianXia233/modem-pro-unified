//! `GET /api/network?modem_id=N` —— 网络注册 / 运营商（AT+COPS?）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    match modem.network() {
        Ok(v) => response::ok(req, serde_json::to_value(v)?),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
