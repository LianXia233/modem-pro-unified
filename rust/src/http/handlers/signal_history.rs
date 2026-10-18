//! `GET /api/signal/history?modem_id=N&minutes=30[&limit=2000]` —— 信号采样序列。
//!
//! 时间升序，供前端画曲线。空值字段缺省表示该次未上报，前端据此断开曲线，
//! 而不是当成 0 画出一条假线。`modem_id` 省略或为负时返回全部模组
//! （每条采样带 `modem_id`）；`minutes` 默认 30（上限 24 小时），`limit` 默认 2000。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::modem::model::history::SAMPLE_MAX;

const ALL_MODEMS: i64 = -1;
const DEFAULT_MINUTES: i64 = 30;
const MAX_MINUTES: i64 = 24 * 60;
const DEFAULT_LIMIT: i64 = 2000;
const MAX_LIMIT: i64 = 5000;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let modem_id = match super::changes::int_param(query, "modem_id", ALL_MODEMS) {
        Ok(v) => v,
        Err(msg) => return response::fail(req, -1, &msg),
    };
    let mut minutes = match super::changes::int_param(query, "minutes", DEFAULT_MINUTES) {
        Ok(v) => v,
        Err(msg) => return response::fail(req, -1, &msg),
    };
    if minutes <= 0 {
        minutes = DEFAULT_MINUTES;
    }
    if minutes > MAX_MINUTES {
        minutes = MAX_MINUTES;
    }
    let mut limit = match super::changes::int_param(query, "limit", DEFAULT_LIMIT) {
        Ok(v) => v,
        Err(msg) => return response::fail(req, -1, &msg),
    };
    if limit <= 0 {
        limit = DEFAULT_LIMIT;
    }
    if limit > MAX_LIMIT {
        limit = MAX_LIMIT;
    }

    let since = crate::logging::unix_now() - minutes * 60;
    let samples = state
        .metrics
        .samples
        .snapshot(modem_id, since, limit as usize);
    response::ok(
        req,
        json!({
            "modem_id": modem_id,
            "minutes": minutes,
            "count": samples.len(),
            "stored": state.metrics.samples.count_for(modem_id),
            "max": SAMPLE_MAX,
            "samples": samples,
        }),
    )
}
