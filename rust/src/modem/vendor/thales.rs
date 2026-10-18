//! Thales（`MV32-W-A` / `MV32-W-B`）。
//!
//! 依据上游 `module/thales`（`cell.go` / `lock.go` / `mode.go` / `usb.go`）：
//! - 服务小区：`AT^DEBUG`（与富士康同源，手册 §15.33）；
//! - 组网模式：`AT^SLMODE=1,<code>`；
//! - USB 模式：`AT+SETCONFIG=<v>`。
//!
//! ⚠️ `AT^DEBUG` 的字段布局未移植：它是 Thales/Foxconn 共用的调试转储，
//! 上游按「手册 §15.33」逐段解析，抄错一段就会把邻区当服务小区。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Thales
    }

    fn label(&self) -> &'static str {
        "Thales"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["THALES", "MV32", "MV31"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT^DEBUG"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT^DEBUG"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("Thales（本模块）", "ATI", "模组型号"),
            of(
                "Thales（本模块）",
                "AT^DEBUG",
                "服务小区 + 邻区调试转储（手册 §15.33）",
            ),
            of("Thales（本模块）", "AT+CESQ", "扩展信号"),
            of("组网模式（Thales）", "AT^SLMODE?", "读组网模式"),
            of(
                "组网模式（Thales）",
                "AT^SLMODE=1,1",
                "设为自动（示例值，参数表未逐档取证）",
            ),
            of("USB（Thales）", "AT+SETCONFIG?", "读 USB 配置"),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &["AT^DEBUG", "AT^SLMODE?", "AT+SETCONFIG?"]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("Thales 的 USB 配置（AT+SETCONFIG）未逐档取证，只支持查看")
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
