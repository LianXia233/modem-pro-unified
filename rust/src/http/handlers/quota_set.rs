//! `POST /api/quota/set` —— 保存配额设置（并按新口径立即评估）。
//!
//! 请求体：`{modem_id, enabled, count_up, count_down, action, reset_day,
//! limit_bytes | limit_mb | limit_gb, used_gb?}`。
//! 上限三种写法只认一个（优先 `limit_bytes`）；都不带则沿用旧值。
//! `used_gb` 是**对账校正**：按当前上下行比例拆到两个方向，之后从这个数继续累计。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{quota, request, response, AppState};
use crate::modem::model::QuotaSettings;

use super::modem_by_query;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MIB: f64 = 1024.0 * 1024.0;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };

    let old = quota::settings_of(state, modem.id);
    // 上限：空输入框会显式带 limit_bytes=0（= 不限量），所以按「有没有这个键」判断
    let limit_bytes = if let Some(v) = request::f64_param(&params, "limit_bytes") {
        v.max(0.0) as i64
    } else if let Some(gb) = request::f64_param(&params, "limit_gb") {
        (gb.max(0.0) * GIB) as i64
    } else if let Some(mb) = request::f64_param(&params, "limit_mb") {
        (mb.max(0.0) * MIB) as i64
    } else {
        old.limit_bytes
    };

    let action = params
        .get("action")
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| old.action.clone());

    let s = QuotaSettings {
        enabled: request::bool_param(&params, "enabled", old.enabled),
        limit_bytes,
        count_up: request::bool_param(&params, "count_up", old.count_up),
        count_down: request::bool_param(&params, "count_down", old.count_down),
        action,
        reset_day: request::i64_param(&params, "reset_day", old.reset_day),
    };
    if !s.count_up && !s.count_down {
        return response::fail(req, -1, "上行/下行至少要计入一个方向");
    }

    // 先落设置（可能当场就超量断网），再做用量校正
    let mut view = quota::save_settings(state, &modem, s);
    if let Some(gb) = request::f64_param(&params, "used_gb") {
        let total = (gb.max(0.0) * GIB) as u64;
        view = quota::set_used(state, &modem, total);
    }
    // 改完立刻广播：别的页面（含屏端）不用等下一轮配额循环
    crate::http::push::push_quota(state, &modem, &serde_json::to_value(&view)?);
    response::ok(req, serde_json::to_value(view)?)
}
