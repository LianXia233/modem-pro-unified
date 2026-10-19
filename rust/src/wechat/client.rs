//! 运行期状态：登录态、学到的推送目标、已推过的短信、以及真正的「发」。
//!
//! 这一层把 `mod.rs` 的纯逻辑与 `transport.rs` 的 curl 接起来，
//! 由 `http/handlers/wechat.rs` 与 `http/loops.rs`（`+CMTI` 触发）调用。
//!
//! ## 三个「别过度设计」的决定
//!
//! 1. **运行期覆盖存在内存里，不热加载 config.toml**：改配置时由 handler
//!    写盘（`config/patch.rs`）+ 覆盖内存，两个都做。不引入文件监听 ——
//!    一个 15 秒一轮的进程不值得为它加一套 inotify。
//! 2. **已推过的短信按 `index|timestamp|sender` 记**：`+CMTI` 只告诉你有新的，
//!    不告诉你是哪条；而 `read_sms()` 回的是整张表。靠这个集合做增量，
//!    上限 [`PUSHED_MAX`] 条，满了丢最旧的（短信本身在模组里，丢了只是
//!    可能重推一次，比内存无上限增长好）。
//! 3. **推送节流 [`PUSH_MIN_GAP`]**：群发短信时几十条一起到，一条一条发 curl
//!    会把进程线程占满（同步 curl，会阻塞调用方）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::Serialize;

use crate::config::WeChatConfig;
use crate::error::{msg, Result};
use crate::modem::model::Sms;

use super::transport;
use super::{
    api_url, effective_template, login_from_config, pick_target, render_template,
    should_learn_sender, IncomingMessage, LoginState, SmsVars,
};

/// 每个模组最多记住多少条「已推过」（防内存无上限增长）。
const PUSHED_MAX: usize = 200;

/// 两次推送的最小间隔：群发时把一批短信合并成一次节奏，别把线程占满。
pub const PUSH_MIN_GAP: Duration = Duration::from_secs(3);

/// curl 超时（秒）：HTTPS 到公网，给足但别挂死调用方线程。
const HTTP_TIMEOUT: u64 = 15;

/// 长轮询超时（秒）：微信那侧挂着等消息，要远大于普通请求。
const POLL_TIMEOUT: u64 = 60;

/// 运行期可覆盖的配置字段（`/api/wechat/set` 改的就是这些）。
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub enabled: Option<bool>,
    pub to_user_id: Option<String>,
    pub template: Option<String>,
    pub listen: Option<bool>,
    pub allowed_senders: Option<String>,
}

/// 给页面看的视图（`GET /api/wechat`）。
#[derive(Debug, Clone, Serialize)]
pub struct WeChatView {
    pub enabled: bool,
    pub logged_in: bool,
    /// 登录阶段：idle / wait / ready / expired（与页面 `stage` 分支同名）
    pub stage: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stage_cn: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stage_msg: String,
    /// 二维码（离线渲染成 SVG data URI；页面直接放 `<img src>`）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub qr_png: String,
    /// 二维码内容（排查/手工打开用）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub qr_url: String,
    /// 二维码剩余秒数
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub qr_left: u64,
    /// 需要配对码（当前登录流程没有这一阶段，恒 false）
    pub verify_required: bool,
    /// 旧字段：登录态短名（保留给 /api/wechat 的老调用方）
    pub state: &'static str,
    pub to_user_id: String,
    /// 生效的推送目标（显式配置优先，否则是学到的发送者）。
    pub effective_target: Option<String>,
    /// 最近给机器人发过消息的人
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_sender: String,
    /// 能不能主动发消息：真的收到过对方消息（或扫码用户已就绪）才算
    pub session_ready: bool,
    /// 收到过对方消息
    pub heard_message: bool,
    /// 有会话上下文令牌（当前实现不维护，恒 false）
    pub has_context: bool,
    /// 长轮询监听是否开着
    pub listen_on: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_incoming: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_incoming_at: Option<String>,
    pub template: String,
    pub listen: bool,
    pub allowed_senders: String,
    pub bot_id: String,
    pub bot_user_id: String,
    /// 掩码后的 token（**不回传完整 token**）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token_mask: String,
    /// 有没有 token
    pub has_token: bool,
    /// curl 是否可用（不可用时推送必然失败，页面上该说清楚）。
    pub transport_ok: bool,
    /// 最近一次发送记录
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_send: Option<SendRecord>,
    pub sent_ok: u64,
    pub sent_fail: u64,
    /// 默认模板（页面「恢复默认」按钮用）
    pub default_template: String,
}

