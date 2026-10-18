//! 移远 Quectel：QMI + MBIM 双栈。
//!
//! 依据 `third_party/quectel-cm`：同时带 `QMIThread.c` 与 `mbim-cm.c`，
//! `-p quectel-mbim-proxy` 走 MBIM、默认走 QMI。所以两条都能接管，
//! 但**默认仍选 QMI**（`QMIThread.c` 是主线，`mbim-cm.c` 是补充路径）。
//!
//! 差异点：
//! - 支持 QMAP（`-m iface-idx` 把呼叫绑到 `wwan0_<idx>`）—— 由 `netprov`
//!   选父口/子口处理，本模块只管父口上的 raw_ip；
//! - `-d` 表示「地址也通过 QMI 取」，我们不用它：地址交给 netifd。

use super::{Caps, DataLink};

pub const CAPS: Caps = Caps {
    links: &[DataLink::Qmi, DataLink::Mbim],
    mbim_session: 0,
    disable_autoconnect: true,
    needs_raw_ip: true,
    note: "QMI 优先（quectel-CM 主线），MBIM 作为备选（quectel-mbim-proxy 同源）",
};
