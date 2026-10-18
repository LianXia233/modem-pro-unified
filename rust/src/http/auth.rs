//! 「跟随 LuCI 登录」闸门。
//!
//! 打开后（默认开），除静态页与少数放行接口外，`/api/*` 都要求请求里带着有效的
//! LuCI 会话 cookie（`modemd_sess`），会话有效性交给 rpcd（ubus `session.get`）判定。
//! ubus 调不起来（非 OpenWrt / rpcd 未起）时**放行并记日志**，避免把整机用户挡在门外。
//!
//! 放行名单：静态页、`/api/version`、`/api/session`、`/api/login`、`/api/logout`；
//! 本机（127.0.0.1）请求免登录。

use tiny_http::Request;

use super::request;
use super::AppState;

/// 会话 cookie 名。
pub const SESSION_COOKIE: &str = "modemd_sess";

/// 该请求是否通过闸门。
pub fn is_authorized(state: &AppState, req: &Request) -> bool {
    if !state.cfg.require_luci_login() {
        return true;
    }
    let path = request::path_of(req.url());
    if is_public_path(&path) {
        return true;
    }
    if client_is_localhost(req) {
        return true;
    }
    match cookie(req, SESSION_COOKIE) {
        None => false,
        Some(sid) => match state.ubus.session_valid(&sid) {
            Ok(valid) => valid,
            Err(e) => {
                log::warn!("ubus 会话校验不可用，放行本次请求: {e}");
                true
            }
        },
    }
}

/// 放行名单。
pub fn is_public_path(path: &str) -> bool {
    if !path.starts_with("/api/") {
        return true; // 静态资源
    }
    matches!(
        path,
        "/api/version" | "/api/session" | "/api/login" | "/api/logout"
    )
}

fn client_is_localhost(req: &Request) -> bool {
    match req.remote_addr() {
        Some(addr) => addr.ip().is_loopback(),
        None => false,
    }
}

/// 从 Cookie 头里取一个值。
pub fn cookie(req: &Request, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    for header in req.headers() {
        if !header.field.equiv("Cookie") {
            continue;
        }
        for part in header.value.as_str().split(';') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix(&needle) {
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}
