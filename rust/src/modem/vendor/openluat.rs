//! 合宙 openluat（`Air724UG` 等）。
//!
//! 依据上游 `module/openluat`（`cell.go` / `lock.go` / `mode.go` / `usb.go`）：
//! - 服务小区：`AT+CCED=0,1`；邻区 `AT+CCED=0,2`（可多行）；
//! - 锁频段：`AT*BAND?`（`*` 私有族，**没有** `=?` 能力形态、也没有设置形态）；
//! - 组网模式：`AT+CTEC=<code>,<code>`（两段相同值）；
//! - USB 模式：`AT+SETUSB=<v>`；
//! - 拨号前预置：`AT+RNDISCALL=0,0`（关掉 RNDIS 自动拨号，否则会和主机抢）。
//!
//! `AT+RNDISCALL=0,0` 是**上游脚本明确要求**的预置动作，所以写进预设表首位。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Openluat
    }

    fn label(&self) -> &'static str {
        "合宙 openluat"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["AIR724", "AIR720", "OPENLUAT"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT+CCED=0,1"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT+CCED=0,1"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("合宙（本模块）", "ATI", "模组型号"),
            of("合宙（本模块）", "AT+CCED=0,1", "服务小区"),
            of("合宙（本模块）", "AT+CCED=0,2", "邻区（可多行）"),
            of("合宙（本模块）", "AT+CSQ", "信号强度"),
            of(
                "合宙（拨号预置）",
                "AT+RNDISCALL=0,0",
                "关掉 RNDIS 自动拨号（否则与主机抢承载）",
            ),
            of(
                "锁网（合宙）",
                "AT*BAND?",
                "读频段配置（无 =? 能力形态、无设置形态）",
            ),
            of("组网模式（合宙）", "AT+CTEC?", "读组网模式"),
            of("组网模式（合宙）", "AT+CTEC=1,1", "自动（两段同值，示例）"),
            of("USB（合宙）", "AT+SETUSB?", "读 USB 模式"),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+CCED=0,1",
            "AT*BAND?",
            "AT+CTEC?",
            "AT+SETUSB?",
            "AT+RNDISCALL?",
        ]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("合宙的 USB 切换（AT+SETUSB）未逐档取证，只支持查看")
    }

    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::{add_from_vendor, replace, AtCmdSpec};
        let mut v = crate::modem::atplan::base_specs(ctx);

        add_from_vendor(&mut v, ctx, "lock_band", "锁频段", "*band", "model::lock::parse_openluat_band", "*BAND 复合字段");
        add_from_vendor(&mut v, ctx, "lock_mode", "组网模式偏好", "ctec", "model::mode::openluat", "AT+CTEC 枚举");
        add_from_vendor(&mut v, ctx, "usbnet", "USB 网卡模式", "setusb", "model::usbnet", "AT+SETUSB 枚举");
        replace(&mut v, AtCmdSpec::off(
            "imei_set", "IMEI 改写", "本厂商未提供可验证的 IMEI 写指令（只读）"));

        v
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
