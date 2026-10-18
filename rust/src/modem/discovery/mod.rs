//! 模组发现：串口枚举 + 探活扫描。

mod ports;
mod scanner;

pub use ports::port_exists;
pub(crate) use scanner::discover_new;

/// 本机候选串口路径（`/api/at/port` 挑自动口用）。
pub fn ports_list() -> Vec<String> {
    ports::list_candidates()
}
