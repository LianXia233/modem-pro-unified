//! 厂商拨号策略：四家外部拨号器的差异，收敛成四个 Rust 模块。
//!
//! 外部二进制（`third_party/{quectel-cm,fibocom-dial,meig-cm,simcom-cm}`）的
//! 数据面协议基本同源（QMI 的 `QMIThread.c`、MBIM 的 `mbim-cm.c`），真正的差异只有：
//!
//! | 厂商 | 数据面 | 差异点 |
//! | --- | --- | --- |
//! | Quectel | QMI + MBIM | 有 `mbim-cm.c`；支持 QMAP / GobiNet；`-d` 走 QMI 取地址 |
//! | Fibocom | 仅 QMI | 无 `mbim-cm.c`；multi-pdn（`-m/-n/-N`）；PCIe 模式另判 |
//! | MeiG | MBIM 优先 | `meig-cm` 主打 MBIM，QMI 走 `meig-qmi-proxy` |
//! | SIMCom | 仅 QMI | 无 MBIM；参数最简（`-s / -p / -f`） |
//!
//! 所以「四家模块」不该复制四份 QMI/MBIM 实现，而是各自声明
//! **支持哪些链路、优先级、以及少数开关**；协议实现在
//! [`super::qmi_link`] / [`super::mbim_link`] 共用。这样：
//!
//! - 新增一家厂商 = 加一个文件 + 在 [`select`] 挂一行；
//! - 协议修一次，四家同时受益；
//! - 谁支持什么一眼可查（`links()`），不会出现「给 SIMCom 发 MBIM 命令」这种错。
//!
//! 原生后端**只碰 `cdc-wdm`**，不发 AT：AT 通道归 `modem/at` 与各厂商
//! `VendorSpec` 管（两条通道抢同一个串口的代价比省几条 AT 大得多）。

pub mod fibocom;
pub mod meig;
pub mod quectel;
pub mod simcom;

use crate::modem::vendor::Vendor;

/// 原生拨号能接管的数据面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataLink {
    Qmi,
    Mbim,
}

impl DataLink {
    pub fn as_str(self) -> &'static str {
        match self {
            DataLink::Qmi => "qmi",
            DataLink::Mbim => "mbim",
        }
    }
}

/// 一家厂商的策略（纯数据；协议实现不在这里）。
pub struct Caps {
    /// 支持的链路，**第一个是首选**。
    pub links: &'static [DataLink],
    /// MBIM 的 session id（0 = 默认会话；multi-pdn 时由 profile 决定）。
    pub mbim_session: u32,
    /// 拨号前发 `WDS Set Autoconnect(0)`：防止固件自己抢先建呼叫。
    ///
    /// quectel-cm / fibocom-dial 的 `QmiWwanInit()` 都做这一步；不做的话
    /// 固件可能在 CTL Sync 之后先建一条默认承载，我们的 Start 会被拒。
    pub disable_autoconnect: bool,
    /// 需要把 `qmi/raw_ip` 置 `Y`（QMI 必须；纯 MBIM 不需要）。
    pub needs_raw_ip: bool,
    /// 一句话说明（写进日志与文档，解释为什么这么排）。
    pub note: &'static str,
}

/// 按厂商选策略。未识别的厂商走「只支持 QMI」的保守默认。
pub fn select(vendor: Vendor) -> &'static Caps {
    match vendor {
        Vendor::Quectel => &quectel::CAPS,
        Vendor::Fibocom => &fibocom::CAPS,
        Vendor::Meig => &meig::CAPS,
        Vendor::Simcom => &simcom::CAPS,
        // 鼎桥 / 展锐 / 未识别：固件多半是 QMI 变体，但没取证就不额外声明
        _ => &UNKNOWN,
    }
}

/// 保守默认（只 QMI、不改 raw_ip 之外的东西）。
pub const UNKNOWN: Caps = Caps {
    links: &[DataLink::Qmi],
    mbim_session: 0,
    disable_autoconnect: false,
    needs_raw_ip: true,
    note: "未识别厂商：按通用 QMI 处理（不额外改固件状态）",
};

/// 这家厂商在这个链路类型上能不能拨。
pub fn supports(vendor: Vendor, link: DataLink) -> bool {
    select(vendor).links.contains(&link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_vendors_declare_at_least_one_link() {
        for v in [
            Vendor::Quectel,
            Vendor::Fibocom,
            Vendor::Simcom,
            Vendor::Meig,
            Vendor::TDTech,
            Vendor::Unisoc,
            Vendor::Unknown,
        ] {
            let caps = select(v);
            assert!(!caps.links.is_empty(), "{v:?} 没声明任何数据面");
            assert!(!caps.note.is_empty(), "{v:?} 缺说明");
        }
    }

    #[test]
    fn vendor_differences_match_reference_sources() {
        // quectel-cm 有 mbim-cm.c ⇒ 支持 MBIM
        assert!(supports(Vendor::Quectel, DataLink::Mbim));
        // meig-cm 有 mbim-cm.c 且主打 MBIM ⇒ 首选 MBIM
        assert_eq!(select(Vendor::Meig).links[0], DataLink::Mbim);
        assert!(supports(Vendor::Meig, DataLink::Qmi), "QMI 走 proxy 仍可用");
        // fibocom-dial / simcom-cm 只有 QMI（没有 mbim-cm.c）
        assert!(!supports(Vendor::Fibocom, DataLink::Mbim));
        assert!(!supports(Vendor::Simcom, DataLink::Mbim));
        assert_eq!(select(Vendor::Fibocom).links[0], DataLink::Qmi);
        assert_eq!(select(Vendor::Simcom).links[0], DataLink::Qmi);
    }

    #[test]
    fn qmi_backends_disable_autoconnect_but_unknown_does_not() {
        // 四家都按 QmiWwanInit() 的做法关自动连接；未识别厂商不乱改固件状态
        for v in [
            Vendor::Quectel,
            Vendor::Fibocom,
            Vendor::Simcom,
            Vendor::Meig,
        ] {
            assert!(select(v).disable_autoconnect, "{v:?} 应该关自动连接");
        }
        assert!(!select(Vendor::Unknown).disable_autoconnect);
    }

    #[test]
    fn all_backends_need_raw_ip_for_qmi() {
        // QMI 数据面必须 raw_ip=Y；没有厂商例外（MBIM 时代不设）
        for v in [
            Vendor::Quectel,
            Vendor::Fibocom,
            Vendor::Simcom,
            Vendor::Meig,
            Vendor::Unknown,
        ] {
            assert!(select(v).needs_raw_ip, "{v:?} 走 QMI 时必须 raw_ip");
        }
    }
}
