//! 统一 JSON 响应：`{"code":<i64>,"data":<any>,"msg":"<string>"}`。
//!
//! 约定与前端契约一致：`code == 0` 表示成功，非 0 表示失败；`data` 为载荷。

use serde_json::{json, Value};
use tiny_http::{Header, Request, Response, StatusCode};

use crate::error::Result;

const CONTENT_TYPE: &[u8] = b"application/json; charset=utf-8";

/// 成功（code=0）。
pub fn ok(req: Request, data: Value) -> Result<()> {
    write(req, 200, 0, data, "ok")
}

/// 成功但带自定义 msg（如能力探测的说明文案）。
pub fn ok_msg(req: Request, data: Value, msg: &str) -> Result<()> {
    write(req, 200, 0, data, msg)
}

/// 失败（code != 0，HTTP 200；与前端既有约定一致）。
pub fn fail(req: Request, code: i64, message: &str) -> Result<()> {
    write(req, 200, code, Value::Null, message)
}

/// 失败但带 data（HTTP 200；如拨号失败时把「卡在哪一步」的状态带回去给页面）。
pub fn fail_data(req: Request, code: i64, data: Value, message: &str) -> Result<()> {
    write(req, 200, code, data, message)
}

/// 失败并指定 HTTP 状态码（如 401）。
pub fn fail_status(req: Request, status: u16, code: i64, message: &str) -> Result<()> {
    write(req, status, code, Value::Null, message)
}

/// 失败并指定 HTTP 状态码与 `data`（如短信闸门的 `{"locked":true,"required":true}`）。
pub fn fail_status_data(
    req: Request,
    status: u16,
    code: i64,
    data: Value,
    message: &str,
) -> Result<()> {
    write(req, status, code, data, message)
}

/// 成功并下发一个 Set-Cookie（登录用）。
pub fn with_cookie(req: Request, data: Value, cookie: &str) -> Result<()> {
    let body = json!({ "code": 0, "data": data, "msg": "ok" }).to_string();
    let response = Response::from_string(body)
        .with_status_code(StatusCode(200))
        .with_header(content_type())
        .with_header(no_cache())
        .with_header(set_cookie(cookie));
    req.respond(response)?;
    Ok(())
}

fn write(req: Request, status: u16, code: i64, data: Value, msg: &str) -> Result<()> {
    let body = json!({ "code": code, "data": data, "msg": msg }).to_string();
    let response = Response::from_string(body)
        .with_status_code(StatusCode(status))
        .with_header(content_type())
        .with_header(no_cache());
    req.respond(response)?;
    Ok(())
}

/// 禁止缓存的响应头（API 一律不缓存）。
pub fn no_cache() -> Header {
    Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).expect("static header")
}

fn content_type() -> Header {
    Header::from_bytes(&b"Content-Type"[..], CONTENT_TYPE).expect("static header")
}

fn set_cookie(value: &str) -> Header {
    Header::from_bytes(&b"Set-Cookie"[..], value.as_bytes()).expect("static header")
}
