//! 解析配置文件路径：`-config/--config <path>`，默认 `/etc/modemd/config.toml`。

/// 从进程参数里取配置路径。
pub fn config_path() -> String {
    let args: Vec<String> = std::env::args().collect();
    parse(&args)
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
}