/// 最近一次发送记录（成功/失败都记）。
#[derive(Debug, Clone, Serialize)]
pub struct SendRecord {
    pub at: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub msg: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub test: bool,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

/// token 掩码：只露首尾 4 位，中间省略（面板不泄露完整凭据）。
fn mask_token(t: &str) -> String {
    let t = t.trim();
    let n = t.chars().count();
    if n == 0 {
        return String::new();
    }
    if n <= 8 {
        return "••••".to_string();
    }
    let head: String = t.chars().take(4).collect();
    let tail: String = t.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// 二维码内容 → SVG data URI。
///
/// 为什么是 SVG 而不是 PNG：`qrcode` crate 不开 `image` 特性时零额外依赖，
/// 手写一小段 `<path>` 就能出图；`<img src="data:image/svg+xml;base64,…">`
/// 浏览器照样渲染，固件体积也不用背一个图像库。
fn qr_data_uri(content: &str) -> String {
    if content.trim().is_empty() {
        return String::new();
    }
    let Ok(code) = qrcode::QrCode::new(content.as_bytes()) else {
        return String::new();
    };
    let width = code.width();
    let colors = code.to_colors();
    let mut path = String::new();
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let x = i % width;
            let y = i / width;
            path.push_str(&format!("M{x} {y}h1v1h-1z"));
        }
    }
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} {width}\" shape-rendering=\"crispEdges\"><rect width=\"{width}\" height=\"{width}\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>"
    );
    format!("data:image/svg+xml;base64,{}", B64.encode(svg))
}

/// 企业微信客户端（一个进程一个，挂在 `AppState` 上）。
pub struct WeChatClient {
    /// 启动时的配置快照（base_url / bot_token 这类不在页面上改的）。
    cfg: WeChatConfig,
    login: Mutex<LoginState>,
    /// 学到的「最近给机器人发过消息的人」（`listen` 开着时才有）。
    last_sender: Mutex<String>,
    ov: Mutex<Overrides>,
    /// 每个模组已推过的短信键（见模块头决定 2）。
    pushed: Mutex<HashMap<u32, Vec<String>>>,
    /// 上次推送时刻（节流）。
    last_push: Mutex<Option<Instant>>,
    /// 最近一次收到对方消息（发送者 + 时刻）
    last_incoming: Mutex<Option<(String, String)>>,
    /// 最近一次发送记录
    last_send: Mutex<Option<SendRecord>>,
    /// 推送成功 / 失败计数
    sent_ok: AtomicU64,
    sent_fail: AtomicU64,
}

impl WeChatClient {
    /// 用配置构造（已有 token ⇒ 直接是已登录态）。
    pub fn new(cfg: &WeChatConfig) -> Self {
        Self {
            cfg: cfg.clone(),
            login: Mutex::new(login_from_config(
                &cfg.bot_token,
                &cfg.bot_id,
                &cfg.bot_user_id,
            )),
            last_sender: Mutex::new(String::new()),
            ov: Mutex::new(Overrides::default()),
            pushed: Mutex::new(HashMap::new()),
            last_push: Mutex::new(None),
            last_incoming: Mutex::new(None),
            last_send: Mutex::new(None),
            sent_ok: AtomicU64::new(0),
            sent_fail: AtomicU64::new(0),
        }
    }

    // ---------- 配置读取（运行期覆盖优先） ----------

    fn enabled(&self) -> bool {
        self.ov.lock().unwrap().enabled.unwrap_or(self.cfg.enabled)
    }

    fn listen(&self) -> bool {
        self.ov.lock().unwrap().listen.unwrap_or(self.cfg.listen)
    }

    fn to_user_id(&self) -> String {
        self.ov
            .lock()
            .unwrap()
            .to_user_id
            .clone()
            .unwrap_or_else(|| self.cfg.to_user_id.clone())
    }

