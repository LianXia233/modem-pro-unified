//! 移远 Quectel。
//!
//! 识别关键字 `QUECTEL`；QMI 初始化走 `quectel-CM`。
//! QENG / QNWCFG / QNWLOCK 这一族都是移远私有的（其它厂商不一定认）。

use crate::modem::at::AtPreset;

use super::{any_line, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Quectel
    }

    fn label(&self) -> &'static str {
        "移远 Quectel"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["QUECTEL"])
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("移远（本模块）", "ATI", "模组型号（移远支持 ATI）"),
            of(
                "移远（本模块）",
                "AT+QNWINFO",
                "网络接入信息（制式/频段/信道）",
            ),
            of(
                "移远（本模块）",
                "AT+QENG=\"servingcell\"",
                "服务小区（含 NR 信号）",
            ),
            of("移远（本模块）", "AT+QENG=\"neighbourcell\"", "邻区列表"),
            of("移远（本模块）", "AT+QCAINFO", "载波聚合分量载波"),
            of("移远（本模块）", "AT+QGDCNT?", "流量计数器（累计字节）"),
            of(
                "移远（本模块）",
                "AT+QNWCFG=\"nr5g_ambr\"",
                "签约速率 AMBR + CQI",
            ),
            of("移远（本模块）", "AT+QTEMP", "温度传感器"),
            of("移远（本模块）", "AT+QCFG=\"usbnet\"", "USB 网卡模式"),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"mode_pref\"",
                "组网模式偏好（当前值）",
            ),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"policy_band\"",
                "允许使用的频段（解锁时的还原依据）",
            ),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"lte_band\"",
                "LTE 锁频段（当前值）",
            ),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"nr5g_band\"",
                "NR 锁频段（当前值）",
            ),
            of("锁网（移远）", "AT+QNWLOCK=\"common/4g\"", "LTE 锁小区状态"),
            of("锁网（移远）", "AT+QNWLOCK=\"common/5g\"", "NR 锁小区状态"),
            of(
                "锁网（移远）",
                "AT+QNWLOCK=\"common/4g\",0",
                "解锁 LTE 小区",
            ),
            of("锁网（移远）", "AT+QNWLOCK=\"common/5g\",0", "解锁 NR 小区"),
            of(
                "设置（谨慎）",
                "AT+QCFG=\"usbnet\",2",
                "切 MBIM 网卡模式（需重启模组）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+QNWINFO",
            "AT+QENG=\"servingcell\"",
            "AT+QENG=\"neighbourcell\"",
            "AT+QCAINFO",
            "AT+QCSQ",
            "AT+QTEMP",
            "AT+QGDCNT?",
        ]
    }

    fn traffic_candidates(&self) -> &'static [&'static str] {
        &["AT+QGDCNT?"]
    }

    fn temperature_cmds(&self, _mtk: bool) -> &'static [&'static str] {
        &["AT+QTEMP"]
    }
    fn mode_read(&self) -> (&'static str, &'static str) {
        ("AT+QNWPREFCFG=\"mode_pref\"", "qcf")
    }
}

/// 预设表构造（同 `at/presets.rs` 的写法，保持表可读）。
fn of(group: &'static str, cmd: &'static str, desc: &'static str) -> AtPreset {
    AtPreset {
        group,
        cmd,
        desc,
        tpl: false,
    }
}
