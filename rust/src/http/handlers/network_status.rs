//! `GET /api/network/status?modem_id=N` —— 网络状态总览。
//!
//! 查询结果顺手喂给波动记录（见 `modem/metrics.rs`），不额外发 AT。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::{modem_by_query, push_changes};

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let status = modem.network_status(state.cfg.net.allow_roaming);
    push_changes(
        state,
        modem.id,
        &state.metrics.note_network(modem.id, &status),
    );
    let value = match response::to_value(status) {
        Ok(j) => j,
        Err(e) => return response::fail(req, -1, &e),
    };
    // 进缓存：快照页免发 AT（network_status 是一次串口会话，代价不小）
    state.cache.put("network", modem.id, value.clone());
    response::ok(req, value)
}
