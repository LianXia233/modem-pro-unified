//! 企业微信推送（`[wechat]` 段）：新短信转发到微信机器人。
//!
//! ## 为什么单独一个域
//!
//! 它能**改推送目标** —— 也就是说，拿到这个段的写权限就能把短信转发到任意微信，
//! 比短信本身还敏感。相关接口只受外层「跟随 LuCI 登录」闸门约束
//!（短信访问密码已于 2026-10-19 取消，`http/smsauth.rs` 已删除），
//! 且配置写回走 `config/patch.rs` 的行级原地替换（保留注释，不重排文件）。
//!
//! ## 分层
//!
//! | 文件 | 职责 | 单测 |
//! | --- | --- | --- |
//! | `mod.rs`（本文件） | **纯逻辑**：模板渲染、发送者过滤、推送目标选择、登录状态机、端点构造 | 全部可 |
//! | `transport.rs` | **薄 IO**：跑 `curl` 发 HTTPS（纯 std 没有 TLS，见下） | 不测 |
//! | `client.rs` | **运行期状态**：登录态 + 学到的推送目标 | 不测 |
//!
//! ## 为什么用 `curl` 而不是引 TLS 库
//!
//! `base_url` 是 `https://`，而 `std` 没有 TLS。两条路：
//!
//! - 引 `rustls` / `openssl`：跨交叉编译要另配一份根证书与链接参数，
//!   `Cargo.toml` 里「依赖刻意保持精简」的约定也就破了；
//! - **跑 `curl`**：OpenWrt 自带，支持 HTTPS，失败时退出码与 stderr 都能带出来。
//!
//! 选后者。代价是设备上必须有 `curl` —— 没有就明确报错（`transport::available()`），
//! 不假装成功。仓库里已有 `modprobe` 的先例（`modem/usbdev.rs`）。
//!
//! ## ⚠️ 未核实
//!
//! iLink Bot 的官方 API 路径**没有逐条对过 SDK 文档**（本项目是从配置注释与
//! 行为描述反推的）。路径集中在 [`api`] 一处的常量里，对得上的时候只改那一处；
//! 结构（拿二维码 → 轮询 → 拿 token → 发消息 / 收消息）是确定的。

pub mod client;
pub mod transport;

use serde::{Deserialize, Serialize};

/// iLink Bot 的 API 路径（集中在同一处，便于核对）。
///
/// ⚠️ 这些是**按官方 SDK 的调用形态推的**，没有逐条对过文档。真机联调时
/// 先在这里改，别散着改。
pub mod api {
    /// 拿登录二维码（扫码登录第一步）。
    pub const QR_PATH: &str = "/cgi-bin/get_login_qrcode";
    /// 轮询扫码结果（拿到 token 即登录成功）。
    pub const QR_CHECK_PATH: &str = "/cgi-bin/check_login";
    /// 发消息给机器人绑定的微信。
    pub const SEND_PATH: &str = "/cgi-bin/webhook/send";
    /// 长轮询「微信发给机器人的消息」（用来学推送目标）。
    pub const POLL_PATH: &str = "/cgi-bin/webhook/recv";
}

/// 模板占位符（`[wechat] template` 只认这几个）。
///
/// 认不出的占位符**原样留着**：模板写错时消息里会露出 `{typo}`，
/// 比静默变成空串好排查。
pub const PLACEHOLDERS: [&str; 5] = ["{modem}", "{number}", "{time}", "{content}", "{parts}"];

/// 一条待推送的短信（模板变量源）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SmsVars {
    /// 模组标识（页面上的那个，如 `模组 0` / IMEI 尾号）。
    pub modem: String,
    /// 对方号码。
    pub number: String,
    /// 接收时间（RFC3339 或页面格式，原样透传）。
    pub time: String,
    /// 短信正文。
    pub content: String,
    /// 长短信的总条数（`<= 1` 时 `{parts}` 渲染成空串）。
    pub total: usize,
    /// 长短信的第几条（从 1 起）。
    pub index: usize,
}

impl SmsVars {
    /// 单条短信的便捷构造（`{parts}` 渲染成空串）。
    pub fn single(modem: &str, number: &str, time: &str, content: &str) -> Self {
        Self {
            modem: modem.to_string(),
            number: number.to_string(),
            time: time.to_string(),
            content: content.to_string(),
            total: 1,
            index: 1,
        }
    }