    fn template(&self) -> String {
        effective_template(
            self.ov
                .lock()
                .unwrap()
                .template
                .as_deref()
                .unwrap_or(&self.cfg.template),
        )
    }

    fn allowed_senders(&self) -> String {
        self.ov
            .lock()
            .unwrap()
            .allowed_senders
            .clone()
            .unwrap_or_else(|| self.cfg.allowed_senders.clone())
    }

    /// 当前生效的 base_url（页面上不显示，但 handler 拼地址要用）。
    pub fn base_url(&self) -> String {
        if self.cfg.base_url.trim().is_empty() {
            "https://ilinkai.weixin.qq.com".to_string()
        } else {
            self.cfg.base_url.clone()
        }
    }

    /// 学到的「最近给机器人发过消息的人」（空串 = 还没学到）。
    pub fn learned_sender(&self) -> String {
        self.last_sender.lock().unwrap().clone()
    }

    // ---------- 登录态 ----------

    /// 当前登录态（克隆）。
    pub fn login_state(&self) -> LoginState {
        self.login.lock().unwrap().clone()
    }

    /// 记下二维码（等扫码）。
    pub fn set_qr_pending(&self, url: &str, expires_at: i64) {
        *self.login.lock().unwrap() = LoginState::QrPending {
            url: url.to_string(),
            expires_at,
        };
    }

    /// 登录成功（凭据同时写回配置，见 handler）。
    pub fn set_logged_in(&self, token: &str, bot_id: &str, bot_user_id: &str) {
        *self.login.lock().unwrap() = LoginState::LoggedIn {
            bot_token: token.to_string(),
            bot_id: bot_id.to_string(),
            bot_user_id: bot_user_id.to_string(),
        };
    }

    /// 登出（清内存态；配置由 handler 写盘）。
    pub fn logout(&self) {
        *self.login.lock().unwrap() = LoginState::LoggedOut;
    }

    // ---------- 视图 ----------

    /// 给页面的视图。
    pub fn view(&self) -> WeChatView {
        let st = self.login_state();
        let now = crate::logging::unix_now();
        let (stage, stage_cn, stage_msg, qr_url, qr_left) = match &st {
            LoginState::LoggedIn { .. } => (
                "ready",
                "已登录",
                "已登录：收到新短信会按模板推到微信。",
                String::new(),
                0,
            ),
            LoginState::QrPending { url, expires_at } => {
                let left = (*expires_at - now).max(0) as u64;
                if left == 0 {
                    (
                        "expired",
                        "二维码已失效",
                        "二维码已过期：点「重新生成二维码」再来一次。",
                        url.clone(),
                        0,
                    )
                } else {
                    (
                        "wait",
                        "等待扫码",
                        "用手机微信扫码，并在手机上点「确认」。",
                        url.clone(),
                        left,
                    )
                }
            }
            LoginState::LoggedOut => (
                "idle",
                "未登录",
                "还没登录：先扫码登录，然后在微信里给机器人发一条消息。",
                String::new(),
                0,
            ),
        };
        let last_sender = self.learned_sender();
        let heard = !last_sender.is_empty();
        let (last_incoming, last_incoming_at) = self
            .last_incoming
            .lock()
            .unwrap()
            .clone()
            .map(|(s, at)| (s, Some(at)))
            .unwrap_or_default();
        WeChatView {
            enabled: self.enabled(),
            logged_in: st.can_push(),
            stage,
            stage_cn: stage_cn.to_string(),
            stage_msg: stage_msg.to_string(),
            qr_png: qr_data_uri(&qr_url),
            qr_url,
            qr_left,
            verify_required: false,
            state: st.label(),
            to_user_id: self.to_user_id(),
            effective_target: pick_target(&self.to_user_id(), &last_sender),
            last_sender: last_sender.clone(),
            session_ready: st.can_push() && heard,
            heard_message: heard,
            has_context: false,
            listen_on: self.listen(),
            last_incoming,
            last_incoming_at,
            template: self.template(),
            listen: self.listen(),
            allowed_senders: self.allowed_senders(),
            bot_id: self.cfg.bot_id.clone(),
            bot_user_id: self.cfg.bot_user_id.clone(),
            token_mask: mask_token(&self.cfg.bot_token),
            has_token: st.can_push(),
            transport_ok: transport::available(),
            last_send: self.last_send.lock().unwrap().clone(),
            sent_ok: self.sent_ok.load(Ordering::Relaxed),
            sent_fail: self.sent_fail.load(Ordering::Relaxed),
            default_template: super::DEFAULT_TEMPLATE.to_string(),
        }
    }

