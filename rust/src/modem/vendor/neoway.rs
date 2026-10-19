//! 有方 Neoway（`N510M` 等）。
//!
//! 依据上游 `module/neoway`（`cell.go` / `lock.go` / `mode.go`）与 `vendor_neoway.sh`：
//! - 服务小区：`AT+NETDMSGEX`（制式串 + MCC/MNC + 频段 + 频点 + 信号，逐格 awk 取证）；
//! - 组网模式：`AT$MYSYSINFO=<code>`；
//! - 锁频段：`AT+NWSETBAND?`（**没有** `=?` 能力查询形态）。
//!
//! ⚠️ `NETDMSGEX` 的字段顺序随制式变化（NR/LTE/WCDMA 三套），解析未移植：
//! 硬套一套下标会把 WCDMA 的 RSCP 当成 RSRP。宁可空。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Neoway
    }

    fn label(&self) -> &'static str {
        "有方 Neoway"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["NEOWAY", "N510M", "N720", "N58"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT+NETDMSGEX"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT+NETDMSGEX"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("有方（本模块）", "ATI", "模组型号"),
            of(
                "有方（本模块）",
                "AT+NETDMSGEX",
                "服务小区 + 信号（字段随制式变化）",
            ),
            of("有方（本模块）", "AT+CSQ", "信号强度（RSSI）"),
            of("有方（本模块）", "AT+CESQ", "扩展信号（RSRQ/RSRP/SINR）"),
            of(
                "锁网（有方）",
                "AT+NWSETBAND?",
                "读锁频段（无 =? 能力查询形态）",
            ),
            of("组网模式（有方）", "AT$MYSYSINFO?", "读系统信息（制式）"),
            of(
                "设置（谨慎·有方）",
                "AT$MYSYSINFO=9",
                "设为 LTE only（示例值；参数表未逐档取证）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &["AT+NETDMSGEX", "AT+CESQ", "AT+NWSETBAND?", "AT$MYSYSINFO?"]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("有方没有可写的 USB 网卡模式指令（只能查看）")
    }

    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::{add_from_vendor, replace, AtCmdSpec};
        let mut v = crate::modem::atplan::base_specs(ctx);

        add_from_vendor(&mut v, ctx, "lock_band", "锁频段", "nwsetband", "model::lock::parse_nwsetband", "NWSETBAND 枚举");
        add_from_vendor(&mut v, ctx, "signal", "信号（CESQ）", "cesq", "model::signal::parse_cesq", "rsrq/rsrp");
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
