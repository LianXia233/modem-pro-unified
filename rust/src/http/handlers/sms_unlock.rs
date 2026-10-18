//! `POST /api/sms/unlock` —— 提交访问密码换令牌。
//!
//! 失败节流：同一 IP 连续失败 5 次锁定 60 秒；锁定期内直接 429。

use std::collections::HashMap;

use tiny_http::{Method, Request};

use crate::error::Result;
use crate::http::{request, response, smsauth, AppState};

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    if *req.method() != Method::Post {
        return response::fail(req, -1, "method not allowed, use POST");
    }
    let params = request::merge_form(query, &request::read_body(&mut req));

    if !state.sms_auth.required(&state.cfg) {
        return response::ok(req, serde_json::json!({ "unlocked": true, "token": "" }));
    }

    let ip = smsauth::client_ip(&req);
    if let Err(wait) = state.sms_auth.check(&ip) {
        let secs = wait.as_secs() + 1;
        return response::fail_status_data(
            req,
            429,
            -1,
            serde_json::json!({ "retry_after_s": secs }),
            &format!("密码错误次数过多，请 {secs} 秒后再试"),
        );
    }

    let given = params.get("password").cloned().unwrap_or_default();
    if !password_matches(&state.sms_auth.password(&state.cfg), &given) {
        let (n, locked) = state.sms_auth.note_fail(&ip);
        log::warn!("短信解锁失败 remote={ip} tries={n}");
        if locked {
            let secs = smsauth::FAIL_LOCK.as_secs();
            return response::fail_status_data(
                req,
                429,
                -1,
                serde_json::json!({ "retry_after_s": secs }),
                &format!("密码错误次数过多，已锁定 {secs} 秒"),
            );
        }
        return response::fail_status_data(
            req,
            403,
            -1,
            serde_json::json!({ "tries": n, "left": smsauth::FAIL_MAX - n }),
            &format!("密码不正确（还可尝试 {} 次）", smsauth::FAIL_MAX - n),
        );
    }

    let Some(token) = state.sms_auth.issue() else {
        return response::fail(req, -1, "生成访问令牌失败");
    };
    state.sms_auth.note_success(&ip);
    log::info!("短信已解锁 remote={ip}");
    response::ok(
        req,
        serde_json::json!({
            "unlocked": true,
            "token": token,
            "ttl_s": smsauth::TOKEN_TTL.as_secs(),
        }),
    )
}

/// 定长比较，避免因提前返回而泄露密码长度之外的信息。
fn password_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::password_matches;

    #[test]
    fn password_compare() {
        assert!(password_matches("password", "password"));
        assert!(!password_matches("password", "passwore"));
        assert!(!password_matches("password", "pass"));
        assert!(!password_matches("", "x"));
    }
}
