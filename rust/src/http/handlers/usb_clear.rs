//! `POST /api/usb/clear` —— 清空 USB 热插拔事件（不动设备）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

pub fn handle(req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    crate::modem::usbdev::clear_usb_events();
    push_usb(state);
    response::ok_msg(req, serde_json::Value::Null, "ok")
}

/// 把 USB 视图推给在线的客户端（其它页面也跟着刷新）。
pub fn push_usb(state: &AppState) {
    let view = crate::modem::usbdev::usb_hotplug_info(state.cfg.serial.usb_autobind);
    let data = serde_json::to_value(view).unwrap_or(serde_json::Value::Null);
    state.ws.broadcast("usb", None, data);
}
