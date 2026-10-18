//! 鼎桥 TD Tech（MT5700 系列）。
//!
//! 识别关键字 `MT5700`。网络/信号/温度/流量全在 `^` 私有命令里
//! （移远那套全回 ERROR），所以拨号器也回落 quectel-CM。

use crate::modem::at::AtPreset;

use super::{any_line, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::TDTech
    }

    fn label(&self) -> &'static str {
        "鼎桥 TD Tech"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["MT5700"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT^MONSC"
    }

    fn net_info_cmd(&self) -> &'static str {
        // 没有 AT+QNWINFO，用它私有的 AT^SYSINFOEX
        "AT^SYSINFOEX"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("鼎桥（本模块）", "ATI", "模组型号（鼎桥支持 ATI）"),
            of(
                "鼎桥（本模块）",
                "AT^MONSC",
                "服务小区（NR/LTE，直接给 dB 值）",
            ),
            of("鼎桥（本模块）", "AT^MONNC", "邻区列表"),
            of("鼎桥（本模块）", "AT^HCSQ?", "信号强度（索引形式）"),
            of("鼎桥（本模块）", "AT^SYSINFOEX", "链路制式/系统信息"),
            of(
                "鼎桥（本模块）",
                "AT^HFREQINFO?",
                "工作频率（频段/ARFCN/带宽）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^ICCID?",
                "ICCID（AT+CCID 在本固件回 ERROR）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^CHIPTEMP?",
                "温度（12 路传感器，0.1 ℃）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^DSFLOWQRY",
                "流量统计（累计收发字节，十六进制）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^DSAMBR=1",
                "签约速率 AMBR（kbps，只支持 cid 1）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT^MONSC",
            "AT^MONNC",
            "AT^HCSQ?",
            "AT^SYSINFOEX",
            "AT^HFREQINFO?",
            "AT^ICCID?",
            "AT^CHIPTEMP?",
            "AT^DSFLOWQRY",
            "AT^DSAMBR=1",
        ]
    }

    fn mode_read(&self) -> (&'static str, &'static str) {
        // acqorder 双向映射（见 tdtech_acq_order / parse_syscfgex）
        ("AT^SYSCFGEX?", "syscfgex")
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
