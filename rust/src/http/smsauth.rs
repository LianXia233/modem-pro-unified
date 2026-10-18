//! 短信访问密码：令牌、失败节流与闸门。
//!
//! 短信内容比别的接口敏感，除「跟随 LuCI 登录」外层闸门外再加一道访问密码：
//! 未解锁时短信接口一律回 `code=401` + `{"locked":true,"required":true}`，前端据此弹密码框。
//!
//! 外层登录闸门开着时这道密码**自动停用**（能调到这些接口的人本来就已经登录过路由器，
//! 再要一遍纯属重复）；`[ui] sms_password` 显式写成空串同样表示不需要密码。
//!
//! 密码可以在页面上改（写回 `config.toml` 的那两行），改完要**立刻生效而不重启**；
//! 但进程内的 `Config` 是启动时读进来的，所以这里存一份运行期覆盖值，
//! 改密码 / 开关保护后由 handler 调用 [`SmsAuth::apply_change`] 覆盖。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};
use tiny_http::Request;

use crate::config::Config;
use crate::error::Result;

use super::{request, response};

/// 令牌滑动过期时间：每次使用续期，闲置超过它就失效。
pub const TOKEN_TTL: Duration = Duration::from_secs(30 * 60);

/// 最多同时保留的会话数（防止令牌表被刷爆）。
const TOKEN_MAX: usize = 32;
/// 同一 IP 连续失败多少次后锁定。
pub const FAIL_MAX: i32 = 5;
/// 锁定时长。
pub const FAIL_LOCK: Duration = Duration::from_secs(60);
/// 密码留空时的默认值（前端据此提醒用户改掉）。
const DEFAULT_PASSWORD: &str = "password";
/// 令牌请求头。
const TOKEN_HEADER: &str = "X-SMS-Token";

/// 短信访问密码的令牌表、失败节流表与运行期覆盖值。
pub struct SmsAuth {
    inner: Mutex<Inner>,
    /// 页面改过密码后的运行期值（与写回 config.toml 同步，免得要重启才生效）
    password: Mutex<Option<String>>,
    /// 页面拨过的「要不要密码」开关
    required: Mutex<Option<bool>>,
}

#[derive(Default)]
struct Inner {
    tokens: HashMap<String, Instant>,
    fails: HashMap<String, i32>,
    locked_until: HashMap<String, Instant>,
}

