//! 探活扫描：在已知集合之外寻找「AT 应答正常」的串口。

use std::collections::BTreeSet;
use std::time::Duration;

use crate::modem::at::AtPort;

use super::ports;

/// 返回「已知之外、且 AT 探活成功」的串口路径。
pub fn discover_new(known: &BTreeSet<String>, baud: u32, probe_timeout: Duration) -> Vec<String> {
    ports::list_candidates()
        .into_iter()
        .filter(|path| !known.contains(path))
        .filter(|path| AtPort::probe(path, baud, probe_timeout))
        .collect()
}
