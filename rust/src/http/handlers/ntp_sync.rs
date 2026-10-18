//! `POST /api/ntp/sync` —— 立即同步一次（不等下一个节拍）。
//!
//! 只置一个「强制」标志：后台循环每秒醒一次，最迟 1 秒后执行（见 runtime/ntp.rs）。
//! 请求立刻返回，结果靠 WS `ntp` 消息推给页面 —— 不在这里同步等应答，
//! 免得三台服务器挨个超时把请求挂住十几秒。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

pub fn handle(req: Request, _state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    crate::runtime::ntp::request_sync();
    let data = crate::runtime::ntp::state().lock().unwrap().to_json();
    response::ok_msg(req, data, "已请求立即同步（1 秒内执行，结果会推送到页面）")
}
