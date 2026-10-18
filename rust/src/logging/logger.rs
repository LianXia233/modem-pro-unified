//! 极简日志器实现。
//!
//! 刻意不用 env_logger / tracing（会把 anstream、windows-sys 等一堆依赖拖进来）。
//! 行为：日志文件打不开时退化为只写 stderr，**不阻断启动**。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};

use super::time::timestamp;

struct SimpleLogger {
    file: Option<Mutex<File>>,
    level: LevelFilter,
}

impl Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {} {}", timestamp(), record.level(), record.args());
        eprintln!("{line}");
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = writeln!(f, "{line}");
            }
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.flush();
            }
        }
    }
}

/// 把 "debug"/"warn"/... 映射成 LevelFilter；认不出用 info。
pub fn parse_level(s: &str) -> LevelFilter {
    match s.trim().to_ascii_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "warn" | "warning" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        "off" => LevelFilter::Off,
        _ => LevelFilter::Info,
    }
}

/// 初始化全局日志器。log_file 打不开时只写 stderr。
pub fn init(log_file: &str, level: &str) {
    let file = if log_file.trim().is_empty() {
        None
    } else {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .ok()
            .map(Mutex::new)
    };
    let filter = parse_level(level);
    // set_boxed_logger 只能成功一次；失败（重复调用）也不致命。
    let _ = log::set_boxed_logger(Box::new(SimpleLogger {
        file,
        level: filter,
    }));
    log::set_max_level(filter);
}

#[cfg(test)]
mod tests {
    use super::parse_level;
    use log::LevelFilter;

    #[test]
    fn level_mapping() {
        assert_eq!(parse_level("DEBUG"), LevelFilter::Debug);
        assert_eq!(parse_level(" warning "), LevelFilter::Warn);
        assert_eq!(parse_level("nonsense"), LevelFilter::Info);
    }
}
