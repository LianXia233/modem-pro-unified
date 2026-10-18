//! `POST /api/netscan/stop` —— 叫停正在进行的主动扫网。
//!
//! 只置标志、立即返回：真正的结果由那个还没返回的 `/api/netscan` 渲染。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let stopped = modem.request_netscan_stop();
    response::ok(req, json!({ "modem_id": modem.id, "stopped": stopped }))
}