    /// `{parts}` 的取值：单条为空串，长短信是「（第 x/y 条）」。
    ///
    /// 放在**后缀**位置（默认模板是 `📩 新短信{parts}\n…`），所以不带前导空格。
    pub fn parts_text(&self) -> String {
        parts_text(self.total, self.index)
    }
}

/// 长短信的后缀文本（纯函数，便于单测）。
///
/// `total <= 1` ⇒ 空串（单条短信不带任何后缀，别在标题里留个空括号）。
pub fn parts_text(total: usize, index: usize) -> String {
    if total <= 1 {
        return String::new();
    }
    format!("（第 {}/{} 条）", index.max(1), total)
}

/// 把模板里的占位符替换掉（认不出的原样留着）。
pub fn render_template(template: &str, vars: &SmsVars) -> String {
    let mut out = template.to_string();
    out = out.replace("{modem}", &vars.modem);
    out = out.replace("{number}", &vars.number);
    out = out.replace("{time}", &vars.time);
    out = out.replace("{content}", &vars.content);
    out = out.replace("{parts}", &vars.parts_text());
    out
}

/// 默认模板（`[wechat] template` 不写时用它）。
pub const DEFAULT_TEMPLATE: &str =
    "📩 新短信{parts}\n模组：{modem}\n号码：{number}\n时间：{time}\n内容：{content}";

/// 取生效的模板：配置里空串 ⇒ 用默认的。
pub fn effective_template(cfg_template: &str) -> String {
    let t = cfg_template.trim();
    if t.is_empty() {
        DEFAULT_TEMPLATE.to_string()
    } else {
        cfg_template.to_string()
    }
}

/// 发送者过滤（`[wechat] allowed_senders`，逗号分隔子串，留空 = 不限制）。
///
/// 为什么是**子串**而不是全等：配置注释写的是「只接受这些发送者（逗号分隔子串）」——
/// 同一个人可能有多个 id 形态（`xxx@im.wechat` / `xxx`），子串能一次覆盖。
/// 大小写不敏感：微信 id 实际都是小写，但配置里手写成大写不该让人收不到。
pub fn sender_allowed(sender: &str, allowed: &str) -> bool {
    let s = sender.trim();
    if s.is_empty() {
        return false; // 没署名的不收
    }
    let list = allowed.trim();
    if list.is_empty() {
        return true; // 不限制
    }
    let low = s.to_lowercase();
    list.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .any(|p| low.contains(&p.to_lowercase()))
}

/// 推送目标：`to_user_id` 显式指定 ⇒ 用它；否则用**学到的**最近发送者。
///
/// 两者都没有 ⇒ `None`（无处可推）。此时不该硬发 —— 发到一个猜出来的目标
/// 等于把短信转发给陌生人。
pub fn pick_target(to_user_id: &str, last_sender: &str) -> Option<String> {
    let t = to_user_id.trim();
    if !t.is_empty() {
        return Some(t.to_string());
    }
    let l = last_sender.trim();
    if l.is_empty() {
        None
    } else {
        Some(l.to_string())
    }
}

/// 登录状态机（扫码登录的三态）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LoginState {
    /// 未登录（初始态 / 登出后）。
    #[default]
    LoggedOut,
    /// 二维码已生成，等扫码。
    QrPending {
        /// 二维码内容（前端据此画码）。
        url: String,
        /// 过期时刻（unix 秒；过期后要重新拿）。
        expires_at: i64,
    },
    /// 已登录（凭据在 `[wechat]` 的 bot_token / bot_id / bot_user_id）。
    LoggedIn {
        bot_token: String,
        bot_id: String,
        bot_user_id: String,
    },
}

impl LoginState {
    /// 能不能发消息（有 token 才算）。
    pub fn can_push(&self) -> bool {
        matches!(self, LoginState::LoggedIn { bot_token, .. } if !bot_token.trim().is_empty())
    }

    /// 给页面看的一句话。
    pub fn label(&self) -> &'static str {
        match self {
            LoginState::LoggedOut => "未登录",
            LoginState::QrPending { .. } => "等待扫码",
            LoginState::LoggedIn { .. } => "已登录",
        }
    }
}

/// 二维码是否过期（`now` 是 unix 秒）。
pub fn qr_expired(state: &LoginState, now: i64) -> bool {
    match state {
        LoginState::QrPending { expires_at, .. } => now >= *expires_at,
        _ => false,
    }
}

