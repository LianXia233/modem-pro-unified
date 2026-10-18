//! `GET /api/netscan?modem_id=N&tech=lte|nr|both&minutes=3~10` —— 主动扫网。
//!
//! 慢接口：最久要等 `minutes` 分钟（夹到 1~10），期间独占该模组的 AT 口。
//! 请求在这个线程里同步跑完 —— 前端只发一次请求，其它查询会排队等串口。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let tech = query.get("tech").map(String::as_str).unwrap_or("lte");
    let minutes = request::i64_param(query, "minutes", 3);
    let res = modem.scan_network(tech, minutes);
    log::info!(
        "主动扫网完成 modem={} tech={} 运营商={} 耗时={}ms",
        modem.id,
        res.tech,
        res.operators.len(),
        res.duration_ms
    );
    response::ok(req, serde_json::to_value(res)?)
}
