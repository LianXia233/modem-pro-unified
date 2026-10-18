//! ubus HTTP 传输：极小的同步 HTTP/1.1 POST（纯 std，不引入 reqwest/ureq）。

use std::io::{Read, Write};
use std::net::TcpStream;

use crate::error::{msg, Result};

use super::UbusClient;

/// 向 rpcd 发一个 POST，返回**去掉响应头后**的正文。
pub fn post(client: &UbusClient, body: &str) -> Result<String> {
    let addr = format!("{}:{}", client.host, client.port);
    let mut stream =
        TcpStream::connect(&addr).map_err(|e| msg(format!("连不上 ubus {addr}: {e}")))?;
    stream.set_read_timeout(Some(client.timeout)).ok();
    stream.set_write_timeout(Some(client.timeout)).ok();

    let req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        client.path,
        client.host,
        body.len(),
        body
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| msg(format!("写 ubus 请求失败: {e}")))?;

    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| msg(format!("读 ubus 响应失败: {e}")))?;
    let text = String::from_utf8_lossy(&buf).to_string();
    Ok(strip_headers(&text))
}

/// 跳过 HTTP 响应头，返回正文。
fn strip_headers(text: &str) -> String {
    match text.find("\r\n\r\n") {
        Some(i) => text[i + 4..].to_string(),
        None => text.to_string(),
    }
}
