//! `POST /api/usb/bind` —— 给「插上了但没串口」的模组手动补一次驱动。
//!
//! `{"vid":"0e8d","pid":"7126"}`；自动补失败时的补救路径，或设备没被自动认出时手填。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};
use crate::modem::usbdev::{self, UsbEvent};

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let vid = params
        .get("vid")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let pid = params
        .get("pid")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if vid.len() != 4 || pid.len() != 4 {
        return response::fail(
            req,
            -1,
            "vid / pid 要写成四位十六进制，例如 {\"vid\":\"0e8d\",\"pid\":\"7126\"}",
        );
    }
    if !usbdev::can_bind() {
        return response::fail(req, -1, usbdev::bind_note());
    }

    let device = format!("{vid}:{pid}");
    // 用清单里的设备信息（拿不到就用 vid/pid 兜底）
    let dev = usbdev::list_modem_usb_devices()
        .into_iter()
        .find(|d| d.vid == vid && d.pid == pid)
        .unwrap_or_else(|| usbdev::ModemUsbDevice {
            vid: vid.clone(),
            pid: pid.clone(),
            ..Default::default()
        });

    match usbdev::ensure_serial_tty(&dev) {
        Ok(note) => {
            usbdev::note_usb_event(UsbEvent {
                at: String::new(),
                kind: usbdev::USB_EVENT_BOUND.to_string(),
                device,
                model: String::new(),
                note: format!("{note}（手动）"),
                modem_id: None,
                ports: Vec::new(),
            });
            super::usb_clear::push_usb(state);
            response::ok_msg(req, json!({ "note": note }), "ok")
        }
        Err(e) => {
            usbdev::note_usb_event(UsbEvent {
                at: String::new(),
                kind: usbdev::USB_EVENT_FAILED.to_string(),
                device,
                model: String::new(),
                note: format!("手动补驱动失败：{e}"),
                modem_id: None,
                ports: Vec::new(),
            });
            response::fail(req, -1, &e)
        }
    }
}
