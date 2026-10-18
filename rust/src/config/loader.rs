//! 配置读取入口与默认路径。

use crate::error::Result;

use super::Config;

/// 读取并归一化配置。
///
/// ⚠️ **解析失败也不退出**：`config.toml` 是 conffile（升级不覆盖），被写坏一行
/// —— 少个引号、多个全角字符、别的程序按自己的格式覆写一遍 —— 就会让服务再也
/// 起不来，整台设备的模组界面全丢（真机踩坑：`[net] wan_mode` 被写成没引号的
/// `dhcp`，服务进 crash loop，只能上机手工改文件）。
///
/// 所以按「主配置 → `<path>.bak` → 内置默认值」依次兜底，每一级都把原因打到
/// stderr（procd 会收进 syslog）。默认值那一级是**收紧**的方向：登录闸门与
/// 短信密码闸门都是「不写也算开」，不会因为配置坏了就敞开。
pub fn load(path: &str) -> Config {
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("读取配置 {path} 失败，改用内置默认值");
        return Config::default_normalized();
    };
    match parse(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("解析配置 {path} 失败: {e}");
            // `.bak` 是改配置前自动留的那一份（见 config/patch.rs），通常是好的
            let bak = format!("{path}.bak");
            if let Ok(t) = std::fs::read_to_string(&bak) {
                match parse(&t) {
                    Ok(cfg) => {
                        eprintln!("已回退到备份 {bak}");
                        return cfg;
                    }
                    Err(e2) => eprintln!("备份 {bak} 也解析失败: {e2}"),
                }
            }
            eprintln!("改用内置默认值（登录闸门与短信密码闸门保持收紧）");
            Config::default_normalized()
        }
    }
}

/// 解析一份配置文本并归一化（默认值兜底 + 各段自检）。
fn parse(text: &str) -> Result<Config> {
    let mut cfg: Config =
        toml::from_str(text).map_err(|e| crate::error::msg(format!("解析失败: {e}")))?;
    cfg.normalize();
    Ok(cfg)
}

/// 默认日志文件（Windows 上退化为当前目录，方便开发自测）。
pub fn default_log_file() -> String {
    if cfg!(unix) {
        "/var/log/modemd.log".to_string()
    } else {
        "modemd.log".to_string()
    }
}

/// 默认 PID 文件。
pub fn default_pid_file() -> String {
    if cfg!(unix) {
        "/var/run/modemd.pid".to_string()
    } else {
        "modemd.pid".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{load, parse};
    use crate::config::Config;

    /// 临时目录（带进程号，避免并行测试互相踩）。
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("modemd-cfg-{}-{}", std::process::id(), tag));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn parses_minimal_config_and_fills_defaults() {
        let cfg = parse("[server]\nlisten = \"0.0.0.0:9999\"\n").unwrap();
        assert_eq!(cfg.server.listen, "0.0.0.0:9999");
        // 没写的段全部走默认值，且归一化把空串补上
        assert_eq!(cfg.net.wan_mode, "modem_pro");
        assert_eq!(cfg.ui.ubus_http, "http://127.0.0.1/ubus");
        // 默认方向是收紧：两道闸门都不写也算开
        assert!(cfg.require_luci_login());
        assert!(cfg.ui.sms_password_required);
    }

    #[test]
    fn broken_config_falls_back_to_bak() {
        let dir = tmp_dir("bak");
        let path = dir.join("config.toml").to_string_lossy().to_string();
        // 主配置被写坏（真机事故：wan_mode 少了引号）
        let _ = std::fs::write(&path, "[net]\nwan_mode = dhcp\n");
        // 改配置前自动留的备份是好的
        let _ = std::fs::write(format!("{path}.bak"), "[net]\nwan_mode = \"both\"\n");
        let cfg = load(&path);
        assert_eq!(cfg.net.wan_mode, "both");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_config_without_bak_uses_defaults() {
        let dir = tmp_dir("nobak");
        let path = dir.join("config.toml").to_string_lossy().to_string();
        let _ = std::fs::write(&path, "[net]\nwan_mode = dhcp\n");
        let _ = std::fs::remove_file(format!("{path}.bak"));
        let cfg = load(&path);
        assert_eq!(cfg.net.wan_mode, "modem_pro");
        // 兜底不能顺手把闸门放开
        assert!(cfg.require_luci_login());
        assert!(cfg.ui.sms_password_required);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tmp_dir("missing");
        let path = dir.join("nope.toml").to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        let cfg = load(&path);
        assert_eq!(cfg.server.log_file, super::default_log_file());
        assert_eq!(cfg.server.pid_file, super::default_pid_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_normalized_is_self_consistent() {
        let cfg = Config::default_normalized();
        assert_eq!(cfg.net.wan_mode, "modem_pro");
        assert_eq!(cfg.ui.ubus_http, "http://127.0.0.1/ubus");
    }
}
