//! `POST /api/modem/reset` —— 重启模组（`AT+CFUN=1,1`）。

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
    match modem.reboot() {
        // 没收到确认也按成功返回：指令已写进串口，重启本来就会断开 AT 口
        Ok(v) => response::ok(req, serde_json::to_value(v)?),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
