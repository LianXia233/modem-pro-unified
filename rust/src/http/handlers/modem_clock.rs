//! `POST /api/modem/clock` —— 把模组时钟校准到主机，并回一份模组状态。
//!
//! 返回体与 `GET /api/modem/status` 同构（前端拿它直接刷新 SIM/温度/时钟卡片），
//! `module_time` / `clock_skew_s` 就是校完回读的值。

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
    if let Err(e) = modem.sync_clock() {
        return response::fail(req, -1, &e.to_string());
    }
    match modem.modem_status(state.allow_roaming()) {
        Ok(v) => response::ok(req, serde_json::to_value(v)?),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
