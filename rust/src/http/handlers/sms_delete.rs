//! `POST /api/sms/delete` —— 删一条（`{modem_id, index}`）或全部（`{modem_id, all:true}`）。
//!
//! 短信密码已取消（2026-10-19）；删除是**不可恢复**的，所以参数要卡死：只认数字位置。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };

    let all = request::bool_param(&params, "all", false);
    let result = if all {
        modem.delete_all_sms().map(|_| 0)
    } else {
        let index = params.get("index").cloned().unwrap_or_default();
        modem.delete_sms(&index).map(|_| 1)
    };
    match result {
        Ok(removed) => {
            log::info!("模组 {} 删除短信（all={all}）", modem.id);
            response::ok(
                req,
                json!({ "modem_id": modem.id, "all": all, "removed": removed }),
            )
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
