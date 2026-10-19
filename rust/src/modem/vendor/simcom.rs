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
}

fn of(group: &'static str, cmd: &'static str, desc: &'static str) -> AtPreset {
    AtPreset {
        group,
        cmd,
        desc,
        tpl: false,
    }
}
