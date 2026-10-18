//! `GET /api/changes[?modem_id=N][&limit=200]` —— 波动记录（网络 / 信号变化时间线）。
//!
//! 返回波动记录（最新在前）+ 各字段当前值。`modem_id` 省略或为负时返回全部模组的
//! 记录（多模组时前端自己筛选）；`limit` 默认 200，上限 1000。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::modem::model::changes::GLOBAL_MAX;

/// `modem_id` 的缺省值：`-1` 表示不限模组。
const ALL_MODEMS: i64 = -1;
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 1000;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let modem_id = match int_param(query, "modem_id", ALL_MODEMS) {
        Ok(v) => v,
        Err(msg) => return response::fail(req, -1, &msg),
    };
    let mut limit = match int_param(query, "limit", DEFAULT_LIMIT) {
        Ok(v) => v,
        Err(msg) => return response::fail(req, -1, &msg),
    };
    if limit <= 0 {
        limit = DEFAULT_LIMIT;
    }
    if limit > MAX_LIMIT {
        limit = MAX_LIMIT;
    }

    let counters = &state.metrics.changes;
    response::ok(
        req,
        json!({
            "modem_id": modem_id,
            "total": counters.count_for(modem_id),
            "limit": limit,
            "max": GLOBAL_MAX,
            "entries": counters.snapshot(modem_id, limit as usize),
            "current": counters.current(modem_id),
        }),
    )
}

/// 取整数型 query 参数（缺省用 `default`；非整数报错）。
pub fn int_param(
    query: &HashMap<String, String>,
    key: &str,
    default: i64,
) -> std::result::Result<i64, String> {
    match query.get(key) {
        Some(v) if !v.is_empty() => v
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{key} must be an integer")),
        _ => Ok(default),
    }
}
