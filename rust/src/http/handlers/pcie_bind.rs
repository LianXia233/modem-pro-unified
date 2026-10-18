//! `POST /api/pcie/bind` —— 给「在但没绑驱动」的 PCIe 模组手动绑驱动。
//!
//! `{"bdf":"0000:01:00.0","driver":"mhi_pci_generic"}`；
//! `driver` 不填就按已知模组驱动挨个试（哪个装了就绑哪个）。

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
    let drv = params
        .get("driver")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if drv.is_empty() {
        let mut last_err = String::new();
        for d in pcidev::PCIE_MODEM_DRIVERS {
            match pcidev::bind_pci_driver(&bdf, d) {
                Ok(()) => {
                    return response::ok_msg(req, json!({ "bdf": bdf, "driver": d }), "驱动已绑定");
                }
                Err(e) => last_err = e,
            }
        }
        let tail = if last_err.is_empty() {
            "未知错误".to_string()
        } else {
            last_err
        };
        return response::fail(req, -1, &format!("没有可用的 PCIe 模组驱动：{tail}"));
    }

    if let Err(e) = pcidev::bind_pci_driver(&bdf, &drv) {
        return response::fail(req, -1, &e);
    }
    response::ok_msg(req, json!({ "bdf": bdf, "driver": drv }), "驱动已绑定")
}
