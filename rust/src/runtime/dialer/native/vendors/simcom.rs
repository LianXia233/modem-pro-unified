//! 芯讯通 SIMCom：QMI 首选，MBIM 作为标准协议兜底。
//!
//! 依据 `third_party/simcom-cm`：只有 `QMIThread.c` / `QmiWwanCM.c` / `GobiNetCM.c`，
//! 厂商实现里没有 MBIM；但原生后端自己实现 MBIM Basic Connect，所以仍可用。
//!
//! 差异点：
//! - 不带 `-i` 参数（网卡靠 `QmiWwanCM.c` 自己找），我们仍按 sysfs 反查，
//!   不依赖它的默认网卡名；
//! - 不支持 QMAP 绑定（`qmap_bridge_mode.c` 只是桥接辅助，不做 mux）。

use super::{Caps, DataLink};

pub const CAPS: Caps = Caps {
    links: &[DataLink::Qmi],
    mbim_session: 0,
    disable_autoconnect: true,
    needs_raw_ip: true,
    note: "QMI 首选（厂商实现无 MBIM）；MBIM 走标准协议；不做 QMAP 绑定",
};
