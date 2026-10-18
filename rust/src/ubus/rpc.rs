//! ubus JSON-RPC 编解码。

use serde_json::{json, Value};

use crate::error::{msg, Result};

/// 构造 `call` 请求体。
pub fn call_body(session: &str, object: &str, method: &str, params: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "call",
        "params": [session, object, method, params],
    })
    .to_string()
}

/// 把响应文本解析成 JSON。
pub fn parse(raw: &str) -> Result<Value> {
    serde_json::from_str::<Value>(raw).map_err(|e| msg(format!("ubus 响应不是 JSON: {e}")))
}

/// ubus `call` 成功时 `result = [0, <payload>]`，这里取出 payload。
/// 失败（`result[0] != 0`）返回 None。
pub fn ok_payload(resp: &Value) -> Option<&serde_json::Map<String, Value>> {
    let arr = resp.get("result")?.as_array()?;
    if arr.first().and_then(Value::as_i64) != Some(0) {
        return None;
    }
    arr.get(1)?.as_object()
}
