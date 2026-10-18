//! 华为 Huawei（`s10` / `MT5710` / `MH5000-*`）。
//!
//! 依据上游 `module/huawei`（只有 `mode.go` / `usb.go` / `imei.go`，没有 cell 解析）
//! 与 `vendor_huawei.sh`：
//! - 组网模式：`AT^SYSCFGEX`（acqorder 口径，`"00"` = 自动）；
//! - USB 模式：`AT^SETMODE`；
//! - 服务小区：`AT^SYSINFOEX`（华为经典，**不带** QENG）。
//!
//! ⚠️ 小区字段解析未移植：上游没有 huawei/cell.go，说明它靠 QModem 脚本的
//! awk 口径，没有 Go 实现可抄。宁可让 `/api/cells` 回空，也不猜字段顺序。
//!
//! ⚠️ **不认 `MT5700M`**：它虽然在上游 catalog 里挂在华为名下，但鼎桥
//! （TD Tech）的关键字就是 `MT5700`，两家共线。本项目的判定顺序里鼎桥在前
//! （见 `DETECT_ORDER`），所以这里不抢 —— 抢了会把鼎桥模组按华为的 `^` 族发指令，
//! 而鼎桥实际认的是自己那套（`AT^MONSC` / `AT^CHIPTEMP`）。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Huawei
    }

    fn label(&self) -> &'static str {
        "华为 Huawei"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["HUAWEI", "MH5000", "MT5710"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT^SYSINFOEX"
    }

    fn net_info_cmd(&self) -> &'static str {
        // 华为没有 AT+QNWINFO；系统信息走 ^SYSINFOEX
        "AT^SYSINFOEX"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("华为（本模块）", "ATI", "模组型号"),
            of(
                "华为（本模块）",
                "AT^SYSINFOEX",
                "系统/链路信息（制式、业务状态）",
            ),
            of("华为（本模块）", "AT^HCSQ?", "信号强度（索引形式，需查表）"),
            of("华为（本模块）", "AT^SYSINFO", "系统信息（老固件）"),
            of(
                "组网模式（华为）",
                "AT^SYSCFGEX?",
                "读 acqorder 配置（第 1 段是制式优先级）",
            ),
            of(
                "组网模式（华为）",
                "AT^SYSCFGEX=\"00\",40000000,1,2,40000000,,",
                "设为自动（各制式按优先级搜索）",
            ),
            of(
                "组网模式（华为）",
                "AT^SYSCFGEX=\"03\",40000000,1,2,40000000,,",
                "仅 LTE",
            ),
            of("USB（华为）", "AT^SETMODE?", "读 USB 模式"),
            of("IMEI（华为）", "AT^PHYNUM=IMEI", "读 IMEI（华为私有）"),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &["AT^SYSINFOEX", "AT^HCSQ?", "AT^SYSCFGEX?", "AT^SETMODE?"]
    }

    fn usbnet_write(&self, _value: u8) -> UsbnetWrite {
        // AT^SETMODE 的参数取值没有逐档取证（切错会掉 USB），只开放查询
        UsbnetWrite::Deny("华为的 USB 模式切换（AT^SETMODE）未逐档取证，只支持查看")
    }
    fn mode_read(&self) -> (&'static str, &'static str) {
        // acqorder 口径：`^SYSCFGEX: "03",...` 的第 1 段决定制式集合
        ("AT^SYSCFGEX?", "huawei_acq")
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
