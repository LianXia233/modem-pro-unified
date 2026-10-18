//! `POST /api/sms/lock` —— 主动锁定（丢掉当前请求带来的令牌）。

use tiny_http::{Method, Request};

use crate::error::Result;
use crate::http::{response, smsauth, AppState};

pub fn handle(req: Request, state: &AppState) -> Result<()> {
    if *req.method() != Method::Post {
        return response::fail(req, -1, "method not allowed, use POST");
    }
    let token = smsauth::token_from_request(&req);
    state.sms_auth.revoke(&token);
    log::info!("短信已锁定 remote={}", smsauth::client_ip(&req));
    response::ok(req, serde_json::json!({ "unlocked": false }))
}
