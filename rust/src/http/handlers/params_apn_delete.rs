//! `POST /api/params/apn/delete` —— 删除一条 APN 定义（`{modem_id, cid}`）。

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
    let cid = request::i64_param(&params, "cid", 0) as i32;
    match modem.delete_apn(cid) {
        Ok((st, msg)) => response::ok_msg(req, serde_json::to_value(st)?, &msg),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
