//! `GET /api/modules` —— 支持模块清单（91 个型号）与逐型号 AT 指令面。
//!
//! | 请求 | 返回 |
//! | --- | --- |
//! | `/api/modules` | `{count, usb_count, pcie_count, vendor_count, vendors, models}` |
//! | `/api/modules?model=RM500Q-GL` | `{plan}`：按型号离线推导 |
//! | `/api/modules?modem_id=0` | `{plan}`：按在网模组，`live=true` |
//!
//! 清单来自 `modem/catalog.rs`（QModem 91 型号）；指令面由 `modem/atplan.rs`
//! 装配、各厂商模块（`modem/vendor/<厂商>.rs`）声明自己的指令。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::modem::{atplan, catalog};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    // ① 在网模组：用实测型号 + 厂商
    if query.contains_key("modem_id") {
        let Some(modem) = modem_by_query(state, query) else {
            return response::fail(req, -1, "modem not found");
        };
        return response::ok(
            req,
            serde_json::json!({ "plan": atplan::for_modem(&modem) }),
        );
    }

    // ② 按型号：离线推导
    if let Some(model) = query
        .get("model")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return response::ok(
            req,
            serde_json::json!({ "plan": atplan::for_model(model, None, false) }),
        );
    }

    // ③ 全量清单（不带 AT 明细：91 个型号的计划太大，点开哪台再单独拉 ②）
    let (count, usb_count, pcie_count, vendor_count) = catalog::stats();
    let vendors: Vec<serde_json::Value> = catalog::vendor_counts()
        .into_iter()
        .map(|(vendor, name, count)| {
            serde_json::json!({ "vendor": vendor, "name": name, "count": count })
        })
        .collect();
    let models: Vec<serde_json::Value> = catalog::entries()
        .iter()
        .map(|e| {
            serde_json::json!({
                "vendor": e.vendor,
                "vendor_name": e.vendor_label(),
                "model": e.model,
                "platform": e.platform,
                "usb_modes": e.usb,
                "pcie_modes": e.pcie,
                "usb_text": e.usb_text(),
                "pcie_text": e.pcie_text(),
            })
        })
        .collect();
    response::ok(
        req,
        serde_json::json!({
            "count": count,
            "usb_count": usb_count,
            "pcie_count": pcie_count,
            "vendor_count": vendor_count,
            "vendors": vendors,
            "models": models,
        }),
    )
}
