//! `POST /api/lock/cell` —— 锁小区 / 锁频点 / 解除。
//!
//! 请求体：`{modem_id, tech, lock_type, earfcn, pcid?, scs?}`。
//! `lock_type`：0=解除 1=锁频点+PCI 2=仅锁频点。`scs` 仅 NR 用（子载波间隔）。
//! 返回最新锁网状态（同 `GET /api/lock/status`）。

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
    let tech = params
        .get("tech")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let lock_type = request::i64_param(&params, "lock_type", 1) as i32;
    let earfcn = request::i64_param(&params, "earfcn", 0) as i32;
    let pcid = request::i64_param(&params, "pcid", 0) as i32;
    let scs = request::i64_param(&params, "scs", 0);
    let scs = (scs > 0).then_some(scs as i32);
    match modem.set_cell_lock(&tech, lock_type, earfcn, pcid, scs) {
        Ok(v) => {
            crate::http::push::push_lock(state, &modem, &v);
            response::ok(req, serde_json::to_value(v)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
