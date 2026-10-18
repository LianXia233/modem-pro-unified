//! `POST /api/sms/password` —— 改短信访问密码 / 开关保护。
//!
//! 请求体：`{current?, new_password?, enabled?}`。
//!   · 带 `new_password`：改密码（4~64 字符），写回 `config.toml` 的 `[ui] sms_password`；
//!   · 带 `enabled`：`false` = 关掉这道保护（密码本身留在配置里，以后再开不用重输）。
//!
//! 两件事都要**再输一次当前密码**（当前没启用保护时不用），并走与解锁相同的失败节流；
//! 成功后吊销全部旧令牌、给当前页面补发一个新的 —— 别的已解锁页面会被踢回锁屏，
//! 这是期望行为（改密码的人显然希望旧会话失效）。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::config::patch;
use crate::error::Result;
use crate::http::{request, response, smsauth, AppState};

/// 密码长度范围（与界面提示一致）。
const PWD_MIN: usize = 4;
const PWD_MAX: usize = 64;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let ip = smsauth::client_ip(&req);

    if let Err(wait) = state.sms_auth.check(&ip) {
        let secs = wait.as_secs() + 1;
        return response::fail_status_data(
            req,
            429,
            -1,
            json!({ "retry_after_s": secs }),
            &format!("尝试次数过多，请 {secs} 秒后再试"),
        );
    }

    let new_password = params.get("new_password").cloned();
    let enabled = params
        .get("enabled")
        .map(|_| request::bool_param(&params, "enabled", true));
    if new_password.is_none() && enabled.is_none() {
        return response::fail(req, -1, "要改密码就带 new_password，要开关保护就带 enabled");
    }

    let was_required = state.sms_auth.required(&state.cfg);
    // 关保护 / 改密码都得再验一次当前密码
    let need_current = was_required;
    if need_current {
        let given = params.get("current").cloned().unwrap_or_default();
        if !password_matches(&state.sms_auth.password(&state.cfg), &given) {
            let (n, locked) = state.sms_auth.note_fail(&ip);
            log::warn!("改短信密码失败（当前密码不对）remote={ip} tries={n}");
            let secs = smsauth::FAIL_LOCK.as_secs();
            if locked {
                return response::fail_status_data(
                    req,
                    429,
                    -1,
                    json!({ "retry_after_s": secs }),
                    &format!("密码错误次数过多，已锁定 {secs} 秒"),
                );
            }
            return response::fail_status_data(
                req,
                403,
                -1,
                json!({ "tries": n, "left": smsauth::FAIL_MAX - n }),
                "当前密码不正确",
            );
        }
    }

    if let Some(pwd) = new_password.as_ref() {
        let len = pwd.chars().count();
        if !(PWD_MIN..=PWD_MAX).contains(&len) {
            return response::fail(
                req,
                -1,
                &format!("新密码长度要在 {PWD_MIN}~{PWD_MAX} 个字符之间"),
            );
        }
    }

    // 写回 config.toml（只改那两行，注释原样保留，改前备份 .bak）
    if let Some(pwd) = new_password.as_ref() {
        if let Err(e) = patch::set_string(&state.config_path, "ui", "sms_password", pwd) {
            return response::fail(req, -1, &format!("写回配置失败：{e}"));
        }
    }
    let want_required = enabled.unwrap_or(true);
    if enabled.is_some() {
        let text = if want_required { "true" } else { "false" };
        if let Err(e) = patch::set_raw(&state.config_path, "ui", "sms_password_required", text) {
            return response::fail(req, -1, &format!("写回配置失败：{e}"));
        }
    }

    state.sms_auth.apply_change(new_password.clone(), enabled);
    state.sms_auth.note_success(&ip);
    log::info!("短信访问密码已更新 remote={ip}（旧令牌已全部吊销）");

    let required = state.sms_auth.required(&state.cfg);
    let token = if required {
        state.sms_auth.issue().unwrap_or_default()
    } else {
        String::new()
    };
    response::ok(
        req,
        json!({
            "required": required,
            "token": token,
            "ttl_s": smsauth::TOKEN_TTL.as_secs(),
            "default": required && state.sms_auth.is_default(&state.cfg),
        }),
    )
}

/// 定长比较（与 unlock 同一套写法，避免提前返回泄露信息）。
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
        assert!(!password_matches("password", ""));
    }
}
