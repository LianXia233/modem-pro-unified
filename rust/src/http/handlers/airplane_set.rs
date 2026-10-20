//! `POST /api/airplane/set` —— 开关飞行模式（`{modem_id, enabled}`；`AT+CFUN=4` / `AT+CFUN=1`）。
//!
//! 不支持 `AT+CFUN?` 的机型（展锐 SIPC 那一族）会被拒绝：关射频会把设备自身的管理通道一起断掉。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let on = request::bool_param(&params, "enabled", false);
    match modem.set_airplane(on) {
        Ok(()) => {
            // 飞行模式会直接掐断数据链路（CFUN=4）：把连接状态拉回 dialing 并作废
            // 在途流程，统一调度下一轮会按退避尝试（飞行中会失败并继续退避）。
            // 关掉飞行模式时清退避 —— 立刻恢复拨号，不等旧窗口。
            if on {
                modem.note_external_break("已进入飞行模式，数据链路中断");
            } else {
                crate::runtime::dialer::supervisor().reset_backoff(modem.id);
            }
            response::ok(req, serde_json::to_value(modem.airplane())?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
