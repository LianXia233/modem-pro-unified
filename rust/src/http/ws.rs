//! 同端口 WebSocket 推送（`/ws`、`/display/ws`）。
//!
//! 前端把地址硬编码为 `location.host + /ws`，所以必须与 HTTP 同端口；
//! tiny_http 不支持协议升级，故由 [`super::gateway`] 在 TCP 层识别升级请求后
//! 交给本模块完成握手与帧编解码。
//!
//! 消息统一为 `{type, modem_id?, data}`；客户端握手后发 `{action:"snapshot"}`
//! 索取初始快照。无客户端连接时上层不应驱动 AT（见 [`WsHub::has_clients`]）。

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::error::{msg, Result};

use super::auth;
use super::{request, AppState};

/// 屏端（display）固定握手 key。
pub const DISPLAY_KEY: &str = "h29k-tft-9f3c1a7b";

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_FRAME: usize = 1 << 20;

/// 一个已建立的 WebSocket 连接（对外只暴露写能力）。
pub struct WsClient {
    pub id: u64,
    /// 握手携带的短信访问令牌（短信推送按令牌过滤，见 docs/PORTING.md）。
    pub sms_token: Option<String>,
    /// 屏端（`/display/ws`）：被动连接，只读缓存，不算「有人在看」。
    ///
    /// 那块屏 7x24 在线，不能因为它就让模组每秒被问一遍 —— 周期推送循环
    /// 据此跳过（见 [`WsHub::active_count`]）。
    pub display: bool,
    stream: Mutex<TcpStream>,
}

impl WsClient {
    /// 发送一帧文本。
    pub fn send_text(&self, text: &str) -> Result<()> {
        let mut stream = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        write_frame(&mut *stream, 0x1, text.as_bytes())
    }

    /// 发送一条标准事件 `{type, modem_id?, data}`。
    pub fn send_event(&self, msg_type: &str, modem_id: Option<u32>, data: Value) -> Result<()> {
        let mut obj = json!({ "type": msg_type, "data": data });
        if let Some(id) = modem_id {
            obj["modem_id"] = json!(id);
        }
        self.send_text(&obj.to_string())
    }
}

/// 推送中枢：登记所有在线连接并广播。
pub struct WsHub {
    next_id: AtomicU64,
    clients: Mutex<Vec<Arc<WsClient>>>,
}

