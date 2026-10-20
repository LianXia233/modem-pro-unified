//! `[serial]` 段：AT 串口参数与超时。

use std::time::Duration;

use serde::Deserialize;

use super::parse_duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SerialConfig {
    pub baud: u32,
    /// 形如 "4s" 的时长字符串（空/0 用默认）。
    pub at_probe_timeout: String,
    pub cmd_timeout: String,
    /// USB 模组没有串口时，自动往 `option1/new_id` 补绑（QModem 同口径）。
    pub usb_autobind: bool,
    /// PCIe 模组没绑驱动时，自动尝试 MHI / T7xx 驱动。
    ///
    /// 默认开：PCIe 模组不绑驱动就没有 AT 口与网卡，等于不存在；
    /// 绑错驱动只会让设备暂时不可用，重扫/重启即可恢复。
    pub pcie_autobind: bool,
    /// 「一台模组都认不出」持续 150s 后，做一次 USB 重枚举 / PCIe 重扫。
    ///
    /// **默认关**：这是破坏性操作（USB `authorized 0→1` 会短暂断开设备）。
    /// 上游 Go 版默认自动执行；本项目要求显式打开，且带 5 分钟冷却、每轮最多 3 次。
    pub auto_rescue: bool,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            baud: 115200,
            at_probe_timeout: "4s".to_string(),
            cmd_timeout: "3s".to_string(),
            usb_autobind: true,
            pcie_autobind: true,
            auto_rescue: false,
        }
    }
}

impl SerialConfig {
    pub fn probe_timeout(&self) -> Duration {
        parse_duration(&self.at_probe_timeout, Duration::from_secs(4))
    }

    pub fn command_timeout(&self) -> Duration {
        parse_duration(&self.cmd_timeout, Duration::from_secs(3))
    }

    pub(super) fn normalize(&mut self) {
        if self.baud == 0 {
            self.baud = 115200;
        }
    }
}