    /// 把最近一次发送标记为「测试消息」（`/api/wechat/test` 用）。
    pub fn mark_last_test(&self) {
        if let Some(r) = self.last_send.lock().unwrap().as_mut() {
            r.test = true;
        }
    }

    // ---------- 收消息（学推送目标） ----------

    /// 采纳一条收到的消息（过了 [`sender_allowed`] 才学）。
    ///
    /// 返回是否采纳。学到一个被过滤掉的发送者，等于把后续短信推给他 ——
    /// 那比不学更糟，所以过滤在这里做而不是在发送时做。
    pub fn learn(&self, msg: &IncomingMessage) -> bool {
        if !should_learn_sender(msg, &self.allowed_senders()) {
            return false;
        }
        let sender = msg.sender.trim().to_string();
        *self.last_sender.lock().unwrap() = sender.clone();
        *self.last_incoming.lock().unwrap() = Some((sender, crate::logging::timestamp()));
        true
    }

    /// 长轮询一次（`listen` 开着时才该调）。
    ///
    /// 拿到消息就学发送者；失败只返回错误，由调用方决定记不记日志。
    pub fn poll_once(&self) -> Result<Vec<IncomingMessage>> {
        let url = api_url(&self.base_url(), super::api::POLL_PATH);
        let body = transport::get(&url, POLL_TIMEOUT)?;
        parse_incoming(&body)
    }

    // ---------- 发消息 ----------

    /// 把**新到的**短信推出去，返回推了几条。
    ///
    /// 调用方是 `http/loops.rs` 的 `+CMTI` 路径（同步线程里跑 curl，
    /// 所以有节流）。任何一步不满足（没开 / 没登录 / 没有目标）都安静返回 0 ——
    /// 没配微信不是错误，不该刷日志。
    pub fn push_new_sms(&self, modem_id: u32, list: &[Sms]) -> usize {
        if !self.enabled() || list.is_empty() {
            return 0;
        }
        let Some(target) = pick_target(&self.to_user_id(), &self.learned_sender()) else {
            log::debug!("企业微信：没有推送目标（to_user_id 空且没人给机器人发过消息）");
            return 0;
        };
        if !self.login_state().can_push() {
            log::debug!("企业微信：未登录，跳过短信推送");
            return 0;
        }
        // 增量：只推没推过的
        let mut fresh: Vec<&Sms> = Vec::new();
        {
            let mut pushed = self.pushed.lock().unwrap();
            let seen = pushed.entry(modem_id).or_default();
            for s in list {
                let key = sms_key(s);
                if !seen.iter().any(|k| k == &key) {
                    fresh.push(s);
                }
            }
            for s in &fresh {
                let key = sms_key(s);
                if seen.len() >= PUSHED_MAX {
                    seen.remove(0);
                }
                seen.push(key);
            }
        }
        if fresh.is_empty() {
            return 0;
        }
        // 节流
        {
            let mut last = self.last_push.lock().unwrap();
            let now = Instant::now();
            if let Some(t) = *last {
                if now.saturating_duration_since(t) < PUSH_MIN_GAP {
                    // 不推，但**不回滚**已记的键 —— 下一条新短信到来时
                    // 这一批会被跳过。宁可少推，也不要并发发一堆 curl。
                    log::debug!("企业微信：{} 条新短信撞上推送节流，本轮不发", fresh.len());
                    return 0;
                }
            }
            *last = Some(now);
        }
        let tpl = self.template();
        let label = format!("模组 {modem_id}");
        let mut sent = 0usize;
        for s in &fresh {
            let vars = SmsVars {
                modem: label.clone(),
                number: s.number.clone(),
                time: s.timestamp.clone(),
                content: s.content.clone(),
                total: 1,
                index: 1,
            };
            let text = render_template(&tpl, &vars);
            match self.send_text(&target, &text) {
                Ok(()) => sent += 1,
                Err(e) => log::warn!("企业微信推送失败（模组 {modem_id}）: {e}"),
            }
        }
        if sent > 0 {
            log::info!("企业微信：已推送 {} 条新短信（模组 {modem_id}）", sent);
        }
        sent
    }

