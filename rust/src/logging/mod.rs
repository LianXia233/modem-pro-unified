//! 日志：标准错误 + 可选文件双写。

mod logger;
mod time;

pub use logger::{init, parse_level};
pub(crate) use time::{
    civil_from_days, days_from_civil, local_unix_now, parse_posix_tz_offset, rfc3339,
    seconds_since, set_clock_offset, set_configured_tz, system_now, timestamp, tz_offset_secs,
    unix_now,
};
