//! 会话相关：`/api/session`、`/api/login`、`/api/logout`。
//!
//! 登录把密码转交 ubus（rpcd）校验，成功后把会话 ID 存进 `modemd_sess` cookie。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{auth, request, response, AppState};

/// `GET /api/session` —— 当前是否已登录。
pub fn session(req: Request, state: &AppState) -> Result<()> {
    let logged_in = match auth::cookie(&req, auth::SESSION_COOKIE) {
        Some(sid) => state.ubus.session_valid(&sid).unwrap_or(false),
        None => false,
    };
    response::ok(req, serde_json::json!({ "logged_in": logged_in }))
}

/// `POST /api/login` —— 校验密码并下发 cookie。
pub fn login(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let user = params
        .get("username")
        .cloned()
        .unwrap_or_else(|| "root".to_string());
    let pass = params.get("password").cloned().unwrap_or_default();

    match state.ubus.session_login(&user, &pass) {
        Ok(sid) => {
            let cookie = format!(
                "{}={sid}; Path=/; HttpOnly; SameSite=Lax",
                auth::SESSION_COOKIE
            );
            response::with_cookie(req, serde_json::json!({ "sid": sid }), &cookie)
        }
        Err(e) => response::fail_status(req, 401, -1, &e.to_string()),
    }
}

/// `POST /api/logout` —— 清掉 cookie。
pub fn logout(req: Request) -> Result<()> {
    let expired = format!(
        "{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax",
        auth::SESSION_COOKIE
    );
    response::with_cookie(req, serde_json::Value::Null, &expired)
}
