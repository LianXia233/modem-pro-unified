//! `[server]` 段：监听地址与各后台任务节拍。

use serde::Deserialize;

use super::loader;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// API 监听地址（LuCI 经它代理请求），如 "0.0.0.0:9527"。
    pub listen: String,
    /// 后台热插拔巡检间隔（秒）。<=0 关闭（只在启动时发现一次模组）。
    pub scan_interval: i64,
    pub pid_file: String,
    pub log_file: String,
    pub log_level: String,
    pub push_interval: i64,
    pub network_interval: i64,
    pub traffic_interval: i64,
    pub change_interval: i64,
    /// 定时任务循环的节拍（秒）。<=0 按 10 秒兜底。
    pub schedule_interval: i64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:9527".to_string(),
            scan_interval: 3,
            // 平台相关路径走 loader 的判定（unix 用 /var/...，其余用当前目录），
            // 这里不能硬编码：Windows 上写 /var/log 会直接失败
            pid_file: loader::default_pid_file(),
            log_file: loader::default_log_file(),
            log_level: "info".to_string(),
            push_interval: 1,
            network_interval: 3,
            traffic_interval: 1,
            change_interval: 0,
            schedule_interval: 10,
        }
    }
}

impl ServerConfig {
    pub(super) fn normalize(&mut self) {
        if self.listen.trim().is_empty() {
            self.listen = "0.0.0.0:9527".to_string();
        }
        if self.scan_interval < 0 {
            self.scan_interval = 0;
        }
        if self.traffic_interval <= 0 {
            self.traffic_interval = 1;
        }
        if self.network_interval <= 0 {
            self.network_interval = 3;
        }
        if self.log_file.trim().is_empty() {
            self.log_file = loader::default_log_file();
        }
        if self.pid_file.trim().is_empty() {
            self.pid_file = loader::default_pid_file();
        }
    }
}
