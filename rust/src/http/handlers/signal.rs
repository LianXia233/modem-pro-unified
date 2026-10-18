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
            response::ok(req, serde_json::to_value(v)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
