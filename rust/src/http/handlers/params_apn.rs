//! `POST /api/params/apn` —— 写 APN（`{modem_id, cid, pdp_type, apn}`）。
//!
//! 写完回读确认 + 自动重新激活该上下文 + 顺手激活 IMS；每一步的结果放在 `msg` 里
//! （界面把 msg 整段显示出来，让人知道到底做了什么）。

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
    let pdp_type = params
        .get("pdp_type")
        .cloned()
        .unwrap_or_else(|| "IPV4V6".to_string());
    let apn = params.get("apn").cloned().unwrap_or_default();
    match modem.set_apn(cid, &pdp_type, &apn, state.cfg.dial.activate_ims) {
        Ok((st, msg)) => response::ok_msg(req, serde_json::to_value(st)?, &msg),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
