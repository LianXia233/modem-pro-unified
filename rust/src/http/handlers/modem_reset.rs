//! `POST /api/modem/reset` —— 重启模组（`AT+CFUN=1,1`）。

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
    match modem.reboot() {
        // 没收到确认也按成功返回：指令已写进串口，重启本来就会断开 AT 口
        Ok(v) => {
            // 重启会断掉数据会话：把状态拉回 dialing（不是「用户断开」，**不置**
            // manual_stop），统一调度在模组回来后按退避重拨。早先这里什么都不做，
            // 状态会一直停在 connected，自动重拨永远不会触发（网卡早没地址了）。
            modem.note_external_break("模组重启中，连接将重建");
            crate::runtime::dialer::supervisor().reset_backoff(modem.id);
            response::ok(req, serde_json::to_value(v)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
