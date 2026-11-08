//! 解析启动模式：`wan-client` 子命令，或守护进程的 `-config/--config <path>`
//!（默认 `/etc/modemd/config.toml`）。

/// 启动模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// 常驻守护进程（默认）。
    Daemon { config: String },
    /// netifd 接口拉起的子命令；参数走环境变量，不接受命令行配置。
    WanClient,
}

/// 从进程参数里取启动模式。
pub fn command() -> Command {
    let args: Vec<String> = std::env::args().collect();
    parse_command(&args)
}

/// 从进程参数里取配置路径。
pub fn config_path() -> String {
    let args: Vec<String> = std::env::args().collect();
    parse(&args)
}

/// 纯函数版：子命令只认 argv[1]。
pub fn parse_command(args: &[String]) -> Command {
    if args.get(1).map(|s| s.as_str()) == Some("wan-client") {
        return Command::WanClient;
    }
    Command::Daemon {
        config: parse(args),
    }
}

/// 纯函数版解析（便于单测）。
pub fn parse(args: &[String]) -> String {
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "-config" || a == "--config" {
            if let Some(p) = args.get(i + 1) {
                return p.clone();
            }
        } else if let Some(p) = a.strip_prefix("--config=") {
            return p.to_string();
        } else if let Some(p) = a.strip_prefix("-config=") {
            return p.to_string();
        }
        i += 1;
    }
    "/etc/modemd/config.toml".to_string()
}

#[cfg(test)]
mod tests {
    use super::parse;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_when_absent() {
        assert_eq!(parse(&args(&["modemd"])), "/etc/modemd/config.toml");
    }

    #[test]
    fn short_and_long_forms() {
        assert_eq!(parse(&args(&["modemd", "-config", "/a.toml"])), "/a.toml");
        assert_eq!(parse(&args(&["modemd", "--config", "/b.toml"])), "/b.toml");
        assert_eq!(parse(&args(&["modemd", "--config=/c.toml"])), "/c.toml");
        assert_eq!(parse(&args(&["modemd", "-config=/d.toml"])), "/d.toml");
    }

    #[test]
    fn wan_client_subcommand() {
        use super::{parse_command, Command};
        assert_eq!(
            parse_command(&args(&["modemd", "wan-client"])),
            Command::WanClient
        );
        // 子命令后跟其它参数也照旧认子命令（参数由环境变量提供）
        assert_eq!(
            parse_command(&args(&["modemd", "wan-client", "wwan0"])),
            Command::WanClient
        );
    }

    #[test]
    fn default_mode_is_daemon() {
        use super::{parse_command, Command};
        assert_eq!(
            parse_command(&args(&["modemd"])),
            Command::Daemon {
                config: "/etc/modemd/config.toml".to_string()
            }
        );
        assert_eq!(
            parse_command(&args(&["modemd", "--config", "/x.toml"])),
            Command::Daemon {
                config: "/x.toml".to_string()
            }
        );
    }
}
