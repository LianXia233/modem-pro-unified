//! 同端口网关：识别 WebSocket 升级后本地处理，其余原样转发给内部 tiny_http。
//!
//! 前端把推送地址硬编码为 `location.host + /ws`（同端口），而 tiny_http 不支持
//! 协议升级；因此监听端口由本模块独占：先窥探请求头，`Upgrade: websocket` 的交
//! [`super::ws`]，普通 HTTP 则把已读到的字节连同后续流量透传给 loopback 上的
//! tiny_http 实例。这样既保住了现有处理器代码，又拿到同端口 WebSocket。

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use tiny_http::Server;

use crate::error::{msg, Result};

use super::{router, ws, AppState};

const MAX_HEAD: usize = 16 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// 启动网关与内部 HTTP 服务（阻塞）。
pub fn run(listen: String, state: Arc<AppState>) -> Result<()> {
    let internal = Server::http("127.0.0.1:0").map_err(|e| msg(format!("内部监听失败: {e}")))?;
    let internal_addr: SocketAddr = format!("{}", internal.server_addr())
        .parse()
        .map_err(|_| msg("无法解析内部监听地址"))?;

    {
        let state = state.clone();
        std::thread::spawn(move || {
            for request in internal.incoming_requests() {
                let state = state.clone();
                std::thread::spawn(move || {
                    if let Err(e) = router::handle(request, state) {
                        log::error!("处理请求失败: {e}");
                    }
                });
            }
        });
    }

    let listener =
        TcpListener::bind(&listen).map_err(|e| msg(format!("监听 {listen} 失败: {e}")))?;
    log::info!("HTTP 服务已启动: http://{listen}/");

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(e) => {
                log::debug!("接受连接失败: {e}");
                continue;
            }
        };
        let state = state.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve_conn(stream, internal_addr, state) {
                log::debug!("连接处理结束: {e}");
            }
        });
    }
    Ok(())
}

fn serve_conn(mut stream: TcpStream, internal: SocketAddr, state: Arc<AppState>) -> Result<()> {
    let _ = stream.set_read_timeout(Some(HEAD_TIMEOUT));
    let loopback = stream
        .peer_addr()
        .map(|a| a.ip().is_loopback())
        .unwrap_or(false);

    let head = read_head(&mut stream)?;
    if head.is_empty() {
        return Ok(());
    }
    let _ = stream.set_read_timeout(None);

    if is_websocket(&head) {
        return ws::serve(stream, &head, state, loopback);
    }
    proxy(stream, &head, internal)
}

/// 读取到请求头结束（`\r\n\r\n`）为止，剩余字节留在 socket 里由 `proxy` 透传。
fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while buf.len() < MAX_HEAD {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset
                ) =>
            {
                break
            }
            Err(e) => return Err(msg(format!("读取请求头失败: {e}"))),
        }
    }
    Ok(buf)
}

fn is_websocket(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    if !text.starts_with("get ") || !text.contains("upgrade: websocket") {
        return false;
    }
    let target = text.split_whitespace().nth(1).unwrap_or("");
    target.starts_with("/ws") || target.starts_with("/display/ws")
}

/// 把 `head` 及其后的字节流双向透传给内部 HTTP 服务。
fn proxy(client: TcpStream, head: &[u8], internal: SocketAddr) -> Result<()> {
    let upstream =
        TcpStream::connect(internal).map_err(|e| msg(format!("连接内部服务失败: {e}")))?;
    let _ = client.set_nodelay(true);
    let _ = upstream.set_nodelay(true);

    let mut up_writer = upstream.try_clone()?;
    up_writer.write_all(head)?;
    up_writer.flush()?;

    let mut up_reader = upstream;
    let mut cl_reader = client.try_clone()?;
    let mut cl_writer = client;

    let to_upstream = std::thread::spawn(move || {
        let _ = std::io::copy(&mut cl_reader, &mut up_writer);
        let _ = up_writer.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut up_reader, &mut cl_writer);
    let _ = cl_writer.shutdown(Shutdown::Write);
    let _ = to_upstream.join();
    Ok(())
}
