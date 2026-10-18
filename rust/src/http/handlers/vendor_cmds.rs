//! `GET /api/vendor/cmds?modem_id=N` —— 该模组探测出的厂商指令集。

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
    let vendor = modem.vendor();
    let cmds = vendor.spec().vendor_cmds().to_vec();
    response::ok(
        req,
        json!({ "vendor": vendor.as_str(), "vendor_name": vendor.label(), "cmds": cmds }),
    )
}
