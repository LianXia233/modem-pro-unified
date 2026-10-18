//! `GET /api/roam?modem_id=N` —— 数据漫游状态。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let view = modem.roam_view(state.allow_roaming());
    response::ok(req, serde_json::to_value(view)?)
}
