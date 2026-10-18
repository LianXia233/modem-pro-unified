//! `POST /api/params/pdp` —— 激活 / 停用一条 PDP 上下文。
//!
//! `reconnect=true` = 先断后连（让刚写入的 APN 立即生效）；部分固件不许手动停用
//! 内部上下文（IMS），被拒时跳过断开直接激活。

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
    let active = request::bool_param(&params, "active", true);
    let reconnect = request::bool_param(&params, "reconnect", false);
    match modem.set_pdp(cid, active, reconnect) {
        Ok((st, msg)) => response::ok_msg(req, serde_json::to_value(st)?, &msg),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
