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

    /// 高新兴 LTE 平台：`AT+ZECMCALL=1` / `=0`（QModem `modem_dial.sh:1475-1482`、`:1154-1156`）。
    fn data_channel(
        &self,
        ctx: &super::datacall::DataCallCtx<'_>,
    ) -> Option<super::datacall::DataChannel> {
        use super::datacall::{AtStep, DataChannel};
        if ctx.platform != "lte" {
            return None;
        }
        Some(DataChannel {
            pdp_first: false,
            activate: vec![AtStep::new(
                "AT+ZECMCALL=1",
                "高新兴 LTE：打开 ECM 数据呼叫",
            )],
            deactivate: vec![AtStep::new("AT+ZECMCALL=0", "关闭 ECM 数据呼叫")],
            cgact_fallback: true,
            autodial_probe: None,
            note: "高新兴 LTE 平台由 AT+ZECMCALL 激活 ECM",
        })
    }

    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::{add_from_vendor, replace, AtCmdSpec};
        let mut v = crate::modem::atplan::base_specs(ctx);

        add_from_vendor(
            &mut v,
            ctx,
            "lock_band",
            "锁频段",
            "zband",
            "model::lock::parse_zband",
            "ZBAND 频段表",
        );
        add_from_vendor(
            &mut v,
            ctx,
            "lock_cell",
            "锁小区",
            "zlockcell",
            "model::lock::parse_zcell",
            "ZLOCKCELL",
        );
        replace(
            &mut v,
            AtCmdSpec::off(
                "imei_set",
                "IMEI 改写",
                "本厂商未提供可验证的 IMEI 写指令（只读）",
            ),
        );

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
