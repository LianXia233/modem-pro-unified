//! `POST /api/lock/band` —— 锁频段 / 解除频段锁定。
//!
//! 请求体：`{modem_id, tech, bands: [78], unlock?}`。
//! `unlock=true` = 该技术域不限制（QNW 走 policy_band 全量写回，见 model/lock.rs）。
//! 返回最新锁网状态（同 `GET /api/lock/status`）。

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
    let tech = params
        .get("tech")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let unlock = request::bool_param(&params, "unlock", false);
    // bands 可能是 JSON 数组（"[78,3]"）或表单串（"78:3" / "78,3"）
    let bands = parse_bands(params.get("bands").map(|s| s.as_str()).unwrap_or(""));
    if !unlock && bands.is_empty() {
        return response::fail(req, -1, "要么给几个频段（bands），要么显式 unlock=true");
    }
    match modem.set_band_lock(&tech, &bands, unlock) {
        Ok(v) => {
            crate::http::push::push_lock(state, &modem, &v);
            response::ok(req, serde_json::to_value(v)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}

/// 解析频段入参：`[78,3]`（JSON 数组被 merge_form 字符串化）/ `78:3` / `78,3` 都认。
fn parse_bands(s: &str) -> Vec<i32> {
    s.trim()
        .trim_matches(|c| c == '[' || c == ']')
        .split([':', ',', ' '])
        .filter_map(|p| p.trim().parse::<i32>().ok())
        .filter(|b| *b > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_bands;

    #[test]
    fn bands_from_json_array_string() {
        assert_eq!(parse_bands("[78]"), vec![78]);
        assert_eq!(parse_bands("[78,3,1]"), vec![78, 3, 1]);
    }

    #[test]
    fn bands_from_form_string() {
        assert_eq!(parse_bands("78:3"), vec![78, 3]);
        assert_eq!(parse_bands("78, 3"), vec![78, 3]);
        assert_eq!(parse_bands("1 3 8"), vec![1, 3, 8]);
    }

    #[test]
    fn bands_rejects_junk() {
        assert!(parse_bands("").is_empty());
        assert!(parse_bands("[]").is_empty());
        assert!(parse_bands("abc").is_empty());
        assert_eq!(parse_bands("[78,x,-1]"), vec![78]);
    }
}
