//! `[ntp]` 段：插件自带的 SNTP 校时。
//!
//! 设计要点（与 `runtime/ntp.rs` 呼应）：
//! - **不改系统时钟**，只维护一个进程内偏移 —— 这样没权限、同步失败都不影响系统；
//! - 没同步上时偏移为 0，时间自然回退到「设备时间」（要求的降级路径）；
//! - 时区在这里配（`timezone`），优先于 `TZ` / `/etc/TZ`。

use serde::Deserialize;

/// 默认服务器：腾讯 / Cloudflare / 阿里（国内可达性依次递减，但都有 IPv6）。
pub const DEFAULT_SERVERS: [&str; 3] = ["ntp.tencent.com", "time.cloudflare.com", "ntp.aliyun.com"];

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NtpConfig {
    /// 总开关。关掉后不发起任何请求，时间回退为设备时间。
    pub enabled: bool,
    /// 服务器列表（按顺序尝试，全失败才算一次失败）。
    pub servers: Vec<String>,
    /// 时区：IANA 名（`Asia/Shanghai`）或 POSIX（`CST-8`）；空 = 跟随系统。
    pub timezone: String,
    /// 单台服务器的等应答超时（秒，1~10）。
    pub timeout_secs: i64,
}

impl Default for NtpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            servers: DEFAULT_SERVERS.iter().map(|s| s.to_string()).collect(),
            timezone: "Asia/Shanghai".to_string(),
            timeout_secs: 3,
        }
    }
}

impl NtpConfig {
    /// 清洗后的服务器列表：去空、去重、限制条数（防止配置里堆一串无意义域名）。
    pub fn normalized_servers(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.servers {
            let t = s.trim();
            if t.is_empty() || out.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                continue;
            }
            out.push(t.to_string());
            if out.len() >= 8 {
                break;
            }
        }
        if out.is_empty() {
            // 空列表按默认三台（比「静默不同步」好：用户至少能看到失败日志）
            out = DEFAULT_SERVERS.iter().map(|s| s.to_string()).collect();
        }
        out
    }

    /// 归一化：超时夹到 1~10 秒，时区去空白。
    pub fn normalize(&mut self) {
        self.timeout_secs = self.timeout_secs.clamp(1, 10);
        self.timezone = self.timezone.trim().to_string();
        self.servers = self.normalized_servers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_three_public_servers() {
        let c = NtpConfig::default();
        assert!(c.enabled);
        assert_eq!(c.servers.len(), 3);
        assert!(c.servers.iter().any(|s| s.contains("tencent")));
        assert!(c.servers.iter().any(|s| s.contains("cloudflare")));
        assert!(c.servers.iter().any(|s| s.contains("aliyun")));
    }

    #[test]
    fn empty_server_list_falls_back_to_defaults() {
        let c = NtpConfig {
            servers: vec!["  ".into(), "".into()],
            ..Default::default()
        };
        assert_eq!(c.normalized_servers().len(), 3, "空列表要回退默认三台");
    }

    #[test]
    fn servers_are_trimmed_deduped_and_capped() {
        let mut c = NtpConfig {
            servers: vec![" a.example ".into(), "A.EXAMPLE".into(), "b.example".into()],
            ..Default::default()
        };
        c.servers = c.normalized_servers();
        assert_eq!(c.servers, vec!["a.example", "b.example"]);
        let many: Vec<String> = (0..20).map(|i| format!("s{i}.example")).collect();
        let c = NtpConfig {
            servers: many,
            ..Default::default()
        };
        assert_eq!(c.normalized_servers().len(), 8, "最多 8 台");
    }

    #[test]
    fn timeout_is_clamped() {
        let mut c = NtpConfig {
            timeout_secs: 99,
            ..Default::default()
        };
        c.normalize();
        assert_eq!(c.timeout_secs, 10);
        let mut c = NtpConfig {
            timeout_secs: 0,
            ..Default::default()
        };
        c.normalize();
        assert_eq!(c.timeout_secs, 1);
    }
}
