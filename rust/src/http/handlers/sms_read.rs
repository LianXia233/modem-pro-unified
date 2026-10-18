//! `GET /api/sms/read?modem_id=N` —— 读取全部短信（按到达时间倒序）。
//!
//! 走短信密码闸门：未解锁回 401，前端据此弹密码框。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, smsauth, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    if !state.sms_auth.unlocked(&state.cfg, &req) {
        return smsauth::locked_response(req, state);
    }
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    match modem.read_sms() {
        Ok(list) => {
            // 顺手广播：别的页面（含屏端）不用再各自读一遍模组
            crate::http::push::push_sms(state, &modem, &list);
            crate::http::push::push_smscount(state, &modem);
            response::ok(req, serde_json::to_value(list)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
