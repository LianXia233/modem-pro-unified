//! `POST /api/at` —— 直发 AT 命令（参数 modem_id、cmd）。
//!
//! 契约（与 docs/API.md 一致）：
//!   · 只允许**单条**指令（换行会拼出多条指令，容易误改模组状态）；
//!   · 成功/失败都回 `data:{cmd, resp}` —— `resp` 是**应答行数组**（已去 \r\n），
//!     前端按行渲染；失败时 `resp` 可能是半截的。

use std::collections::HashMap;

use tiny_http::{Method, Request};

use crate::error::Result;
use crate::http::{request, response, AppState};
use crate::modem::model::split_at_lines;

use super::modem_by_query;

/// 单条指令的长度上限。
const MAX_CMD_LEN: usize = 128;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    if *req.method() != Method::Post {
        return response::fail(req, -1, "method not allowed, use POST");
    }
    let params = request::merge_form(query, &request::read_body(&mut req));

    let cmd = params.get("cmd").cloned().unwrap_or_default();
    let cmd = cmd.trim().to_string();
    if cmd.is_empty() {
        return response::fail(req, -1, "cmd required");
    }
    if cmd.len() > MAX_CMD_LEN || cmd.contains('\r') || cmd.contains('\n') {
        return response::fail(req, -1, "cmd must be a single line (<=128 chars)");
    }

    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(
            req,
            -1,
            &format!("modem not found: {}", modem_id_of(&params)),
        );
    };
    let timeout = state.cfg.serial.command_timeout();
    // 用户手动发的指令：**最高优先级**，插到所有后台采样前面。
    // 这是这一层调度存在的直接理由 —— 早先用户点「发送」要排队等信号采样
    // 和短信巡检，十几秒才回，看起来像卡死。
    match modem.with_at_prio(crate::modem::at::Prio::User, |p| p.command(&cmd, timeout)) {
        Ok(out) => response::ok(
            req,
            serde_json::json!({ "cmd": cmd, "resp": split_at_lines(&out) }),
        ),
        Err(e) => response::fail_status_data(
            req,
            200,
            -1,
            serde_json::json!({ "cmd": cmd, "resp": Vec::<String>::new() }),
            &e.to_string(),
        ),
    }
}

fn modem_id_of(params: &HashMap<String, String>) -> String {
    params
        .get("modem_id")
        .cloned()
        .unwrap_or_else(|| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::{modem_id_of, MAX_CMD_LEN};
    use std::collections::HashMap;

    #[test]
    fn cmd_len_limit() {
        assert_eq!(MAX_CMD_LEN, 128);
    }

    #[test]
    fn modem_id_default_zero() {
        let params = HashMap::new();
        assert_eq!(modem_id_of(&params), "0");
        let mut params = HashMap::new();
        params.insert("modem_id".to_string(), "3".to_string());
        assert_eq!(modem_id_of(&params), "3");
    }
}
