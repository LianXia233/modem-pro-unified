//! `POST /api/lock/unlock` —— 解除全部锁定（频段 + 小区）。
//!
//! 请求体：`{modem_id}`。返回最新锁网状态（同 `GET /api/lock/status`）。

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
    match modem.unlock_all() {
        Ok(v) => {
            crate::http::push::push_lock(state, &modem, &v);
            response::ok(req, serde_json::to_value(v)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
