//! `GET /api/usbnet?modem_id=N` —— USB 网卡模式。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    state.note_http_seen();
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let st = modem.usbnet();
    let value = match response::to_value(&st) {
        Ok(j) => j,
        Err(e) => return response::fail(req, -1, &e),
    };
    // 进缓存：`usbnet` 是 StateCache::KINDS 里的一员，不归档的话快照里
    // 会出现一个「永远 null、永远过期」的占位块（见 http/cache.rs 的表）。
    state.cache.put("usbnet", modem.id, value.clone());
    response::ok(req, value)
}
