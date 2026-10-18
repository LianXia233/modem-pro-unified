//! `POST /api/schedule/run` —— 立即执行一次开始动作（调试用，`{"id":"..."}`）。
//!
//! 执行了但模组拒绝时按 `fail_data` 返回（`code=-1` + 视图 + 错误文本）：
//! 界面直接显示错误，同时列表里的「上次结果」也同步刷新，便于对账。

use std::collections::HashMap;

use serde::Deserialize;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, schedule, AppState};

#[derive(Deserialize)]
struct RunReq {
    #[serde(default)]
    id: String,
}

pub fn handle(mut req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    let body = request::read_body(&mut req);
    let parsed: RunReq = serde_json::from_str(&body)
        .map_err(|e| crate::error::msg(format!("请求体应为 JSON：{e}")))?;
    let run = match schedule::run_now(state, parsed.id.trim()) {
        Ok(run) => run,
        Err(e) => return response::fail(req, -1, &e.to_string()),
    };
    if !run.ok {
        return response::fail_data(
            req,
            -1,
            serde_json::to_value(schedule::view(state))?,
            &run.msg,
        );
    }
    response::ok_msg(req, serde_json::to_value(schedule::view(state))?, "已执行")
}
