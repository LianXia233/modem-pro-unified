//! 美格智能 MeiG。
//!
//! 识别关键字 `MEIG` / `SRM8*` / `SLM7*`。QMI 拨号器是 `meig-cm`。

use crate::modem::at::AtPreset;

use super::{any_line, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Meig
    }

    fn label(&self) -> &'static str {
        "美格智能 MeiG"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["MEIG", "SRM8", "SLM7"])
    }

    fn dialer_bin(&self) -> &'static str {
        "meig-cm"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("美格（本模块）", "ATI", "模组型号（美格支持 ATI）"),
            of("美格（本模块）", "AT+QNWINFO", "网络接入信息"),
            of(
                "美格（本模块）",
                "AT+QENG=\"servingcell\"",
                "服务小区（含 NR 信号）",
            ),
            of("美格（本模块）", "AT+QENG=\"neighbourcell\"", "邻区列表"),
            of("美格（本模块）", "AT+QCAINFO", "载波聚合分量载波"),
            of(
                "美格（本模块）",
                "AT+QCSQ",
                "信号质量（部分固件用它替代 QENG）",
            ),
            of(
                "美格（本模块）",
                "AT+CPSI?",
                "服务小区（部分固件用它替代 QENG）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+QNWINFO",
            "AT+QENG=\"servingcell\"",
            "AT+QCSQ",
            "AT+CPSI?",
        ]
    }

    fn traffic_candidates(&self) -> &'static [&'static str] {
        &["AT+QGDCNT?"]
    }
    fn mode_read(&self) -> (&'static str, &'static str) {
        ("AT+QNWPREFCFG=\"mode_pref\"", "qcf")
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
