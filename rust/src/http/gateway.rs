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
        // WS 走的是**真实连接**，`loopback` 就是客户端地址，不存在下面说的失真问题
        return ws::serve(stream, &head, state, loopback);
    }
    let client_ip = stream
        .peer_addr()
        .map(|a| a.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    proxy(stream, &head, internal, client_ip)
}

/// 把真实客户端地址注入请求头（**覆盖式**，客户端伪造无效）。
///
/// ## 为什么必须这样做
///
/// 普通 HTTP 请求被本网关转给内部 tiny_http（`127.0.0.1:0`），对端地址因此
/// **恒为 127.0.0.1**。而 `auth::client_is_localhost()` 原来直接看
/// `req.remote_addr().is_loopback()` —— 于是「本机免登录」对所有请求都成立，
/// `require_luci_login = true` 形同虚设：局域网里任何人都能直接访问 9527
/// 调用全部接口（改 IMEI、发短信、切飞行模式…）。
///
/// 修法：网关在这里把**真实**对端地址写进 `X-Real-IP`，并且先删掉客户端
/// 自带的那一行 —— 否则外部请求可以自己伪造一个 `X-Real-IP: 127.0.0.1` 绕过闸门。
fn inject_real_ip(head: &[u8], ip: std::net::IpAddr) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    // 正常情况一定有 `\r\n\r\n`；异常截断时退回「原样 + 追加」，不让请求整个丢掉
    let (head_part, tail) = match text.find("\r\n\r\n") {
        Some(i) => (&text[..i], &text[i..]),
        None => (text.as_ref(), ""),
    };
    let mut lines: Vec<&str> = head_part.split("\r\n").collect();
    lines.retain(|l| {
        !l.trim_start()
            .to_ascii_lowercase()
            .starts_with("x-real-ip:")
    });
    let mut out = lines.join("\r\n");
    out.push_str(&format!("\r\nX-Real-IP: {ip}"));
    out.push_str(tail);
    out.into_bytes()
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
fn proxy(
    client: TcpStream,
    head: &[u8],
    internal: SocketAddr,
    client_ip: std::net::IpAddr,
) -> Result<()> {
    let upstream =
        TcpStream::connect(internal).map_err(|e| msg(format!("连接内部服务失败: {e}")))?;
    let _ = client.set_nodelay(true);
    let _ = upstream.set_nodelay(true);

    let mut up_writer = upstream.try_clone()?;
    // 注入真实对端地址：内部 tiny_http 的 remote_addr 恒为 127.0.0.1，
    // 鉴权闸门只能靠这个头判断「是不是本机请求」（见 inject_real_ip 的说明）
    up_writer.write_all(&inject_real_ip(head, client_ip))?;
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

#[cfg(test)]
mod tests {
    use super::inject_real_ip;
    use std::net::{IpAddr, Ipv4Addr};

    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    #[test]
    fn injects_real_ip_before_body_separator() {
        let head = b"GET /api/snapshot HTTP/1.1\r\nHost: x\r\n\r\n";
        let out = text(&inject_real_ip(
            head,
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 9)),
        ));
        assert!(out.contains("\r\nX-Real-IP: 192.168.1.9\r\n"), "{out}");
        // 注入的头必须在空行之前，否则会被当成 body
        let (h, _) = out.split_once("\r\n\r\n").expect("应有头体分隔");
        assert!(h.contains("X-Real-IP"), "头里没有注入的地址：{h}");
        assert!(out.ends_with("\r\n\r\n"), "头体分隔不能被破坏");
    }

    #[test]
    fn overrides_forged_real_ip() {
        // 外部请求可以自己带一个 X-Real-IP: 127.0.0.1 想绕过闸门 —— 必须被覆盖掉
        let head =
            b"GET /api/imei/set HTTP/1.1\r\nX-Real-IP: 127.0.0.1\r\nX-Real-IP: 127.0.0.1\r\n\r\n";
        let out = text(&inject_real_ip(
            head,
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        ));
        assert_eq!(
            out.matches("X-Real-IP").count(),
            1,
            "伪造的 X-Real-IP 必须被删干净：{out}"
        );
        assert!(out.contains("X-Real-IP: 203.0.113.7"), "{out}");
        assert!(!out.contains("127.0.0.1"), "伪造值残留：{out}");
    }

    #[test]
    fn keeps_loopback_for_genuine_local_calls() {
        // ucode 代理从 127.0.0.1 发起：注入的就是 127.0.0.1，闸门照旧放行
        let head = b"GET /api/snapshot HTTP/1.1\r\nHost: x\r\n\r\n";
        let out = text(&inject_real_ip(head, IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(out.contains("X-Real-IP: 127.0.0.1"), "{out}");
    }

    #[test]
    fn truncated_head_does_not_lose_request() {
        // 没有头体分隔（异常/超长截断）：不能把整个请求丢掉，仍要能转发
        let head = b"GET /api/x HTTP/1.1\r\nHost: y";
        let out = text(&inject_real_ip(
            head,
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        ));
        assert!(out.starts_with("GET /api/x HTTP/1.1"), "{out}");
        assert!(out.contains("X-Real-IP: 10.0.0.1"), "{out}");
    }
}
