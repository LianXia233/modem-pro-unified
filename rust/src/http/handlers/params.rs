//! `GET /api/params?modem_id=N` —— PDP 上下文与选网总览。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    // 默认走 15 秒缓存（进一次设置页要问三条命令，不值得每次重来）；
    // 页面「刷新」按钮带 `?refresh=1` 强制实时问模组。
    let st = if request::bool_param(query, "refresh", false) {
        modem.params_refresh()
    } else {
        modem.params()
    };
    response::ok(req, serde_json::to_value(st)?)
}
