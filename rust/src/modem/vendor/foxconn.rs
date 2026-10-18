//! 富士康 Foxconn（`T77W968` / `T99W175` / `T99W373` / `T99W368` / `T99W640`）。
//!
//! 依据上游 `module/foxconn`（`imei.go` / `lock.go` / `mode.go` / `usb.go`）：
//! - 服务小区：`AT^DEBUG`（与 Thales 同源）；
//! - 组网模式：`AT!SLMODE=<code>`；
//! - USB 模式：`AT!USBSWITCH=<v>`；
//! - 锁频段：通过 `AT!nv=550,...` 写 NV（`;` 分隔的多条指令，**高危**）。
//!
//! ⚠️ 锁频段的 NV 写法（`AT!nv=550,"0";AT!nv=550,9,"<逗号列表>"`）不生成预设：
//! 它一次改两条 NV 且没有回读确认，误发会把频段配置写坏。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Foxconn
    }

    fn label(&self) -> &'static str {
        "富士康 Foxconn"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(
            lines,
            &["FOXCONN", "T77W968", "T99W175", "T99W373", "T99W640"],
        )
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT^DEBUG"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT^DEBUG"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("富士康（本模块）", "ATI", "模组型号"),
            of("富士康（本模块）", "AT^DEBUG", "服务小区 + 邻区调试转储"),
            of("富士康（本模块）", "AT+CESQ", "扩展信号"),
            of("组网模式（富士康）", "AT!SLMODE?", "读组网模式"),
            of(
                "组网模式（富士康）",
                "AT!SLMODE=9",
                "自动（示例值，参数表未逐档取证）",
            ),
            of("USB（富士康）", "AT!USBSWITCH?", "读 USB 模式"),
            of(
                "锁网（富士康·高危）",
                "AT!nv=550,9,\"<频段逗号列表>\"",
                "模板：写频段 NV。⚠️ 没有回读确认、误发会写坏频段配置，只在明确知道当前值时用",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &["AT^DEBUG", "AT!SLMODE?", "AT!USBSWITCH?", "AT+CESQ"]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("富士康的 USB 切换（AT!USBSWITCH）未逐档取证，只支持查看")
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
