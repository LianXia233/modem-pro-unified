//! `GET /api/sms/auth` —— 要不要密码、现在解锁了没（**不泄露任何短信内容**）。
//!
//! 前端进「短信」页时可以无脑先问它。

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, smsauth, AppState};

pub fn handle(req: Request, state: &AppState) -> Result<()> {
    let need = state.sms_auth.required(&state.cfg);
    let data = serde_json::json!({
        "required": need,
        "unlocked": !need || state.sms_auth.unlocked(&state.cfg, &req),
        // 还在用默认密码？页面据此提醒改掉
        "default": need && state.sms_auth.is_default(&state.cfg),
        "ttl_s": smsauth::TOKEN_TTL.as_secs(),
        // 外层登录闸门开着时短信密码会被自动停用，界面据此换一种文案
        "outer_auth": state.cfg.require_luci_login(),
    });
    response::ok(req, data)
}
