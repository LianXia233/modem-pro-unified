//! 芯讯通 SIMCom：仅 QMI。
//!
//! 依据 `third_party/simcom-cm`：只有 `QMIThread.c` / `QmiWwanCM.c` / `GobiNetCM.c`，
//! 没有 MBIM 实现；参数也最简（`-s apn user pass auth`、`-p pin`、`-f log`）。
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
    note: "仅 QMI（simcom-cm 无 MBIM 实现），不支持 QMAP 绑定",
};
