//! 广和通 Fibocom：QMI 首选，MBIM 作为标准协议兜底。
//!
//! 依据 `third_party/fibocom-dial`：那份厂商实现里**没有** `mbim-cm.c`
//! （只有 `QMIThread.c` / `QmiWwanCM.c` / `GobiNetCM.c`），所以它的首选是 QMI。
//! 但 MBIM 是标准协议，[`super::super::mbim_link`] 自己实现 Basic Connect，
//! 不再受厂商二进制能力限制 —— 这里的 `links` 只表示**优先级**。
//!
//! 差异点：
//! - **multi-pdn**（`-m/-n/-N`）：多通道时每条通道各自一个 PDN，本实现只做
//!   `[net] host_dial_pdn` 指定的那一条（单通道足够覆盖家用场景）；
//! - **PCIe 模式**（`query_pcie_mode.c`）：PCIe 挂载的模组走 MHI；控制口由
//!   [`super::find_control`] 从 `/dev/mhi_QMI*` 反查，端点类型自动切成 PCIe；
//! - FM350 / RW350 是 **RNDIS**（不是 ECM），本来就不走这里（由 `netprov` 配 static 地址）。

use super::{Caps, DataLink};

pub const CAPS: Caps = Caps {
    links: &[DataLink::Qmi],
    mbim_session: 0,
    disable_autoconnect: true,
    needs_raw_ip: true,
    note: "QMI 首选（厂商实现无 mbim-cm.c）；MBIM 走标准协议；PCIe/MHI 用 mhi_QMI 控制口",
};