/// 从配置推出初始登录态（配置里已有 token ⇒ 直接算已登录）。
pub fn login_from_config(token: &str, bot_id: &str, bot_user_id: &str) -> LoginState {
    if token.trim().is_empty() {
        LoginState::LoggedOut
    } else {
        LoginState::LoggedIn {
            bot_token: token.to_string(),
            bot_id: bot_id.to_string(),
            bot_user_id: bot_user_id.to_string(),
        }
    }
}

/// 拼一个完整的 API 地址（`base_url` 末尾的 `/` 与 `path` 前导的 `/` 只留一个）。
pub fn api_url(base_url: &str, path: &str) -> String {
    let b = base_url.trim().trim_end_matches('/');
    let p = path.trim_start_matches('/');
    if b.is_empty() {
        format!("/{p}")
    } else {
        format!("{b}/{p}")
    }
}

/// 一条收到的微信消息（长轮询的返回）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct IncomingMessage {
    /// 发送者 id（用来学推送目标）。
    #[serde(default)]
    pub sender: String,
    /// 消息正文。
    #[serde(default)]
    pub content: String,
    /// 时间戳（秒）。
    #[serde(default)]
    pub time: i64,
}

/// 一条收到的消息该不该采纳为「最近发送者」。
///
/// 两个条件：过了 [`sender_allowed`] 的过滤，且**非空**。
/// 学到一个被过滤掉的发送者等于把后续短信推给他 —— 那比不学更糟。
pub fn should_learn_sender(msg: &IncomingMessage, allowed: &str) -> bool {
    !msg.sender.trim().is_empty() && sender_allowed(&msg.sender, allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> SmsVars {
        SmsVars::single("模组 0", "10086", "2026-10-18 12:00:00", "您好")
    }

    // ---------- parts ----------

    #[test]
    fn parts_empty_for_single_sms() {
        // 单条短信不带任何后缀：默认模板是 `新短信{parts}\n`，
        // 留个空括号比什么都没有更难看
        assert_eq!(parts_text(1, 1), "");
        assert_eq!(parts_text(0, 0), "");
        assert!(SmsVars::single("m", "n", "t", "c").parts_text().is_empty());
    }

    #[test]
    fn parts_numbered_for_multipart() {
        assert_eq!(parts_text(3, 1), "（第 1/3 条）");
        assert_eq!(parts_text(2, 2), "（第 2/2 条）");
        // index 从 0 起算的脏数据也要能看
        assert_eq!(parts_text(2, 0), "（第 1/2 条）");
    }

    // ---------- 模板 ----------

    #[test]
    fn renders_all_placeholders() {
        let out = render_template(DEFAULT_TEMPLATE, &vars());
        assert!(out.starts_with("📩 新短信\n"), "单条时 {{parts}} 是空串");
        assert!(out.contains("模组：模组 0"));
        assert!(out.contains("号码：10086"));
        assert!(out.contains("时间：2026-10-18 12:00:00"));
        assert!(out.contains("内容：您好"));
    }

    #[test]
    fn renders_parts_placeholder() {
        let mut v = vars();
        v.total = 2;
        v.index = 1;
        let out = render_template(DEFAULT_TEMPLATE, &v);
        assert!(out.starts_with("📩 新短信（第 1/2 条）\n"));
    }

    #[test]
    fn unknown_placeholder_is_left_visible() {
        // 写错占位符时原样留着：消息里露出 {typo} 比静默变空串好排查
        let out = render_template("x{modem}y{typo}z", &vars());
        assert_eq!(out, "x模组 0y{typo}z");
    }

    #[test]
    fn effective_template_falls_back_to_default() {
        assert_eq!(effective_template(""), DEFAULT_TEMPLATE);
        assert_eq!(effective_template("   "), DEFAULT_TEMPLATE);
        assert_eq!(effective_template("自定义"), "自定义");
    }

    #[test]
    fn template_with_newline_is_preserved() {
        let out = render_template("a\nb{number}", &vars());
        assert_eq!(out, "a\nb10086");
    }

    // ---------- 发送者过滤 ----------

    #[test]
    fn empty_allowed_means_no_limit() {
        assert!(sender_allowed("anyone@im.wechat", ""));
        assert!(sender_allowed("anyone@im.wechat", "   "));
    }

    #[test]
    fn allowed_is_substring_match() {
        assert!(sender_allowed("zhangsan@im.wechat", "zhangsan,lisi"));
        assert!(sender_allowed("lisi@im.wechat", "zhangsan, lisi"));
        assert!(!sender_allowed("wangwu@im.wechat", "zhangsan,lisi"));
    }

    #[test]
    fn allowed_is_case_insensitive() {
        assert!(sender_allowed("ZhangSan@im.wechat", "zhangsan"));
        assert!(sender_allowed("zhangsan@im.wechat", "ZhangSan"));
    }

    #[test]
    fn empty_entries_in_allowed_are_ignored() {
        // ",,zhangsan,," 里空项不该被当成「匹配一切」
        assert!(sender_allowed("zhangsan", ",,zhangsan,,"));
        assert!(!sender_allowed("lisi", ",,zhangsan,,"));
    }

    #[test]
    fn empty_sender_is_never_allowed() {
        assert!(!sender_allowed("", ""));
        assert!(!sender_allowed("   ", ""));
    }

    // ---------- 推送目标 ----------

    #[test]
    fn explicit_target_wins() {
        assert_eq!(
            pick_target("boss@im.wechat", "someone@im.wechat"),
            Some("boss@im.wechat".to_string())
        );
    }

    #[test]
    fn falls_back_to_learned_sender() {
        assert_eq!(
            pick_target("", "someone@im.wechat"),
            Some("someone@im.wechat".to_string())
        );
        assert_eq!(pick_target("  ", " someone "), Some("someone".to_string()));
    }

    #[test]
    fn no_target_means_cannot_push() {
        assert_eq!(pick_target("", ""), None);
        assert_eq!(pick_target("  ", "   "), None);
    }

    // ---------- 登录态 ----------

    #[test]
    fn login_state_from_config() {
        assert_eq!(login_from_config("", "", ""), LoginState::LoggedOut);
        let st = login_from_config("tok", "bid", "buid");
        assert_eq!(
            st,
            LoginState::LoggedIn {
                bot_token: "tok".into(),
                bot_id: "bid".into(),
                bot_user_id: "buid".into()
            }
        );
        assert!(st.can_push());
    }

    #[test]
    fn qr_state_expiry() {
        let st = LoginState::QrPending {
            url: "https://x".into(),
            expires_at: 100,
        };
        assert!(!qr_expired(&st, 99));
        assert!(qr_expired(&st, 100));
        // 非等待态一律算「没过期」（没有可过期的东西）
        assert!(!qr_expired(&LoginState::LoggedOut, 999));
    }

    #[test]
    fn logged_out_cannot_push() {
        assert!(!LoginState::LoggedOut.can_push());
        assert!(!LoginState::QrPending {
            url: "u".into(),
            expires_at: 0
        }
        .can_push());
    }

    #[test]
    fn labels_are_non_empty() {
        for st in [
            LoginState::LoggedOut,
            LoginState::QrPending {
                url: "u".into(),
                expires_at: 0,
            },
            login_from_config("t", "", ""),
        ] {
            assert!(!st.label().is_empty());
        }
    }

    // ---------- 端点 ----------

    #[test]
    fn api_url_joins_without_double_slash() {
        assert_eq!(
            api_url("https://ilinkai.weixin.qq.com", "/cgi-bin/send"),
            "https://ilinkai.weixin.qq.com/cgi-bin/send"
        );
        assert_eq!(
            api_url("https://ilinkai.weixin.qq.com/", "/cgi-bin/send"),
            "https://ilinkai.weixin.qq.com/cgi-bin/send"
        );
        assert_eq!(
            api_url("https://ilinkai.weixin.qq.com//", "cgi-bin/send"),
            "https://ilinkai.weixin.qq.com/cgi-bin/send"
        );
        assert_eq!(
            api_url("https://x.test/", api::SEND_PATH),
            "https://x.test/cgi-bin/webhook/send"
        );
    }

    #[test]
    fn api_url_with_empty_base_is_relative() {
        assert_eq!(api_url("", "/cgi-bin/send"), "/cgi-bin/send");
        assert_eq!(api_url("   ", "cgi-bin/send"), "/cgi-bin/send");
    }

    // ---------- 收消息 ----------

    #[test]
    fn learns_only_allowed_senders() {
        let ok = IncomingMessage {
            sender: "zhangsan@im.wechat".into(),
            content: "在吗".into(),
            time: 1,
        };
        let bad = IncomingMessage {
            sender: "lisi@im.wechat".into(),
            content: "在吗".into(),
            time: 1,
        };
        let anon = IncomingMessage {
            sender: "".into(),
            content: "在吗".into(),
            time: 1,
        };
        assert!(should_learn_sender(&ok, "zhangsan"));
        assert!(!should_learn_sender(&bad, "zhangsan"));
        assert!(!should_learn_sender(&anon, ""));
    }
}
