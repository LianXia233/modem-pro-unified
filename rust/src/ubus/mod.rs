//! ubus 客户端（HTTP JSON-RPC，走 `http://127.0.0.1/ubus`）。
//!
//! 用途：把「是否登录过路由器」这件事外包给 rpcd ——
//! 页面把密码 POST 给 modemd 的 `/api/login`，modemd 用它去调 ubus `session.login`，
//! 拿回会话 ID 存进 cookie；之后每个请求拿该 ID 调 `session.get` 校验。
//!
//! 拆分为：`transport`（HTTP 收发）/ `rpc`（JSON-RPC 编解码）/ `session`（语义方法）。

mod rpc;
mod session;
mod transport;

use std::time::Duration;

use serde_json::Value;

use crate::error::Result;

/// 与 rpcd 的一个会话连接参数。
pub struct UbusClient {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) path: String,
    pub(crate) timeout: Duration,
}

impl UbusClient {
    /// 从形如 `http://127.0.0.1/ubus` 的地址构造；接受缺省端口（80）。
    pub fn from_url(url: &str) -> Self {
        let rest = url.trim().trim_start_matches("http://");
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/ubus"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(80)),
            None => (authority.to_string(), 80),
        };
        Self {
            host: if host.is_empty() {
                "127.0.0.1".to_string()
            } else {
                host
            },
            port,
            path: if path.is_empty() {
                "/ubus".to_string()
            } else {
                path.to_string()
            },
            timeout: Duration::from_secs(5),
        }
    }

    /// 发一个 JSON-RPC `call`，返回整个 JSON 响应。
    pub fn call(&self, session: &str, object: &str, method: &str, params: Value) -> Result<Value> {
        let body = rpc::call_body(session, object, method, params);
        let raw = transport::post(self, &body)?;
        rpc::parse(&raw)
    }

    /// `session.login`：成功返回会话 ID。
    pub fn session_login(&self, username: &str, password: &str) -> Result<String> {
        session::login(self, username, password)
    }

    /// `session.get`：会话有效返回 true。
    pub fn session_valid(&self, sid: &str) -> Result<bool> {
        session::valid(self, sid)
    }
}