    /// 发一条纯文本（`/api/wechat/test` 与上面的推送共用）。
    ///
    /// 成功/失败都会更新面板上的计数与「最近一次发送」—— 页面那两格
    /// （`sent_ok` / `sent_fail` / `last_send`）就是靠它。
    pub fn send_text(&self, target: &str, text: &str) -> Result<()> {
        let r = self.send_text_inner(target, text);
        match &r {
            Ok(()) => self.sent_ok.fetch_add(1, Ordering::Relaxed),
            Err(_) => self.sent_fail.fetch_add(1, Ordering::Relaxed),
        };
        *self.last_send.lock().unwrap() = Some(SendRecord {
            at: crate::logging::timestamp(),
            ok: r.is_ok(),
            msg: match &r {
                Ok(()) => String::new(),
                Err(e) => e.to_string(),
            },
            test: false,
        });
        r
    }

    fn send_text_inner(&self, target: &str, text: &str) -> Result<()> {
        let url = api_url(&self.base_url(), super::api::SEND_PATH);
        let body = serde_json::json!({
            "bot_token": self.token(),
            "to_user_id": target,
            "msgtype": "text",
            "text": { "content": text },
        })
        .to_string();
        let resp = transport::post_json(&url, Some(&body), HTTP_TIMEOUT)?;
        if resp.is_empty() {
            return Err(msg("企业微信返回空响应"));
        }
        // 业务错误：curl 退出码是 0（HTTP 层成功），错在返回体里
        let v: serde_json::Value = serde_json::from_str(&resp)
            .map_err(|e| msg(format!("企业微信返回不是 JSON: {e}（{resp}）")))?;
        let code = v.get("errcode").and_then(|c| c.as_i64()).unwrap_or(0);
        if code != 0 {
            let err = v
                .get("errmsg")
                .and_then(|m| m.as_str())
                .unwrap_or("未知错误");
            return Err(msg(format!("企业微信 errcode={code}: {err}")));
        }
        Ok(())
    }

    fn token(&self) -> String {
        match self.login_state() {
            LoginState::LoggedIn { bot_token, .. } => bot_token,
            _ => String::new(),
        }
    }

    // ---------- 改配置 ----------

    /// 应用 `/api/wechat/set`（内存覆盖 + 返回要写盘的键值对）。
    ///
    /// 写盘由 handler 做（它才知道 `config_path`）；这里只决定**该写什么**，
    /// 顺带做校验（纯逻辑部分可单测）。
    pub fn apply_set(&self, params: &[(String, String)]) -> Result<Vec<(String, String)>> {
        let mut ov = self.ov.lock().unwrap();
        let mut writes: Vec<(String, String)> = Vec::new();
        for (k, v) in params {
            match k.as_str() {
                "enabled" => {
                    let b = v == "1" || v == "true" || v == "on";
                    ov.enabled = Some(b);
                    writes.push(("enabled".into(), b.to_string()));
                }
                "listen" => {
                    let b = v == "1" || v == "true" || v == "on";
                    ov.listen = Some(b);
                    writes.push(("listen".into(), b.to_string()));
                }
                "to_user_id" => {
                    ov.to_user_id = Some(v.clone());
                    writes.push(("to_user_id".into(), v.clone()));
                }
                "template" => {
                    let t = if v.trim().is_empty() {
                        super::DEFAULT_TEMPLATE.to_string()
                    } else {
                        v.clone()
                    };
                    ov.template = Some(t.clone());
                    writes.push(("template".into(), t));
                }
                "allowed_senders" => {
                    ov.allowed_senders = Some(v.clone());
                    writes.push(("allowed_senders".into(), v.clone()));
                }
                // 未知键忽略：页面上多带一个字段不该让整个保存失败
                _ => {}
            }
        }
        Ok(writes)
    }
}

