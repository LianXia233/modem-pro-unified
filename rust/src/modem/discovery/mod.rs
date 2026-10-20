//! 模组发现：串口枚举 + 物理分组 + 端口规则 + 身份级探活。
//!
//! 三层各管一件事，互不越界：
//!
//! - [`ports`]：把 `/dev/ttyUSB*` 读成「USB 设备 + 接口号 + 驱动」快照；
//! - [`plan`]：纯函数决策 —— 哪台设备、选哪个口、跳过谁、为什么；
//! - [`scanner`]：按计划探活，一台物理设备只返回一个命令口。

mod plan;
mod ports;
mod scanner;

pub use ports::port_exists;
pub(crate) use scanner::discover_new;

/// 本机候选串口路径（`/api/at/port` 挑自动口用）。
pub fn ports_list() -> Vec<String> {
    ports::list_candidates()
}
