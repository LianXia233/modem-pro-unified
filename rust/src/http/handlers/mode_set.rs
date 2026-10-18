//! `POST /api/mode/set` —— 下发组网模式偏好（`{modem_id, value}`），写完回读确认。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let value = params.get("value").cloned().unwrap_or_default();
    match modem.set_mode_pref(&value) {
        Ok(st) => response::ok(req, serde_json::to_value(st)?),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
