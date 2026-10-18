//! `GET /api/imei?modem_id=N` —— IMEI 管理状态。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let backup = state.imei.get(modem.id);
    // 只读状态：没有正在进行的写操作，不带 warning
    response::ok(
        req,
        serde_json::to_value(modem.imei_status(backup.as_ref(), None))?,
    )
}
