//! `POST /api/quota/reset` —— 本周期用量清零并开始新周期（同时恢复数据连接）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{quota, request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let view = quota::reset_period(state, &modem);
    // 清零后不再超限：立刻广播，页面上的「已断网」提示要跟着消失
    crate::http::push::push_quota(state, &modem, &serde_json::to_value(&view)?);
    response::ok(req, serde_json::to_value(view)?)
}
