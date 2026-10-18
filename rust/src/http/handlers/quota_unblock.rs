//! `POST /api/quota/unblock` —— 本周期不再按上限断网（并恢复数据连接）。

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
    let view = quota::unblock(state, &modem);
    // 刚恢复了数据连接：立刻广播，免得页面还显示着超限状态
    crate::http::push::push_quota(state, &modem, &serde_json::to_value(&view)?);
    response::ok(req, serde_json::to_value(view)?)
}
