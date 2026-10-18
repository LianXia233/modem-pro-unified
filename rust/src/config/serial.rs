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
    pub usb_autobind: bool,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            baud: 115200,
            at_probe_timeout: "4s".to_string(),
            cmd_timeout: "3s".to_string(),
            usb_autobind: true,
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