impl WsHub {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            clients: Mutex::new(Vec::new()),
        }
    }

    /// 在线连接数（含屏端被动连接）。
    pub fn client_count(&self) -> usize {
        self.lock().len()
    }

    /// 在线**主动**客户端数（不含屏端这类被动只读连接）。
    ///
    /// 周期推送循环用它决定要不要发 AT：只有屏挂着时一个 AT 都不发
    /// （屏拿的就是现有缓存）；想让没人看网页时也保持刷新，靠
    /// `change_interval` 后台采样（见 `http/loops.rs`）。
    pub fn active_count(&self) -> usize {
        self.lock().iter().filter(|c| !c.display).count()
    }

    /// 广播一条事件；写失败的连接随后会在读循环里被回收。
    pub fn broadcast(&self, msg_type: &str, modem_id: Option<u32>, data: Value) {
        let clients: Vec<Arc<WsClient>> = self.lock().clone();
        for client in clients {
            if let Err(e) = client.send_event(msg_type, modem_id, data.clone()) {
                log::debug!("推送失败（客户端 {}）: {e}", client.id);
            }
        }
    }

    /// 广播一条**只给部分连接**的事件（`allow` 返回 false 的连接跳过）。
    ///
    /// 用于短信推送：内容带正文，必须过短信密码闸门（见 `http/smsauth.rs`），
    /// 闸门与短信接口是同一把。条数角标（`smscount`）不含正文，走普通
    /// [`Self::broadcast`] 即可。
    pub fn broadcast_sms(
        &self,
        msg_type: &str,
        modem_id: Option<u32>,
        data: Value,
        allow: impl Fn(&WsClient) -> bool,
    ) {
        let clients: Vec<Arc<WsClient>> = self.lock().clone();
        for client in clients {
            if !allow(&client) {
                continue;
            }
            if let Err(e) = client.send_event(msg_type, modem_id, data.clone()) {
                log::debug!("推送失败（客户端 {}）: {e}", client.id);
            }
        }
    }

    fn register(
        &self,
        stream: TcpStream,
        sms_token: Option<String>,
        display: bool,
    ) -> Arc<WsClient> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let client = Arc::new(WsClient {
            id,
            sms_token,
            display,
            stream: Mutex::new(stream),
        });
        self.lock().push(client.clone());
        client
    }

    fn unregister(&self, id: u64) {
        self.lock().retain(|c| c.id != id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<WsClient>>> {
        self.clients.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for WsHub {
    fn default() -> Self {
        Self::new()
    }
}

/// 完成握手并进入读循环（阻塞，直到连接关闭）。
pub fn serve(stream: TcpStream, head: &[u8], state: Arc<AppState>, loopback: bool) -> Result<()> {
    let text = String::from_utf8_lossy(head);
    let (target, headers) = parse_head(&text);
    let path = request::path_of(&target);
    let query = request::parse_query(&target);

    if path == "/display/ws" {
        if query.get("key").map(String::as_str) != Some(DISPLAY_KEY) {
            return deny(stream);
        }
    } else if !authorized(&state, &headers, &query, loopback) {
        return deny(stream);
    }

    let key = match header(&headers, "sec-websocket-key") {
        Some(k) => k,
        None => return deny(stream),
    };

    let mut stream = stream;
    let _ = stream.set_nodelay(true);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(&key)
    );
    stream.write_all(response.as_bytes())?;

    let client = state.ws.register(
        stream.try_clone()?,
        query.get("sms_token").cloned(),
        path == "/display/ws",
    );
    log::info!("WebSocket 已连接: {} (id={})", path, client.id);

    let result = pump(&mut stream, &client, &state);
    state.ws.unregister(client.id);
    log::info!("WebSocket 已断开: id={}", client.id);
    result
}

fn pump(stream: &mut TcpStream, client: &Arc<WsClient>, state: &Arc<AppState>) -> Result<()> {
    loop {
        match read_frame(stream)? {
            None => return Ok(()),
            Some((0x8, _)) => return Ok(()),
            Some((0x9, payload)) => write_frame(stream, 0xA, &payload)?,
            Some((0x1, payload)) => {
                let text = String::from_utf8_lossy(&payload);
                handle_text(client, state, text.trim());
            }
            Some(_) => {}
        }
    }
}

fn handle_text(client: &Arc<WsClient>, state: &Arc<AppState>, text: &str) {
    let cmd: WsCommand = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    match cmd.action.as_str() {
        "snapshot" => snapshot(client, state),
        "ping" => {
            let _ = client.send_text("{\"type\":\"pong\"}");
        }
        // ⚠️ 下面几个都要占串口（发短信要等提示符、切模式要等回读），
        //    一律丢到独立线程：WS 读循环卡住会让这一路的心跳与推送全停。
        "sms_read" => {
            let Some(modem) = modem_of(state, cmd.modem_id) else {
                return;
            };
            let idx = match parse_index(&cmd.index) {
                Some(v) => v,
                None => return,
            };
            let state = state.clone();
            std::thread::spawn(move || {
                if let Err(e) = modem.mark_sms_read(&idx.to_string()) {
                    log::warn!("标记短信已读失败（模组 {}）: {e}", modem.id);
                    return;
                }
                // 状态变了 ⇒ 重读一遍：列表 / 条数 / 未读角标一起回落
                refresh_and_push_sms(&state, &modem);
            });
        }
        "sms_delete" => {
            let Some(modem) = modem_of(state, cmd.modem_id) else {
                return;
            };
            let idx = match parse_index(&cmd.index) {
                Some(v) => v,
                None => return,
            };
            let state = state.clone();
            std::thread::spawn(move || {
                if let Err(e) = modem.delete_sms(&idx.to_string()) {
                    log::warn!("屏端删除短信失败（模组 {}）: {e}", modem.id);
                    return;
                }
                refresh_and_push_sms(&state, &modem);
            });
        }
        "mode_get" => {
            let Some(modem) = modem_of(state, cmd.modem_id) else {
                return;
            };
            let state = state.clone();
            std::thread::spawn(move || {
                super::push::push_mode(&state, &modem, String::new());
            });
        }
        "mode_set" => {
            let Some(modem) = modem_of(state, cmd.modem_id) else {
                return;
            };
            let value = cmd.value.trim().to_string();
            if value.is_empty() {
                return;
            }
            let state = state.clone();
            std::thread::spawn(move || {
                // 下发 + 回读确认都在 set_mode_pref 里（模组异步生效，可能占串口几秒）
                let msg = match modem.set_mode_pref(&value) {
                    Ok(pref) => format!("已切换到 {}", pref.value_text),
                    Err(e) => format!("设置失败：{e}"),
                };
                super::push::push_mode(&state, &modem, msg);
            });
        }
        "sms_send" => {
            let Some(modem) = modem_of(state, cmd.modem_id) else {
                return;
            };
            let number = cmd.number.trim().to_string();
            let text = cmd.text.clone();
            let state = state.clone();
            std::thread::spawn(move || {
                let err = modem.send_sms(&number, &text).err().map(|e| e.to_string());
                super::push::push_sendsms(&state, &modem, &number, &text, err);
            });
        }
        "net_get" => {
            // 屏端是 Passive（不算"有人在看"），没人开网页时网络缓存是冷的，
            // 运营商名会一直空着 ⇒ 屏幕得自己开口要一次。
            let Some(modem) = modem_of(state, cmd.modem_id) else {
                return;
            };
            let state = state.clone();
            std::thread::spawn(move || {
                super::push::push_network(&state, &modem);
            });
        }
        _ => {}
    }
}

/// 客户端 → 服务端的指令（快照 / 短信已读 / 删短信 / 组网模式 / 发短信 / 网络状态）。
///
/// 前端不需要用 WS 触发读取（手动刷新走 HTTP，完成后由推送同步），这里只留
/// **轻量指令**：它们要么改状态（标已读/删除），要么屏端自己要数据（`mode_get`
/// / `net_get`），要么必须异步（发短信 / 切模式）。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct WsCommand {
    action: String,
    modem_id: u32,
    /// 短信的**存储位置**（不是列表下标——列表按时间倒序排）
    index: String,
    /// mode_set 的取值（语义档位 LTE / NR5G / LTE:NR5G）
    value: String,
    /// sms_send 的目标号码与正文
    number: String,
    text: String,
}

/// 取指令的目标模组（不存在就静默忽略：屏端可能还在连老 id）。
fn modem_of(state: &AppState, id: u32) -> Option<Arc<crate::modem::model::Modem>> {
    state.mgr.get(id)
}

/// 屏端给的短信存储位置（只认非负整数；空 / 带空格等都当无效）。
fn parse_index(raw: &str) -> Option<u32> {
    let t = raw.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

/// 重读短信并推送（列表带正文 → 过密码闸门；角标只有数字 → 人人可收）。
fn refresh_and_push_sms(state: &AppState, modem: &crate::modem::model::Modem) {
    match modem.read_sms() {
        Ok(list) => super::push::push_sms(state, modem, &list),
        Err(e) => log::warn!("重读短信失败（模组 {}）: {e}", modem.id),
    }
    super::push::push_smscount(state, modem);
}

fn snapshot(client: &Arc<WsClient>, state: &Arc<AppState>) {
    let modems = serde_json::to_value(state.mgr.list()).unwrap_or(Value::Null);
    let _ = client.send_event("modems", None, modems);
}

/// 「跟随 LuCI 登录」在 WS 侧的同款闸门。
fn authorized(
    state: &AppState,
    headers: &HashMap<String, String>,
    query: &HashMap<String, String>,
    loopback: bool,
) -> bool {
    if !state.cfg.require_luci_login() {
        return true;
    }
    if loopback {
        return true;
    }
    let sid = cookie(headers, auth::SESSION_COOKIE).or_else(|| query.get("luci_sid").cloned());
    match sid {
        None => false,
        Some(sid) => match state.ubus.session_valid(&sid) {
            Ok(valid) => valid,
            Err(e) => {
                log::warn!("ubus 会话校验不可用，放行本次 WebSocket 连接: {e}");
                true
            }
        },
    }
}

fn deny(mut stream: TcpStream) -> Result<()> {
    let _ = stream
        .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    Ok(())
}

/// `Sec-WebSocket-Accept` = base64(sha1(key + GUID))。
fn accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(GUID.as_bytes());
    STANDARD.encode(hasher.finalize())
}

/// 读一帧；返回 `Ok(None)` 表示对端正常关闭。
fn read_frame(stream: &mut TcpStream) -> Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0u8; 2];
    match stream.read_exact(&mut head) {
        Ok(()) => {}
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
            ) =>
        {
            return Ok(None)
        }
        Err(e) => return Err(msg(format!("读取 WebSocket 帧失败: {e}"))),
    }

    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let mut len = (head[1] & 0x7f) as usize;
    match len {
        126 => {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext)?;
            len = u16::from_be_bytes(ext) as usize;
        }
        127 => {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext)?;
            len = u64::from_be_bytes(ext) as usize;
        }
        _ => {}
    }
    if len > MAX_FRAME {
        return Err(msg("WebSocket 帧超出上限"));
    }

    let mut mask = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload)?;
    }
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i & 3];
        }
    }
    Ok(Some((opcode, payload)))
}

