//! `POST /api/usbnet/set` —— 设置 USB 网卡模式（`{modem_id, value}`，0~3）。
//!
//! 只有移远 / 美格的 `AT+QCFG="usbnet"` 可写；广和通 RNDIS 不允许执行，其余厂商只能看。

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
    let value = request::i64_param(&params, "value", -1) as i32;
    match modem.set_usbnet(value) {
        Ok(st) => response::ok_msg(
            req,
            serde_json::to_value(st)?,
            "已写入；需要重启模组才会生效",
        ),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
