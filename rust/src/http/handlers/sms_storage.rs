//! `GET /api/sms/storage?modem_id=N` —— 短信存储用量与支持的存储区。
//!
//! 切换存储区是 `POST /api/sms/storage`（写操作域）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    match modem.query_sms_storage() {
        Ok(st) => response::ok(req, serde_json::to_value(st)?),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
