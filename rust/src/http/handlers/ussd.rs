//! `/api/ussd` —— USSD 应答（GET）与下发（POST）。
//!
//!   · `GET  /api/ussd?modem_id=N` 读最近一次应答：`{modem_id, result}`（`result` 可能是 null）；
//!   · `POST /api/ussd` `{modem_id, code}` 下发，成功回 `msg:"已下发，等待模组应答（异步推送）"`。
//!
//! ⚠️ 真正的应答是**异步 URC**（`+CUSD`）：POST 只代表「已下发」，结果由 WebSocket
//! `ussd` 消息推给页面（或稍后 GET 一次）。少数固件把 `+CUSD` 放在 `OK` 之前一起回来，
//! 那种情况下发时就地收下并当场推送（见 `modem/model/ussd.rs`）。

use std::collections::HashMap;

use serde_json::Value;
use tiny_http::{Method, Request};

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    if *req.method() == Method::Get {
        return handle_get(req, state, query);
    }
    // POST：先读 body 再按字段分支（表单 / JSON 两种提交方式都认）
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let code = params.get("code").cloned().unwrap_or_default();
    match modem.send_ussd(&code) {
        // 同步就收到 +CUSD 的固件：这里顺手推给页面，不用等下一轮
        Ok(Some(result)) => {
            crate::http::push::push_ussd(state, &modem, &result);
            response::ok_msg(req, Value::Null, "已下发，等待模组应答（异步推送）")
        }
        Ok(None) => response::ok_msg(req, Value::Null, "已下发，等待模组应答（异步推送）"),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}

fn handle_get(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let result = match modem.ussd_last() {
        Some(result) => serde_json::to_value(result)?,
        None => Value::Null,
    };
    response::ok(
        req,
        serde_json::json!({ "modem_id": modem.id, "result": result }),
    )
}
