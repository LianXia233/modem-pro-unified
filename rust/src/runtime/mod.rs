//! 运行时支撑：PID 文件 / 信号 / 拨号器回收 / 后台巡检。

pub mod dialer;
pub mod hostnic;
pub mod ntp;
pub mod pid;
pub mod scheduler;
pub mod signals;
pub mod store;
