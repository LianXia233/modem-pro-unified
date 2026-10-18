//! `POST /api/sms/send` —— 发一条短信（`{modem_id, phone, content}`）。
//!
//! 走短信密码闸门（发短信比读短信更敏感）；中文 / emoji 自动走 UCS2，
//! 超长内容由后端分段发出（见 `model/sms.rs::split_content`）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, smsauth, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    if !state.sms_auth.unlocked(&state.cfg, &req) {
        return smsauth::locked_response(req, state);
    }
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let phone = params.get("phone").cloned().unwrap_or_default();
    let content = params.get("content").cloned().unwrap_or_default();
    if phone.trim().is_empty() || content.trim().is_empty() {
        return response::fail(req, -1, "收件号码与短信内容都不能为空");
    }
    match modem.send_sms(&phone, &content) {
        Ok(r) => {
            log::info!("模组 {} 发出短信至 {}（{} 段）", modem.id, r.phone, r.parts);
            response::ok(req, serde_json::to_value(r)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
