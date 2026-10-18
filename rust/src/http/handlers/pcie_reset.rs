//! `POST /api/pcie/reset` —— PCIe 软复位（remove + rescan，不断模组的电）。
//!
//! `{"bdf":"0000:01:00.0"}`；对 USB 那边的「软重插」，驱动会自动重新绑定。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};
use crate::modem::pcidev;

pub fn handle(mut req: Request, _state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    if !matches!(req.method(), tiny_http::Method::Post) {
        return response::fail(req, -1, "method not allowed, use POST");
    }
    let params = request::merge_form(query, &request::read_body(&mut req));
    let bdf = params.get("bdf").cloned().unwrap_or_default();
    if !pcidev::reset_supported() {
        return response::fail(req, -1, "本平台不支持 PCIe 软复位");
    }
    if let Err(e) = pcidev::reset_pci_device(&bdf) {
        return response::fail(req, -1, &e);
    }
    response::ok_msg(
        req,
        json!({ "bdf": bdf }),
        "已重新枚举 PCIe 设备（驱动会自动重新绑定）",
    )
}
