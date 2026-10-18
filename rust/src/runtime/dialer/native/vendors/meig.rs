//! 美格智能 MeiG：MBIM 优先，QMI 备选。
//!
//! 依据 `third_party/meig-cm`：带 `mbim-cm.c` 且默认流程走 MBIM；
//! QMI 通过 `meig-qmi-proxy.c` 转发（协议仍是标准 QMI，但多一层代理）。
//! 所以首选 MBIM，QMI 作为兼容路径。
//!
//! 差异点：
//! - MBIM 会话 id 用 0（`mbim-cm.c` 里 `sessionID = 0`）；
//! - `-n channelID` 的 multi-pdn 与 fibocom 同构，本实现只做单通道。

use super::{Caps, DataLink};

pub const CAPS: Caps = Caps {
    links: &[DataLink::Mbim, DataLink::Qmi],
    mbim_session: 0,
    disable_autoconnect: true,
    needs_raw_ip: true,
    note: "MBIM 优先（meig-cm 主线），QMI 走 meig-qmi-proxy 兼容",
};
