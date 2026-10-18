//! ubus 会话语义方法。

use serde_json::{json, Value};

use crate::error::{msg, Result};

use super::rpc::ok_payload;
use super::UbusClient;

/// 匿名会话 ID（ubus 约定：全 0）。
pub const ANONYMOUS: &str = "00000000000000000000000000000000";

/// `session.login`：成功返回会话 ID。
pub fn login(client: &UbusClient, username: &str, password: &str) -> Result<String> {
    let resp = client.call(
        ANONYMOUS,
        "session",
        "login",
        json!({ "username": username, "password": password }),
    )?;
    if let Some(obj) = ok_payload(&resp) {
        if let Some(sid) = obj.get("ubus_rpc_session").and_then(Value::as_str) {
            return Ok(sid.to_string());
        }
    }
    Err(msg("ubus session.login 失败（用户名或密码错误）"))
}

/// `session.get`：会话有效返回 true。
pub fn valid(client: &UbusClient, sid: &str) -> Result<bool> {
    let resp = client.call(sid, "session", "get", json!({}))?;
    Ok(ok_payload(&resp).is_some())
}
