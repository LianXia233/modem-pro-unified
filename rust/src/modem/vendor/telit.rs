//! Telit（`FN990A40` 等）。
//!
//! 依据上游 `module/telit`（`cell.go` / `lock.go` / `mode.go` / `usb.go`）：
//! - 服务小区：`AT#CAINFOEXT?`（**同时**含主载波与辅载波 —— 辅载波是分量载波、
//!   不是独立小区，解析时必须区分，否则小区数会翻倍）；
//! - 组网模式：`AT+WS46=<code>`（3GPP 标准 WS46 表）；
//! - 锁频段：`AT#BND?`；
//! - USB 模式：`AT#USBCFG=<v>`。
//!
//! `#CAINFOEXT` 的字段布局未移植（上游是逐格解析，混合主/辅载波），见模块说明。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Telit
    }

    fn label(&self) -> &'static str {
        "Telit"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["TELIT", "FN990", "LN920", "LM960"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT#CAINFOEXT?"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT#CAINFOEXT?"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("Telit（本模块）", "ATI", "模组型号"),
            of(
                "Telit（本模块）",
                "AT#CAINFOEXT?",
                "主载波 + 辅载波（CA）信息",
            ),
            of("Telit（本模块）", "AT#RFSTS", "射频状态（制式/频段/信号）"),
            of("Telit（本模块）", "AT+CESQ", "扩展信号"),
            of("锁网（Telit）", "AT#BND?", "读锁频段"),
            of("组网模式（Telit）", "AT+WS46?", "读 3GPP WS46 制式集合"),
            of("组网模式（Telit）", "AT+WS46=22", "仅 LTE（WS46=22）"),
            of("组网模式（Telit）", "AT+WS46=28", "LTE + NR5G（WS46=28）"),
            of("USB（Telit）", "AT#USBCFG?", "读 USB 配置"),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT#CAINFOEXT?",
            "AT#RFSTS",
            "AT#BND?",
            "AT+WS46?",
            "AT#USBCFG?",
        ]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("Telit 的 USB 配置（AT#USBCFG）未逐档取证，只支持查看")
    }
    fn mode_read(&self) -> (&'static str, &'static str) {
        // 3GPP WS46 数字码（22 = E-UTRAN only、28 = E-UTRAN + NR5G）
        ("AT+WS46?", "ws46")
    }

    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::add_from_vendor;
        let mut v = crate::modem::atplan::base_specs(ctx);

        add_from_vendor(&mut v, ctx, "ca", "载波聚合 CA", "cainfoext", "model::cells::parse_ca", "#CAINFOEXT");
        add_from_vendor(&mut v, ctx, "lock_band", "锁频段", "bnd", "model::lock::parse_telit_band", "AT#BND 枚举");
        add_from_vendor(&mut v, ctx, "usbnet", "USB 网卡模式", "usbcfg", "model::usbnet", "AT#USBCFG");

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
