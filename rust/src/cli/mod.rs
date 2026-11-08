//! 命令行参数解析。
//!
//! 只有两种启动模式：
//!   · 不带子命令 —— 常驻守护进程（读 `-config/--config`）
//!   · `wan-client` —— netifd 接口拉起的子命令，参数全部来自环境变量（见 `wanclient`）

mod parser;

pub use parser::{command, config_path, Command};
