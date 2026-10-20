//! 芯讯通 SIMCom。
//!
//! 识别关键字 `SIMCOM` / `SIM7900`。模式用 `AT+CNMP`、温度用 `AT+CPMUTEMP`、
//! 主动扫小区只有它实测支持（`AT+CNWSEARCH`）。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Simcom
    }

    fn label(&self) -> &'static str {
        "芯讯通 SIMCom"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["SIMCOM", "SIM7900"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT+CPSI?"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT+QNWINFO"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("芯讯通（本模块）", "ATI", "模组型号（芯讯通支持 ATI）"),
            of("芯讯通（本模块）", "AT+CPSI?", "服务小区（含 NR 信号）"),
            of(
                "芯讯通（本模块）",
                "AT+CNWINFO?",
                "附加网络信息（带宽/调制/CQI/上行功率）",
            ),
            of(
                "芯讯通（本模块）",
                "AT+CPMUTEMP",
                "模组温度（不是 AT+QTEMP，那个在本固件回 ERROR）",
            ),
            of("芯讯通（本模块）", "AT+CBC", "供电电压"),
            of("芯讯通（本模块）", "AT+QNWINFO", "网络接入信息"),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &["AT+CPSI?", "AT+QNWINFO", "AT+CPMUTEMP"]
    }

    fn temperature_cmds(&self, _mtk: bool) -> &'static [&'static str] {
        &["AT+CPMUTEMP"]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        UsbnetWrite::Deny("该厂商没有可写的 USB 网卡模式指令（只能查看）")
    }

    fn mode_read(&self) -> (&'static str, &'static str) {
        ("AT+CNMP?", "cnmp")
    }

    fn cell_scan_cmd(&self) -> Option<&'static str> {
        // 实测可用的只有它；别的固件发过去只会白占串口几十秒
        Some("AT+CNWSEARCH=1")
    }

    /// 芯讯通：**只有** 3GPP 的 `AT+CGACT`，没有私有激活指令。
    ///
    /// 高通平台的 QModem 实现会先读 `AT+CNMP?` 再发 `AT+CNMP=<原值>;+CNWINFO=1`
    /// 保活 —— 这个值随卡/固件变化，写死会改掉用户的网络制式偏好，
    /// 所以这里不发，只在 note 里说明差异。
    fn data_channel(
        &self,
        ctx: &super::datacall::DataCallCtx<'_>,
    ) -> Option<super::datacall::DataChannel> {
        use super::datacall::{cgact_deactivate, DataChannel};
        let _ = ctx.platform;
        Some(DataChannel {
            pdp_first: true,
            activate: Vec::new(),
            deactivate: vec![cgact_deactivate(ctx.cid)],
            cgact_fallback: true,
            autodial_probe: None,
            note: "芯讯通只有 3GPP CGACT；高通平台的 CNMP 保活值随卡变化，不写死",
        })
    }

    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::{add_from_vendor, replace, AtCmdSpec};
        let mut v = crate::modem::atplan::base_specs(ctx);

        replace(
            &mut v,
            AtCmdSpec::on(
                "lock_band",
                "锁频段",
                "AT+CNBP?",
                "model::lock::parse_cnbp",
                "位图（需先解锁 AT+CNBP=0）",
                "厂商指令集",
            ),
        );
        replace(
            &mut v,
            AtCmdSpec::off(
                "imei_set",
                "IMEI 改写",
                "本厂商未提供可验证的 IMEI 写指令（只读）",
            ),
        );
        add_from_vendor(
            &mut v,
            ctx,
            "signal",
            "信号（CPSI）",
            "cpsi",
            "model::cells::parse_cpsi",
            "系统信息（含信号）",
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
