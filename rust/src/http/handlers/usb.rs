//! `GET /api/usb` —— USB 热插拔视图：模组 USB 设备清单 + 最近事件 + 补驱动能力。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

pub fn handle(req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    let view = crate::modem::usbdev::usb_hotplug_info(state.cfg.serial.usb_autobind);
    response::ok(req, serde_json::to_value(view)?)
}
