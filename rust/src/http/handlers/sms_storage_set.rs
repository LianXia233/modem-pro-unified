//! `POST /api/sms/storage` —— 切换短信存储区（`{modem_id, mem}`，mem ∈ SM/ME/MT）。
//!
//! 与 GET 同路径不同方法：GET 读用量，POST 切存储区（下发 `AT+CPMS` 后回读确认），
//! 并把选择写回 `config.toml` 的 `[sms] sms_memory`，下次启动自动沿用。

use std::collections::HashMap;

use tiny_http::Request;

use crate::config::patch;
use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let mem = params.get("mem").cloned().unwrap_or_default();
    if mem.trim().is_empty() {
        return response::fail(req, -1, "要指定切到哪个存储区（SM / ME / MT）");
    }

    let st = match modem.set_sms_memory(&mem) {
        Ok(st) => st,
        Err(e) => return response::fail(req, -1, &e.to_string()),
    };
    // 写回配置：切完重启也能沿用（写失败只记日志，不影响本次结果）
    if let Err(e) = patch::set_string(&state.config_path, "sms", "sms_memory", st.current.as_str())
    {
        log::warn!("写回 [sms] sms_memory 失败: {e}");
    }
    log::info!("模组 {} 短信存储区切到 {}", modem.id, st.current);
    response::ok(req, serde_json::to_value(st)?)
}
