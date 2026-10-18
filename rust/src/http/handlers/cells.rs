//! `GET /api/cells?modem_id=N[&cache=1]` —— 基站扫描（服务小区 + 邻小区）。
//!
//! 扫描完成后顺手记录「服务小区 / 邻区数量」的波动（见 `modem/metrics.rs`）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::{modem_by_query, push_changes};

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let cached = query.get("cache").map(|v| v == "1").unwrap_or(false);
    let result = if cached {
        modem.cells_cached()
    } else {
        modem.scan_cells()
    };
    // 缓存为空时会把「未上报/0」当成基线，所以只在真正扫描后记录
    if !cached {
        push_changes(
            state,
            modem.id,
            &state.metrics.note_cells(modem.id, &result),
        );
        // 别的页面也刷新（小区扫描很慢，不能每个页面各扫一遍）
        crate::http::push::push_cells(state, &modem, &result);
    }
    response::ok(req, serde_json::to_value(result)?)
}
