//! `GET /api/modem/status?modem_id=N` —— SIM 卡与模组状态。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let allow_roaming = state.allow_roaming();
    match modem.modem_status(allow_roaming) {
        Ok(v) => {
            let value = match response::to_value(&v) {
                Ok(j) => j,
                Err(e) => return response::fail(req, -1, &e),
            };
            // 进缓存：概览页的「SIM 卡信息 / 模组硬件参数」两块卡片靠它一次性拿到，
            // 不必为了显示 ICCID / IMEI 再单独发一轮 AT（见 http/cache.rs）。
            state.cache.put("modem", modem.id, value.clone());
            response::ok(req, value)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
