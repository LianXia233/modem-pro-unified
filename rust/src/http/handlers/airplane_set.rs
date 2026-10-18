//! `POST /api/airplane/set` —— 开关飞行模式（`{modem_id, enabled}`；`AT+CFUN=4` / `AT+CFUN=1`）。
//!
//! 不支持 `AT+CFUN?` 的机型（展锐 SIPC 那一族）会被拒绝：关射频会把设备自身的管理通道一起断掉。

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
    let on = request::bool_param(&params, "enabled", false);
    match modem.set_airplane(on) {
        Ok(()) => response::ok(req, serde_json::to_value(modem.airplane())?),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