fn write_frame<W: Write>(writer: &mut W, opcode: u8, payload: &[u8]) -> Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | (opcode & 0x0f));
    let len = payload.len();
    if len < 126 {
        header.push(len as u8);
    } else if len <= u16::MAX as usize {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

/// 拆出请求行目标与（小写化的）头部表。
fn parse_head(head: &str) -> (String, HashMap<String, String>) {
    let mut lines = head.split("\r\n");
    let target = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    (target, headers)
}

fn header(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers.get(name).cloned()
}

/// 从头部表里取 Cookie。
fn cookie(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let raw = headers.get("cookie")?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&needle) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{accept_key, parse_index, WsCommand, WsHub};
    use serde_json::json;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn rfc6455_accept_key() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    /// 建一对互联的 TCP，服务端那侧注册成客户端（返回客户端侧供测试读）。
    fn pair(hub: &WsHub, token: Option<&str>) -> TcpStream {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let server = listener.accept().unwrap().0;
        let _: Arc<super::WsClient> = hub.register(server, token.map(str::to_string), false);
        client
    }

    /// 从客户端侧读一帧；超时/没数据都算「没收到」。
    fn read_some(c: &mut TcpStream) -> Option<String> {
        c.set_read_timeout(Some(Duration::from_millis(400)))
            .unwrap();
        let mut buf = [0u8; 1024];
        match c.read(&mut buf) {
            Ok(n) if n > 0 => Some(String::from_utf8_lossy(&buf[..n]).to_string()),
            _ => None,
        }
    }

    #[test]
    fn broadcast_sms_gate_filters_by_token() {
        let hub = WsHub::new();
        let mut good = pair(&hub, Some("tok-ok"));
        let mut bad = pair(&hub, Some("tok-bad"));
        let mut none = pair(&hub, None);

        hub.broadcast_sms("sms", Some(0), json!([{"index": 7}]), |c| {
            c.sms_token.as_deref() == Some("tok-ok")
        });

        // 闸门放行的那条：收到带正文的 sms
        let got = read_some(&mut good).expect("放行的连接该收到 sms");
        assert!(got.contains("\"type\":\"sms\""), "帧内容不对: {got}");
        assert!(got.contains("\"index\":7"), "帧里没有正文: {got}");

        // 令牌不对的、没带令牌的：一个字都不该收到
        assert!(read_some(&mut bad).is_none(), "令牌不对不该收到");
        assert!(read_some(&mut none).is_none(), "没带令牌不该收到");
    }

    #[test]
    fn ungated_broadcast_reaches_everyone() {
        // smscount 只有 {total,unread}、不含正文，走普通广播：人人可收
        let hub = WsHub::new();
        let mut good = pair(&hub, Some("tok-ok"));
        let mut bad = pair(&hub, Some("tok-bad"));
        let mut none = pair(&hub, None);

        hub.broadcast("smscount", Some(0), json!({"total": 3, "unread": 1}));

        for (c, tag) in [
            (&mut good, "带对令牌"),
            (&mut bad, "带错令牌"),
            (&mut none, "无令牌"),
        ] {
            let got = read_some(c).unwrap_or_else(|| panic!("{tag} 也该收到 smscount"));
            assert!(
                got.contains("\"type\":\"smscount\""),
                "{tag} 帧内容不对: {got}"
            );
            assert!(got.contains("\"unread\":1"), "{tag} 帧里没有角标: {got}");
        }
    }

    #[test]
    fn broken_client_is_dropped_not_fatal() {
        // 写失败的连接不能带崩广播（对端已关时 send 会报错，循环要继续）
        let hub = WsHub::new();
        let dead = pair(&hub, Some("tok-ok"));
        drop(dead); // 对端直接关掉
        let mut alive = pair(&hub, Some("tok-ok"));
        hub.broadcast_sms("sms", None, json!([{"index": 1}]), |_| true);
        let got = read_some(&mut alive).expect("活着的连接该照常收到");
        assert!(got.contains("\"type\":\"sms\""));
    }

    #[test]
    fn ws_command_parses_and_defaults() {
        let c: WsCommand =
            serde_json::from_str(r#"{"action":"mode_set","modem_id":2,"value":"LTE"}"#).unwrap();
        assert_eq!(c.action, "mode_set");
        assert_eq!(c.modem_id, 2);
        assert_eq!(c.value, "LTE");
        // 没给的字段按零值（屏端只发它关心的那几个）
        assert_eq!(c.index, "");
        assert_eq!(c.number, "");
        assert_eq!(c.text, "");

        let c: WsCommand = serde_json::from_str(r#"{"action":"snapshot"}"#).unwrap();
        assert_eq!(c.action, "snapshot");
        assert_eq!(c.modem_id, 0);

        // index 是**字符串**（屏端按存储位置给，与 SMS.Index 同型）
        let c: WsCommand =
            serde_json::from_str(r#"{"action":"sms_read","modem_id":0,"index":"3"}"#).unwrap();
        assert_eq!(c.index, "3");
        let c: WsCommand = serde_json::from_str(
            r#"{"action":"sms_send","modem_id":1,"number":"10086","text":"CXHF"}"#,
        )
        .unwrap();
        assert_eq!(c.number, "10086");
        assert_eq!(c.text, "CXHF");
        // 乱七八糟的帧不该 panic（handle_text 里解析失败直接 return）
        assert!(serde_json::from_str::<WsCommand>("not json").is_err());
    }

    #[test]
    fn sms_index_must_be_a_plain_number() {
        assert_eq!(parse_index("3"), Some(3));
        assert_eq!(parse_index(" 12 "), Some(12));
        assert_eq!(parse_index(""), None);
        assert_eq!(parse_index("abc"), None);
        assert_eq!(parse_index("-1"), None);
        assert_eq!(parse_index("3 4"), None);
    }
}
