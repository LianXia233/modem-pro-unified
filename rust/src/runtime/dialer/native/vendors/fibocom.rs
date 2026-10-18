//! 广和通 Fibocom：仅 QMI。
//!
//! 依据 `third_party/fibocom-dial`：源码里**没有** `mbim-cm.c`（只有
//! `QMIThread.c` / `QmiWwanCM.c` / `GobiNetCM.c`），所以 MBIM 不在接管范围 ——
//! 声明成支持 MBIM 的话，`host_dial_bin = "native"` 会在 MBIM 模组上直接失败。
//!
//! 差异点：
//! - **multi-pdn**（`-m/-n/-N`）：多通道时每条通道各自一个 PDN，本实现只做
//!   `[net] host_dial_pdn` 指定的那一条（单通道足够覆盖家用场景）；
//! - **PCIe 模式**（`query_pcie_mode.c`）：PCIe 挂载的模组走 MHI，不发 QMI
//!   控制命令；这种情况 `find_wdm()` 找不到控制口，会明确报错而不是瞎猜；
//! - FM350 / RW350 是 RNDIS/ECM，本来就不走这里（由 `netprov` 配 static 地址）。

use super::{Caps, DataLink};

pub const CAPS: Caps = Caps {
    links: &[DataLink::Qmi],
    mbim_session: 0,
    disable_autoconnect: true,
    needs_raw_ip: true,
    note: "仅 QMI（fibocom-dial 无 mbim-cm.c）；PCIe/MHI 模式不走这里",
};