/// 一条短信的增量键（`index|timestamp|sender`）。
///
/// 为什么带 timestamp：模组重启 / 换卡后 index 会从 1 重新开始，
/// 只按 index 去重会把新卡的第一条当成已推过。
pub fn sms_key(s: &Sms) -> String {
    format!("{}|{}|{}", s.index, s.timestamp, s.number)
}

/// 解析长轮询的返回（`{list:[{sender,content,time}]}` 或直接是数组）。
fn parse_incoming(body: &str) -> Result<Vec<IncomingMessage>> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| msg(format!("企业微信长轮询返回不是 JSON: {e}")))?;
    let arr = v
        .get("list")
        .or_else(|| v.get("messages"))
        .or_else(|| v.get("data"))
        .unwrap_or(&v);
    serde_json::from_value::<Vec<IncomingMessage>>(arr.clone())
        .map_err(|e| msg(format!("企业微信长轮询返回结构不对: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> WeChatConfig {
        WeChatConfig {
            base_url: "https://ilinkai.weixin.qq.com".into(),
            bot_token: "tok".into(),
            bot_id: "bid".into(),
            bot_user_id: "buid".into(),
            ..WeChatConfig::default()
        }
    }

    fn sms(index: &str, number: &str) -> Sms {
        Sms {
            index: index.to_string(),
            number: number.to_string(),
            timestamp: "2026-10-18 12:00".into(),
            content: "您好".into(),
            ..Sms::default()
        }
    }

    #[test]
    fn new_client_uses_config_login() {
        let c = WeChatClient::new(&cfg());
        assert!(c.login_state().can_push());
        assert_eq!(c.view().state, "已登录");
    }

    #[test]
    fn new_client_without_token_is_logged_out() {
        let c = WeChatClient::new(&WeChatConfig::default());
        assert!(!c.login_state().can_push());
        assert_eq!(c.view().state, "未登录");
    }

    #[test]
    fn view_never_leaks_token() {
        // 用一个不可能撞上字段名的值：`has_token` 这种键名本身就含 "tok"
        let mut c = cfg();
        c.bot_token = "SECRET-TOKEN-9f3c1a7b".into();
        let cli = WeChatClient::new(&c);
        let v = cli.view();
        let text = serde_json::to_string(&v).unwrap();
        assert!(!text.contains("SECRET-TOKEN"), "token 不能出现在视图里");
        assert!(v.has_token);
    }

    #[test]
    fn login_transitions() {
        let c = WeChatClient::new(&WeChatConfig::default());
        c.set_qr_pending("https://qr", 100);
        assert_eq!(c.login_state().label(), "等待扫码");
        assert!(!c.login_state().can_push());
        c.set_logged_in("t2", "b", "u");
        assert!(c.login_state().can_push());
        c.logout();
        assert_eq!(c.login_state(), LoginState::LoggedOut);
    }

    #[test]
    fn learn_filters_by_allowlist() {
        let mut c = cfg();
        c.allowed_senders = "zhangsan".into();
        let cli = WeChatClient::new(&c);
        assert!(cli.learn(&IncomingMessage {
            sender: "zhangsan@im.wechat".into(),
            content: "在吗".into(),
            time: 1,
        }));
        // 没配 allowed_senders 前先确认它真的在过滤
        assert_eq!(cli.view().effective_target.unwrap(), "zhangsan@im.wechat");

        assert!(!cli.learn(&IncomingMessage {
            sender: "lisi@im.wechat".into(),
            content: "在吗".into(),
            time: 2,
        }));
        // 被过滤的不该覆盖已学到的人
        assert_eq!(cli.view().effective_target.unwrap(), "zhangsan@im.wechat");
    }

    #[test]
    fn learn_without_allowlist_accepts_anyone() {
        // allowed_senders 留空 = 不限制
        let cli = WeChatClient::new(&cfg());
        assert!(cli.learn(&IncomingMessage {
            sender: "anyone@im.wechat".into(),
            content: "hi".into(),
            time: 1,
        }));
        assert_eq!(cli.view().effective_target.unwrap(), "anyone@im.wechat");
    }

    #[test]
    fn sms_key_includes_timestamp() {
        let a = sms("1", "10086");
        let mut b = sms("1", "10086");
        b.timestamp = "2026-10-19 08:00".into();
        assert_ne!(sms_key(&a), sms_key(&b), "同 index 不同时间算两条");
        assert_eq!(sms_key(&a), sms_key(&sms("1", "10086")));
    }

    #[test]
    fn apply_set_covers_all_editable_fields() {
        let c = WeChatClient::new(&WeChatConfig::default());
        let params: Vec<(String, String)> = vec![
            ("enabled".into(), "1".into()),
            ("listen".into(), "0".into()),
            ("to_user_id".into(), "boss@im.wechat".into()),
            ("template".into(), "".into()),
            ("allowed_senders".into(), "zhangsan".into()),
            ("unknown_field".into(), "x".into()),
        ];
        let writes = c.apply_set(&params).unwrap();
        let keys: Vec<&str> = writes.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "enabled",
                "listen",
                "to_user_id",
                "template",
                "allowed_senders"
            ],
            "未知键不该写盘"
        );
        // 空 template 回落默认值
        let tpl = writes.iter().find(|(k, _)| k == "template").unwrap();
        assert_eq!(tpl.1, super::super::DEFAULT_TEMPLATE);
        let v = c.view();
        assert!(v.enabled);
        assert!(!v.listen);
        assert_eq!(v.to_user_id, "boss@im.wechat");
        assert_eq!(v.allowed_senders, "zhangsan");
    }

    #[test]
    fn apply_set_accepts_bool_words() {
        let c = WeChatClient::new(&WeChatConfig::default());
        for on in ["1", "true", "on"] {
            let w = c.apply_set(&[("enabled".into(), on.into())]).unwrap();
            assert_eq!(w[0].1, "true");
        }
        let w = c.apply_set(&[("enabled".into(), "0".into())]).unwrap();
        assert_eq!(w[0].1, "false");
    }

    #[test]
    fn push_new_sms_noop_when_disabled_or_no_target() {
        let c = WeChatClient::new(&WeChatConfig::default());
        // 默认 enabled=false ⇒ 一条都不推
        assert_eq!(c.push_new_sms(0, &[sms("1", "10086")]), 0);
    }

    #[test]
    fn push_new_sms_requires_login() {
        let mut c = cfg();
        c.enabled = true;
        c.to_user_id = "boss@im.wechat".into();
        c.bot_token = String::new(); // 未登录
        let cli = WeChatClient::new(&c);
        assert_eq!(cli.push_new_sms(0, &[sms("1", "10086")]), 0);
    }

    #[test]
    fn push_new_sms_requires_target() {
        let mut c = cfg();
        c.enabled = true;
        // to_user_id 空、没人发过消息 ⇒ 无处可推
        let cli = WeChatClient::new(&c);
        assert_eq!(cli.push_new_sms(0, &[sms("1", "10086")]), 0);
    }

    #[test]
    fn effective_target_reflects_learned_sender() {
        let mut c = cfg();
        c.enabled = true;
        let cli = WeChatClient::new(&c);
        assert_eq!(cli.view().effective_target, None);
        cli.learn(&IncomingMessage {
            sender: "me@im.wechat".into(),
            content: "hi".into(),
            time: 1,
        });
        assert_eq!(
            cli.view().effective_target,
            Some("me@im.wechat".to_string())
        );
    }

    #[test]
    fn parse_incoming_accepts_three_shapes() {
        let a = parse_incoming(r#"[{"sender":"a","content":"x","time":1}]"#).unwrap();
        assert_eq!(a.len(), 1);
        let b = parse_incoming(r#"{"list":[{"sender":"b","content":"y","time":2}]}"#).unwrap();
        assert_eq!(b[0].sender, "b");
        let c = parse_incoming(r#"{"messages":[{"sender":"c","content":"z","time":3}]}"#).unwrap();
        assert_eq!(c[0].sender, "c");
    }

    #[test]
    fn parse_incoming_rejects_garbage() {
        assert!(parse_incoming("not json").is_err());
        assert!(parse_incoming(r#"{"list":"nope"}"#).is_err());
    }

    #[test]
    fn sender_allowed_helper_is_reachable() {
        // 纯逻辑从 client 也能走到（handler 不直接引 mod.rs 的私有项）
        use crate::wechat::sender_allowed;
        assert!(sender_allowed("zhangsan@im.wechat", "zhangsan"));
    }
}
