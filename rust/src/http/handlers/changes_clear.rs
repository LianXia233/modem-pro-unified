//! `POST /api/changes/clear` —— 清空波动记录与信号采样历史。
//!
//! 请求体 `{"modem_id":0}`；`modem_id` 省略或为负时清空全部模组。清空会连同基线
//! 一并清掉，所以下一轮采样只是重建基线、不会立刻冒出「变化」。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let modem_id = match super::changes::int_param(&params, "modem_id", -1) {
        Ok(v) => v,
        Err(msg) => return response::fail(req, -1, &msg),
    };

    let removed = state.metrics.changes.clear(modem_id);
    // 信号采样历史（曲线数据）一并清掉，否则图表与时间线会对不上。
    let samples = state.metrics.samples.clear(modem_id);

    // 通知所有页面：否则别的标签页还挂着已经删掉的流水。
    state
        .ws
        .broadcast("changes_clear", None, json!({ "modem_id": modem_id }));

    response::ok(
        req,
        json!({ "removed": removed, "samples": samples, "modem_id": modem_id }),
    )
}
