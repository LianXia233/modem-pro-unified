//! Sierra Wireless（`EM9190` / `EM9291` / `MC7354` / `MC7355`）。
//!
//! 依据上游 `module/sierra`（`cell.go` / `mode.go` / `usb.go`）：
//! - 服务小区 / 网络：`AT!GSTATUS?`（`!` 私有族）；
//! - 组网模式：`AT!SELRAT=<code>`；
//! - USB 模式：`AT!USBCOMP=1,4,<v>`（第一段是 USB 配置号、第二段是接口总数）。
//!
//! ⚠️ `!GSTATUS?` 是自由格式，字段随固件版本变；解析未移植。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Sierra
    }

    fn label(&self) -> &'static str {
        "Sierra Wireless"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["SIERRA", "EM9190", "EM9291", "MC7354", "MC7355"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT!GSTATUS?"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT!GSTATUS?"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("Sierra（本模块）", "ATI", "模组型号"),
            of(
                "Sierra（本模块）",
                "AT!GSTATUS?",
                "网络/信号状态（自由格式）",
            ),
            of("Sierra（本模块）", "AT!UIMS?", "SIM 状态"),
            of("组网模式（Sierra）", "AT!SELRAT?", "读 RAT 偏好"),
            of("组网模式（Sierra）", "AT!SELRAT=06", "自动（含 5G）"),
            of("组网模式（Sierra）", "AT!SELRAT=14", "仅 LTE"),
            of("USB（Sierra）", "AT!USBCOMP?", "读 USB 组合"),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &["AT!GSTATUS?", "AT!SELRAT?", "AT!USBCOMP?", "AT!UIMS?"]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("Sierra 的 USB 组合（AT!USBCOMP）未逐档取证，只支持查看")
    }
    fn mode_read(&self) -> (&'static str, &'static str) {
        // `!SELRAT: <n>`（00/06 = 自动、14 = LTE only）
        ("AT!SELRAT?", "selrat")
    }
}

fn of(group: &'static str, cmd: &'static str, desc: &'static str) -> AtPreset {
    AtPreset {
        group,
        cmd,
        desc,
        tpl: false,
    }
}
