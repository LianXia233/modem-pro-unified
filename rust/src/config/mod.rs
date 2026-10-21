//! 配置加载：`/etc/modemd/config.toml`。
//!
//! 口径：
//!   · 键缺失时用**默认值**（config.toml 是 conffile，升级不覆盖，新键永远不出现）；
//!   · `[server] listen` 是地址，`[wechat] listen` 是布尔 —— 别混；
//!   · 「不写也算开」的开关（require_luci_login / wechat.listen）
//!     由各分节的 `Default` + `normalize()` 兜底。
//!
//! 分节各自成文件，便于按域扩展（迁移清单见 docs/INTEGRATION.md）。

mod dial;
mod duration;
mod loader;
mod net;
mod ntp;
pub mod patch;
mod quota;
mod serial;
mod sim;
mod sms;
mod ui;
pub mod wan_mode;
mod wechat;

pub mod server;

use serde::Deserialize;

pub use dial::DialConfig;
pub use duration::parse_duration;
pub use loader::{default_log_file, default_pid_file, load};
pub use net::{NetConfig, DEFAULT_WAN_DNS};
pub use ntp::NtpConfig;
pub use quota::{QuotaConfig, ACTION_BLOCK, ACTION_BLOCK_RF, ACTION_WARN};
pub use serial::SerialConfig;
pub use sim::SimConfig;
pub use sms::SmsConfig;
pub use ui::UiConfig;
pub use wan_mode::WanMode;
pub use wechat::WeChatConfig;

/// 进程级配置（对应 config.toml）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: server::ServerConfig,
    pub serial: SerialConfig,
    pub sms: SmsConfig,
    pub sim: SimConfig,
    pub net: NetConfig,
    pub ntp: NtpConfig,
    pub quota: QuotaConfig,
    pub dial: DialConfig,
    pub ui: UiConfig,
    pub wechat: WeChatConfig,
}

impl Config {
    /// 应用「不写也要有」的默认值（各分节自行归一化）。
    fn normalize(&mut self) {
        self.server.normalize();
        self.serial.normalize();
        self.net.normalize();
        self.quota.normalize();
        self.ui.normalize();
        self.wechat.normalize();
        self.ntp.normalize();
    }

    /// `[ui] require_luci_login` 的最终取值（不写也算开）。
    pub fn require_luci_login(&self) -> bool {
        self.ui.require_luci_login
    }

    /// 内置默认值 + 归一化：配置读不了时的最后兜底（见 config/loader.rs）。
    ///
    /// 方向是**收紧**：登录闸门、短信密码闸门默认都开着，不会因为配置坏了就敞开。
    pub fn default_normalized() -> Config {
        let mut cfg = Config::default();
        cfg.normalize();
        cfg
    }
}
