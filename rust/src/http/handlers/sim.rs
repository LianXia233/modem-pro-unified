//! `GET /api/sim?modem_id=N` —— SIM 卡身份概览。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    // AT 会话失败（串口忙 / 模组拔出）必须变成信封里的 msg：
    // 用 `?` 会把 Request 丢回网关层，连接上什么都不会写。
    let status = match modem.modem_status(state.cfg.net.allow_roaming) {
        Ok(s) => s,
        Err(e) => return response::fail(req, -1, &e.to_string()),
    };
    let (last_event, last_event_at) = modem.sim_last_event();
    let checked_at = modem.sim_checked_at();
    let mut data = json!({
        "modem_id": status.modem_id,
        "present": status.sim_present,
        "sim_state": status.sim_state,
        "iccid": status.iccid,
        "imsi": status.imsi,
        "operator": status.operator,
    });
    // 热插拔那几项：没有过事件就不出现（omitempty 口径）
    if !last_event.is_empty() {
        data["last_event"] = json!(last_event);
        data["last_event_at"] = json!(crate::logging::rfc3339(last_event_at));
    }
    if checked_at > 0 {
        data["checked_at"] = json!(crate::logging::rfc3339(checked_at));
    }
    response::ok(req, data)
}
