//! 广和通 Fibocom（含 FM350 / RW350 的 MTK 平台差异）。
//!
//! 识别关键字 `FIBOCOM` / `FM650`。高通平台走 QNW 那套；
//! MTK 平台（FM350）只有 `AT+GTCCINFO?` / `AT+GTSENRDTEMP=0` 这一族。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Fibocom
    }

    fn label(&self) -> &'static str {
        "广和通 Fibocom"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["FIBOCOM", "FM650", "RW350"])
    }

    fn dialer_bin(&self) -> &'static str {
        "fibocom-dial"
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT+QNWINFO"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("广和通（本模块）", "ATI", "模组型号（广和通支持 ATI）"),
            of("广和通（本模块）", "AT+QNWINFO", "网络接入信息"),
            of(
                "广和通（本模块）",
                "AT+FENG=\"servingcell\"",
                "服务小区（FM150/FM160 高通平台）",
            ),
            of(
                "广和通（本模块）",
                "AT+FENG=\"neighbourcell\"",
                "邻区（FM150/FM160 高通平台）",
            ),
            of(
                "广和通（本模块）",
                "AT+GTCCINFO?",
                "服务小区+邻区+信号（FM650-CN 展锐平台机型）",
            ),
            of(
                "广和通（本模块）",
                "AT+GTSTATIS?",
                "流量统计（瞬时速率 Byte/s）",
            ),
            of(
                "广和通（MTK 平台 FM350）",
                "AT+GTSENRDTEMP=0",
                "模组各路温度（0.001 ℃；读形式在本平台回 phone failure）",
            ),
            of(
                "广和通（MTK 平台 FM350）",
                "AT+EMBIND?",
                "数据通道绑定状态（决定数据去主机侧还是模组自身）",
            ),
            of(
                "广和通（MTK 平台 FM350）",
                "AT+EMBIND=?",
                "本固件支持的 L2P 与 cid 范围",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWPREFCFG=\"mode_pref\"",
                "组网模式偏好（当前值）",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWPREFCFG=\"policy_band\"",
                "允许使用的频段（解锁时的还原依据）",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWPREFCFG=\"lte_band\"",
                "LTE 锁频段（当前值）",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWPREFCFG=\"nr5g_band\"",
                "NR 锁频段（当前值）",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWLOCK=\"common/4g\"",
                "LTE 锁小区状态",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWLOCK=\"common/5g\"",
                "NR 锁小区状态",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWLOCK=\"common/4g\",0",
                "解锁 LTE 小区",
            ),
            of(
                "锁网（广和通）",
                "AT+QNWLOCK=\"common/5g\",0",
                "解锁 NR 小区",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+QNWINFO",
            "AT+GTCCINFO?",
            "AT+GTCAINFO?",
            "AT+QENG=\"servingcell\"",
        ]
    }

    fn traffic_candidates(&self) -> &'static [&'static str] {
        &["AT+GTSTATIS?"]
    }

    fn temperature_cmds(&self, mtk: bool) -> &'static [&'static str] {
        // MTK 平台只有带参数的写形式能读（读形式回 phone failure）
        if mtk {
            &["AT+GTSENRDTEMP=0"]
        } else {
            &[]
        }
    }

    fn usbnet_write(&self, value: u8) -> UsbnetWrite {
        if value == 3 {
            // 切 RNDIS 会整机掉 USB（实测），不许从这里动
            return UsbnetWrite::Deny("广和通 RNDIS 不允许在这里执行（切过去会整机掉 USB）");
        }
        UsbnetWrite::Deny("该厂商没有可写的 USB 网卡模式指令（只能查看）")
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
