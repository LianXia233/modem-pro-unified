//! `POST /api/traffic/reset` —— 清零模组的累计流量计数器。
//!
//! 清的是**模组那一侧**的累计（`AT+QGDCNT=0`）；速率式机型没有可清的计数器，
//! 只重置本进程的积分基准。配额的「本周期用量」是另一本账，不在这里动。

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
    let (cleared, _) = modem.reset_traffic_counter();
    let mut view = modem.traffic_view();
    if !cleared {
        view.note =
            Some("该模组没有可清零的累计计数器（只报瞬时速率），已重置本地积分基准".to_string());
    }
    // 清零后计数器从头开始：立刻广播新采样，页面上的累计数要跟着归零。
    // ⚠️ 不走 push_traffic：那个是给周期循环用的，会过滤掉「不上报计数器」的模组；
    // 而这里是用户刚按了按钮，必须有个回响（口径同 Go 的 apiResetTraffic：无条件推）。
    let data = serde_json::to_value(&view).unwrap_or(serde_json::Value::Null);
    state.ws.broadcast("traffic", Some(modem.id), data);
    response::ok(req, serde_json::to_value(view)?)
}