impl SmsAuth {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            password: Mutex::new(None),
            required: Mutex::new(None),
        }
    }

    /// 本请求是否已解锁（不需要密码时恒为 true）。
    pub fn unlocked(&self, cfg: &Config, req: &Request) -> bool {
        if !self.required(cfg) {
            return true;
        }
        self.token_valid(&token_from_request(req))
    }

    /// 当前是否要求短信密码（运行期覆盖优先于配置文件）。
    pub fn required(&self, cfg: &Config) -> bool {
        if let Some(v) = *self.required.lock().unwrap() {
            return v && !cfg.require_luci_login();
        }
        required(cfg)
    }

    /// 当前生效的密码（运行期覆盖优先于配置文件）。
    pub fn password(&self, cfg: &Config) -> String {
        match self.password.lock().unwrap().as_ref() {
            Some(p) => p.clone(),
            None => current_password(cfg),
        }
    }

    /// 是否还在用默认密码。
    pub fn is_default(&self, cfg: &Config) -> bool {
        self.password(cfg) == DEFAULT_PASSWORD
    }

    /// 改密码 / 开关保护后同步进来：覆盖值 + 吊销全部旧令牌。
    pub fn apply_change(&self, password: Option<String>, required: Option<bool>) {
        if let Some(p) = password {
            *self.password.lock().unwrap() = Some(p);
        }
        if let Some(r) = required {
            *self.required.lock().unwrap() = Some(r);
        }
        self.revoke_all();
    }

    /// 生成一个新令牌（失败返回 None）。
    pub fn issue(&self) -> Option<String> {
        let tok = new_token()?;
        let mut inner = self.inner.lock().unwrap();
        inner.tokens.insert(tok.clone(), Instant::now());
        prune(&mut inner);
        Some(tok)
    }

    /// 吊销一个令牌（主动锁定）。
    pub fn revoke(&self, tok: &str) {
        let tok = tok.trim();
        if tok.is_empty() {
            return;
        }
        self.inner.lock().unwrap().tokens.remove(tok);
    }

    /// 吊销全部令牌（改密码/开关保护后用）。
    pub fn revoke_all(&self) {
        self.inner.lock().unwrap().tokens.clear();
    }

    /// 是否允许尝试：被锁定则返回还需等待的时长。
    pub fn check(&self, ip: &str) -> std::result::Result<(), Duration> {
        let inner = self.inner.lock().unwrap();
        match inner.locked_until.get(ip) {
            Some(until) => {
                let now = Instant::now();
                if *until > now {
                    return Err(*until - now);
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// 记一次失败，返回（连续失败次数，是否已锁定）。
    pub fn note_fail(&self, ip: &str) -> (i32, bool) {
        let mut inner = self.inner.lock().unwrap();
        let n = {
            let c = inner.fails.entry(ip.to_string()).or_insert(0);
            *c += 1;
            *c
        };
        if n >= FAIL_MAX {
            inner
                .locked_until
                .insert(ip.to_string(), Instant::now() + FAIL_LOCK);
            return (n, true);
        }
        (n, false)
    }

    /// 记一次成功：清掉该 IP 的失败计数与锁定。
    pub fn note_success(&self, ip: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.fails.remove(ip);
        inner.locked_until.remove(ip);
    }

    /// 校验并续期（滑动过期）。
    ///
    /// 短信接口与短信推送（`sms` 事件）共用这一把闸门，所以对推送侧也开放：
    /// 推送侧只能「验令牌」，不能改状态。
    pub fn token_valid(&self, tok: &str) -> bool {
        let tok = tok.trim();
        if tok.is_empty() {
            return false;
        }
        let mut inner = self.inner.lock().unwrap();
        let Some(last) = inner.tokens.get(tok).copied() else {
            return false;
        };
        if last.elapsed() > TOKEN_TTL {
            inner.tokens.remove(tok);
            return false;
        }
        inner.tokens.insert(tok.to_string(), Instant::now());
        true
    }
}

impl Default for SmsAuth {
    fn default() -> Self {
        Self::new()
    }
}

/// 是否需要短信访问密码。
pub fn required(cfg: &Config) -> bool {
    cfg.ui.sms_password_required && !cfg.ui.sms_password.is_empty() && !cfg.require_luci_login()
}

/// 当前生效的密码。
pub fn current_password(cfg: &Config) -> String {
    if cfg.ui.sms_password.is_empty() {
        DEFAULT_PASSWORD.to_string()
    } else {
        cfg.ui.sms_password.clone()
    }
}

/// 请求带来的令牌：优先请求头，其次查询参数（WebSocket 握手只能用后者）。
pub fn token_from_request(req: &Request) -> String {
    for header in req.headers() {
        if header.field.equiv(TOKEN_HEADER) {
            let v = header.value.as_str().trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    let url = req.url().to_string();
    request::parse_query(&url)
        .get("sms_token")
        .cloned()
        .unwrap_or_default()
}

/// 请求来源 IP（节流用）。
pub fn client_ip(req: &Request) -> String {
    req.remote_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default()
}

/// 未解锁时的统一响应（调用方直接 return）。
pub fn locked_response(req: Request, state: &super::AppState) -> Result<()> {
    let need = state.sms_auth.required(&state.cfg);
    response::fail_status_data(
        req,
        401,
        401,
        serde_json::json!({ "locked": true, "required": need }),
        "短信需要密码，请先解锁",
    )
}

/// 生成令牌：用系统时间、进程号、栈地址与自增计数混合后做 SHA-1。
///
/// 不引入额外随机源依赖：这几路熵在设备上互不相关，且令牌只在本进程内存里，
/// 与 30 分钟滑动过期配合足够。
fn new_token() -> Option<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let pid = std::process::id();
    let probe = 0u8;
    let addr = std::ptr::addr_of!(probe) as usize as u64;
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha1::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(pid.to_le_bytes());
    hasher.update(addr.to_le_bytes());
    hasher.update(seq.to_le_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(40);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    Some(out)
}

/// 清理过期令牌，并裁剪到上限（调用方持锁）。
fn prune(inner: &mut Inner) {
    let now = Instant::now();
    inner
        .tokens
        .retain(|_, last| now.duration_since(*last) <= TOKEN_TTL);
    while inner.tokens.len() > TOKEN_MAX {
        let Some(oldest) = inner
            .tokens
            .iter()
            .min_by_key(|(_, last)| **last)
            .map(|(t, _)| t.clone())
        else {
            break;
        };
        inner.tokens.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(required: bool, password: &str, luci_login: bool) -> Config {
        let mut c = Config::default();
        c.ui.sms_password_required = required;
        c.ui.sms_password = password.to_string();
        c.ui.require_luci_login = luci_login;
        c
    }

    #[test]
    fn password_requirement_rules() {
        assert!(required(&cfg(true, "password", false)));
        assert!(
            !required(&cfg(false, "password", false)),
            "显式关掉就不需要"
        );
        assert!(!required(&cfg(true, "", false)), "显式空密码 = 不需要");
        assert!(
            !required(&cfg(true, "password", true)),
            "外层登录闸门开着时不再重复要密码"
        );
    }

    #[test]
    fn default_password_detected() {
        let auth = SmsAuth::new();
        assert!(auth.is_default(&cfg(true, "password", false)));
        assert!(auth.is_default(&cfg(true, "", false)));
        assert!(!auth.is_default(&cfg(true, "hunter2", false)));
    }

    #[test]
    fn runtime_override_wins_over_config() {
        let auth = SmsAuth::new();
        let c = cfg(true, "password", false);
        assert!(auth.required(&c));
        auth.apply_change(Some("hunter2".to_string()), Some(false));
        assert!(!auth.required(&c), "关掉开关后应立即不需要密码");
        assert_eq!(auth.password(&c), "hunter2");
        // 开关拨回来时密码仍是新的
        auth.apply_change(None, Some(true));
        assert!(auth.required(&c));
        assert_eq!(auth.password(&c), "hunter2");
    }

    #[test]
    fn outer_login_still_disables_password() {
        let auth = SmsAuth::new();
        auth.apply_change(None, Some(true));
        assert!(!auth.required(&cfg(true, "password", true)));
    }

    #[test]
    fn token_lifecycle() {
        let auth = SmsAuth::new();
        assert!(!auth.token_valid(""));
        assert!(!auth.token_valid("nope"));
        let tok = auth.issue().expect("应能生成令牌");
        assert_eq!(tok.len(), 40);
        assert!(auth.token_valid(&tok));
        auth.revoke(&tok);
        assert!(!auth.token_valid(&tok));
        let tok2 = auth.issue().unwrap();
        auth.revoke_all();
        assert!(!auth.token_valid(&tok2));
    }

    #[test]
    fn tokens_are_distinct() {
        let auth = SmsAuth::new();
        let a = auth.issue().unwrap();
        let b = auth.issue().unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn fail_throttle_locks_after_max() {
        let auth = SmsAuth::new();
        assert!(auth.check("1.2.3.4").is_ok());
        for _ in 0..FAIL_MAX - 1 {
            let (_, locked) = auth.note_fail("1.2.3.4");
            assert!(!locked);
        }
        let (n, locked) = auth.note_fail("1.2.3.4");
        assert_eq!(n, FAIL_MAX);
        assert!(locked);
        assert!(auth.check("1.2.3.4").is_err(), "锁定期内应被拒");
        assert!(auth.check("5.6.7.8").is_ok(), "别的 IP 不受影响");
        auth.note_success("1.2.3.4");
        assert!(auth.check("1.2.3.4").is_ok());
    }

    #[test]
    fn prune_keeps_table_bounded() {
        let auth = SmsAuth::new();
        let mut inner = auth.inner.lock().unwrap();
        for i in 0..TOKEN_MAX + 5 {
            inner.tokens.insert(format!("t{i}"), Instant::now());
        }
        prune(&mut inner);
        assert!(inner.tokens.len() <= TOKEN_MAX);
    }
}
