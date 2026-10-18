//! `GET /api/at/presets` —— 本模组的常用 AT 指令清单（按厂商渲染）。
//!
//! 契约（与 docs/API.md 一致）：
//!   · 厂商还没识别出来时给纯 3GPP 那一份 —— 宁可少，也不要给用户看别家的私有指令；
//!   · 取不到模组也回 **code 0**（界面照样能用兜底清单），data.modem_id = -1。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::modem::at::presets_for;
use crate::modem::vendor::Vendor;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    // modem_id 缺失/不是整数：回 code 0 + 兜底清单，msg 说明原因
    let id_text = query.get("modem_id").cloned().unwrap_or_default();
    let Ok(id) = id_text.trim().parse::<u32>() else {
        return fallback(req, "modem_id must be an integer");
    };
    let Some(modem) = state.mgr.get(id) else {
        return fallback(req, &format!("modem not found: {id}"));
    };

    let vendor = modem.vendor();
    let data = serde_json::json!({
        "modem_id": modem.id,
        "vendor": vendor.as_str(),
        "vendor_name": vendor.label(),
        "presets": presets_for(vendor),
    });
    response::ok(req, data)
}

/// 没有模组时的兜底应答（未识别厂商 → 纯 3GPP 清单）。
fn fallback(req: Request, msg: &str) -> Result<()> {
    let data = serde_json::json!({
        "modem_id": -1,
        "vendor": Vendor::Unknown.as_str(),
        "vendor_name": Vendor::Unknown.label(),
        "presets": presets_for(Vendor::Unknown),
    });
    response::ok_msg(req, data, msg)
}

#[cfg(test)]
mod tests {
    use super::presets_for;
    use crate::modem::vendor::Vendor;

    #[test]
    fn fallback_is_common_only() {
        let v = Vendor::Unknown;
        let list = presets_for(v);
        assert_eq!(v.as_str(), "unknown");
        assert_eq!(v.label(), "未识别");
        assert!(!list.is_empty());
    }
}
