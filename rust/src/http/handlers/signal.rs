//! `GET /api/signal?modem_id=N` —— 信号（AT+CSQ）。
//!
//! 查询结果顺手喂给波动记录与采样历史（见 `modem/metrics.rs`），不额外发 AT。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::{modem_by_query, push_changes};

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    match modem.signal() {
        Ok(v) => {
            push_changes(state, modem.id, &state.metrics.note_signal(modem.id, &v));
            let value = serde_json::to_value(v)?;
            // 进缓存：概览页的快照不再重复发 AT（见 http/cache.rs）
            state.cache.put("signal", modem.id, value.clone());
            response::ok(req, value)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
