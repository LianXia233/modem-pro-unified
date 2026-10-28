//! 「跟随 LuCI 登录」闸门。
//!
//! 打开后（默认开），除少数放行接口外，`/api/*` 都要求请求里带着有效的
//! LuCI 会话 cookie（`modemd_sess`），会话有效性交给 rpcd（ubus `session.get`）判定。
//! ubus 调不起来（非 OpenWrt / rpcd 未起）时**放行并记日志**，避免把整机用户挡在门外。
//!
//! 放行名单：`/api/version`、`/api/session`、`/api/login`、`/api/logout`；
//! 本机（127.0.0.1）请求免登录。非 `/api/` 路径不再有静态页，由路由层直接 404。

use tiny_http::Request;

use super::request;
use super::AppState;

/// 会话 cookie 名。
pub const SESSION_COOKIE: &str = "modemd_sess";

/// ubus 会话校验 + 失败放行（`sid` 来自 Cookie 或 `?luci_sid=`）。
///
/// `ctx` 只进日志（如「本次请求」「本次 WebSocket 连接」），不参与判定。
/// 放行语义必须与 WS 侧逐字一致——两处漂移会变成「HTTP 进不去、WS 进得去」
/// 这类只影响半边的认证缺口，且极难在测试里发现。
pub fn session_valid(state: &AppState, sid: &str, ctx: &str) -> bool {
    match state.ubus.session_valid(sid) {
        Ok(valid) => valid,
        Err(e) => {
            log::warn!("ubus 会话校验不可用，放行{ctx}: {e}");
            true
        }
    }
}

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
        Some(sid) => session_valid(state, &sid, "本次请求"),
    }
}

/// 放行名单。
pub fn is_public_path(path: &str) -> bool {
    if !path.starts_with("/api/") {
        return true; // 无需保护：非 /api/ 路径已无静态页，路由层直接 404
    }
    matches!(
        path,
        "/api/version" | "/api/session" | "/api/login" | "/api/logout"
    )
}

/// 请求的真实来源地址。
///
/// 优先读网关注入的 `X-Real-IP`：普通 HTTP 请求是先被 `gateway::proxy` 转给
/// 内部 tiny_http 的，`req.remote_addr()` 在那条链路上**恒为 127.0.0.1**。
/// 早先直接用它判「本机请求」，导致 `require_luci_login` 对所有请求都放行
/// —— 默认 `listen = "0.0.0.0:9527"` 下，局域网内任何人都能无密码调用
/// 全部接口（改 IMEI / 发短信 / 切飞行模式）。
///
/// `X-Real-IP` 由网关**覆盖式**写入（先删客户端自带的那行），所以外部伪造无效。
/// 读不到这个头时（理论只有直连内部端口的测试会这样）才退回 `remote_addr`。
fn client_addr(req: &Request) -> Option<std::net::IpAddr> {
    for header in req.headers() {
        if header.field.equiv("X-Real-IP") {
            if let Ok(ip) = header.value.as_str().trim().parse() {
                return Some(ip);
            }
        }
    }
    req.remote_addr().map(|a| a.ip())
}

fn client_is_localhost(req: &Request) -> bool {
    client_addr(req).map(|ip| ip.is_loopback()).unwrap_or(false)
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
