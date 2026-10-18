//! `GET /api/lock/caps?modem_id=N` —— 锁网指令支持探测（只读）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let caps = modem.lock_caps();
    let msg = if caps.supported {
        "ok"
    } else {
        "该模组固件不支持锁网（锁频段 / 锁基站）"
    };
    response::ok_msg(req, serde_json::to_value(caps)?, msg)
}
