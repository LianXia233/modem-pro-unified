//! `POST /api/params/netselect` —— 选网方式（`{modem_id, mode, plmn}`）。
//!
//! `auto` → `AT+COPS=0`；`manual` → `AT+COPS=1,2,"<plmn>"`（PLMN 必须 5~6 位数字）。

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
    let mode = params
        .get("mode")
        .cloned()
        .unwrap_or_else(|| "auto".to_string());
    let plmn = params.get("plmn").cloned().unwrap_or_default();
    match modem.set_netselect(&mode, &plmn) {
        Ok((st, msg)) => response::ok_msg(req, serde_json::to_value(st)?, &msg),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
