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
    // bands 可能是 JSON 数组（"[78,3]"）或表单串（"78:3" / "78,3"），
    // 解析与校验口径与定时任务 lock.band 共用（见 handlers::band_lock_params）
    let (tech, bands, unlock) = match super::band_lock_params(&params) {
        Ok(v) => v,
        Err(e) => return response::fail(req, -1, &e.to_string()),
    };
    match modem.set_band_lock(&tech, &bands, unlock) {
        Ok(v) => {
            crate::http::push::push_lock(state, &modem, &v);
            response::ok(req, serde_json::to_value(v)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::http::handlers::parse_bands;

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
