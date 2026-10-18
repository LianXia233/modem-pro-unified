//! 高新兴 Gosuncn（`ME3630-W` / `GM800`）。
//!
//! 依据上游 `module/gosuncn`（`cell.go` / `lock.go` / `mode.go` / `usb.go`）：
//! - 服务小区：`AT+ZCELLINFO?`（两种平台都认；信号另发 `AT+CESQ`）；
//! - 锁频段：`AT+ZBAND?`（读）/ `AT+ZBAND=?`（能力）；**没有** `=0` 解锁形态；
//! - 锁小区：`AT+ZLOCKCELL?`（读）/ `AT+ZLOCKCELL=0`（解）；
//! - 组网模式：`AT+ZSNT=<code>,0,0`（后两段是 2G/3G 开关）；
//! - USB 模式：`AT+ZSWITCH=<v>`。
//!
//! ⚠️ `AT+ZCELLINFO?` 有 LTE / 非 LTE 两种行形状，解析未移植。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Gosuncn
    }

    fn label(&self) -> &'static str {
        "高新兴 Gosuncn"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["GOSUNCN", "ME3630", "GM800", "ZM8620"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT+ZCELLINFO?"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT+ZCELLINFO?"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("高新兴（本模块）", "ATI", "模组型号"),
            of(
                "高新兴（本模块）",
                "AT+ZCELLINFO?",
                "服务小区（LTE / 非 LTE 两种行形状）",
            ),
            of("高新兴（本模块）", "AT+CSQ", "信号强度"),
            of("高新兴（本模块）", "AT+CESQ", "扩展信号"),
            of("锁网（高新兴）", "AT+ZBAND?", "读锁频段"),
            of(
                "锁网（高新兴）",
                "AT+ZBAND=?",
                "锁频段能力（支持的频段列表）",
            ),
            of(
                "锁网（高新兴）",
                "AT+ZLOCKCELL?",
                "读锁小区；`AT+ZLOCKCELL=0` 解除",
            ),
            of("组网模式（高新兴）", "AT+ZSNT?", "读组网模式"),
            of(
                "组网模式（高新兴）",
                "AT+ZSNT=0,0,0",
                "自动（后两段是 2G/3G 开关）",
            ),
            of("组网模式（高新兴）", "AT+ZSNT=2,0,0", "仅 LTE"),
            of("USB（高新兴）", "AT+ZSWITCH?", "读 USB 模式"),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+ZCELLINFO?",
            "AT+ZBAND?",
            "AT+ZLOCKCELL?",
            "AT+ZSNT?",
            "AT+ZSWITCH?",
        ]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("高新兴的 USB 切换（AT+ZSWITCH）未逐档取证，只支持查看")
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
