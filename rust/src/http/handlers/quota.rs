//! `GET /api/quota?modem_id=N` —— 流量配额状态（含本次采样后的本周期用量）。
//!
//! 这个接口会**顺手评估超限动作**：后台采样循环起来前，页面每次打开/刷新配额卡片
//! 就是一次评估时机（超量即停用数据连接，见 http/quota.rs）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{quota, response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    response::ok(req, serde_json::to_value(quota::evaluate(state, &modem))?)
}
