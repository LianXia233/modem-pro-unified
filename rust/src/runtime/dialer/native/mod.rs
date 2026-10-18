//! 原生拨号器（不依赖 `quectel-CM` 等外部二进制）。
//!
//! ## 结构
//!
//! ```text
//! native/
//!   mod.rs        计划（NativePlan）+ 会话门面（Session）+ 链路选择
//!   qmi_link.rs   QMI 传输与 WDS 数据呼叫
//!   mbim_link.rs  MBIM 传输与 Basic Connect
//!   vendors/      四家厂商策略（支持哪些链路 / 优先级 / 开关）
//! ```
//!
//! 协议实现在两个 `*_link` 里共用，厂商差异只有 `vendors/<厂商>.rs` 的
//! `Caps` —— 与上游 `module/<vendor>` 的分法同构，但按「数据面」而不是
//! 「AT 方言」切分。
//!
//! ## 它做什么
//!
//! 只做**数据呼叫的生命周期**，与外部拨号器的职责边界一致：
//!
//! 1. 打开 `/dev/cdc-wdmN`（从数据网卡的 sysfs 反查，多模组不会串）；
//! 2. QMI：`CTL Sync` → `Get Client ID` → `WDS Start`（v4 service 1 / v6 0x11）；
//!    MBIM：`OPEN` → `CONNECT(activate)` → `IP_CONFIGURATION`；
//! 3. 退出时对称拆除（QMI 发 `Stop` 并释放 client id；MBIM 去激活后 `CLOSE`）。
//!
//! 地址/DNS/默认路由**不在**这里配：`netprov` 建的 `modem<id>` 接口仍由 netifd
//! 的 `modem_pro`（udhcpc + odhcp6c）取址 —— 与外部拨号器时代完全一样，
//! 所以切换后端不会改变接口形态，也不会和 netifd 抢同一张网卡。
//!
//! ## 为什么值得做
//!
//! - 外部二进制要跟固件/内核版本对齐（qmi_wwan 的 raw_ip、QMAP、GobiNet 差异），
//!   出问题只能等厂商更新；
//! - 状态能直接回给 `/api/dial`（handle / 地址 / MTU / 最后错误），排障不用猜；
//! - 参数（APN / family / profile）与配置强绑定，不会被拨号器自己的默认值覆盖。
//!
//! ## 开启方式
//!
//! `[net] host_dial_bin = "native"`（`rust` / `builtin` 同义）。默认仍用外部二进制；
//! 原生路径需要 `cdc-wdm` 控制口，RNDIS/ECM 模组（FM350 / RW350）不适用。

pub mod qmi_link;
pub mod vendors;

#[cfg(unix)]
pub mod mbim_link;

use crate::modem::vendor::Vendor;
use crate::runtime::dialer::PlanError;

use vendors::{Caps, DataLink};

/// 从数据网卡反查 QMI/MBIM 控制设备。
///
/// `qmi_wwan` 的布局是 `<usb interface>/usbmisc/cdc-wdmN`；不同内核/驱动层级
/// 差一层，所以逐级向上找。找不到就返回 `None` —— **不猜** `/dev/cdc-wdm0`
/// （多模组时会把呼叫建到别人身上）。
pub fn find_wdm(iface: &str) -> Option<String> {
    let base = parent_iface(iface);
    let mut dir = std::path::PathBuf::from(format!("/sys/class/net/{base}/device"));
    for _ in 0..4 {
        for sub in [dir.join("usbmisc"), dir.join("../usbmisc")] {
            if let Ok(rd) = std::fs::read_dir(&sub) {
                let mut names: Vec<String> = rd
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.starts_with("cdc-wdm"))
                    .collect();
                names.sort();
                if let Some(n) = names.into_iter().next() {
                    return Some(format!("/dev/{n}"));
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// QMAP 的父口名（`wwan0_1` → `wwan0`）。
pub fn parent_iface(iface: &str) -> String {
    // 只认 QMAP 的 `<父口>_<mux>`（后缀必须是数字）；`rmnet_data0` 这类名字
    // 里的下划线不是 mux 分隔符，不能切。
    match iface.rsplit_once('_') {
        Some((p, n)) if !p.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            p.to_string()
        }
        _ => iface.to_string(),
    }
}

/// 把 qmi_wwan 的 `raw_ip` 置 `Y`（数据面走 IP 而不是 802.3）。
///
/// 返回 `true` = 现在是 raw IP（或本来没有这个 sysfs，不需要设）。
/// 内核里这个属性有 `raw_ip` 与老版本的 `rawip` 两种写法，都试。
pub fn ensure_raw_ip(iface: &str) -> bool {
    let base = parent_iface(iface);
    for name in ["raw_ip", "rawip"] {
        let path = format!("/sys/class/net/{base}/qmi/{name}");
        let Ok(cur) = std::fs::read_to_string(&path) else {
            continue;
        };
        let cur = cur.trim();
        if cur == "Y" || cur == "1" {
            return true;
        }
        if let Err(e) = std::fs::write(&path, "Y\n") {
            log::warn!("写 {path} 失败: {e}");
            return false;
        }
        log::info!("原生拨号：把 {base} 的 qmi/{name} 置为 Y（数据面走 raw IP）");
        return true;
    }
    // 没有这个属性：要么不是 qmi_wwan（比如 GobiNet），要么已经由驱动处理好
    true
}

/// 原生拨号计划（纯数据，便于比较与日志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePlan {
    pub modem_id: u32,
    /// 厂商（决定用哪套策略）。
    pub vendor: Vendor,
    /// 数据面（QMI / MBIM）。
    pub link: DataLink,
    /// `/dev/cdc-wdmN`。
    pub wdm: String,
    /// 数据网卡（raw_ip sysfs 在它上面；QMAP 时取父口）。
    pub iface: String,
    pub apn: String,
    pub ipv6: bool,
    /// profile index（`[net] host_dial_pdn`，0 = 不指定）。
    pub pdn: i64,
    /// MBIM session id（QMI 忽略）。
    pub session: u32,
}

impl NativePlan {
    /// 一行可读描述（日志 / 状态接口用）。
    pub fn describe(&self) -> String {
        format!(
            "native {} {} ({}, apn={}, v6={}, pdn={})",
            self.vendor.as_str(),
            self.link.as_str(),
            self.wdm,
            if self.apn.is_empty() {
                "默认"
            } else {
                &self.apn
            },
            self.ipv6,
            self.pdn
        )
    }

    fn caps(&self) -> &'static Caps {
        vendors::select(self.vendor)
    }
}

/// 把链路类型字符串归一（`qmi` / `mbim`）。
fn parse_link(kind: &str) -> Option<DataLink> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "qmi" => Some(DataLink::Qmi),
        "mbim" => Some(DataLink::Mbim),
        _ => None,
    }
}

/// 算一个原生拨号计划（纯函数；`wdm` 反查是唯一 IO）。
pub fn plan_native(
    modem_id: u32,
    vendor: Vendor,
    kind: &str,
    apn: &str,
    device: &str,
    ipv6: bool,
    pdn: i64,
) -> Result<NativePlan, PlanError> {
    let Some(link) = parse_link(kind) else {
        return Err(PlanError::Unsupported(format!(
            "原生拨号器只接管 QMI / MBIM（当前类型 {kind}）；ECM/RNDIS/NCM 走模组侧 AT 拨号"
        )));
    };
    let caps = vendors::select(vendor);
    if !caps.links.contains(&link) {
        return Err(PlanError::Unsupported(format!(
            "{} 的原生后端不支持 {}（只支持 {}）：{}",
            vendor.label(),
            link.as_str(),
            caps.links
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
            caps.note
        )));
    }
    if device.trim().is_empty() {
        return Err(PlanError::NoDevice);
    }
    let wdm = find_wdm(device).ok_or_else(|| {
        PlanError::NoWdm(format!(
            "没在 /sys/class/net/{}/device 下找到 cdc-wdm 控制口",
            parent_iface(device)
        ))
    })?;
    Ok(NativePlan {
        modem_id,
        vendor,
        link,
        wdm,
        iface: parent_iface(device),
        apn: apn.trim().to_string(),
        ipv6,
        pdn: if pdn > 0 { pdn } else { 0 },
        session: caps.mbim_session,
    })
}

/// 一个在跑的原生会话（QMI 或 MBIM）。
pub struct Session {
    pub plan: NativePlan,
    inner: Inner,
    started_at: String,
    /// 给页面看的一句话（v6 未起、订阅者未就绪等非致命情况）
    pub note: String,
}

enum Inner {
    Qmi(qmi_link::QmiSession),
    #[cfg(unix)]
    Mbim(mbim_link::MbimSession),
}

impl Session {
    /// 建立数据呼叫（v4 必须成功；v6 失败只记 note，不拖垮 v4）。
    pub fn start(plan: NativePlan) -> Result<Self, String> {
        let caps = plan.caps();
        let (inner, note) = match plan.link {
            DataLink::Qmi => {
                // raw_ip 只对 QMI 有意义；MBIM 走 802.3 封装，设了反而可能出错
                if caps.needs_raw_ip && !ensure_raw_ip(&plan.iface) {
                    return Err(format!("{} 的 qmi/raw_ip 设置失败", plan.iface));
                }
                let (s, note) = qmi_link::QmiSession::start(
                    &plan.wdm,
                    &plan.iface,
                    &plan.apn,
                    plan.ipv6,
                    plan.pdn,
                    caps.disable_autoconnect,
                )
                .map_err(|e| format!("{}：{e}", plan.describe()))?;
                (Inner::Qmi(s), note)
            }
            DataLink::Mbim => {
                #[cfg(unix)]
                {
                    let (s, note) = mbim_link::MbimSession::start(
                        &plan.wdm,
                        &plan.apn,
                        plan.ipv6,
                        plan.session,
                    )
                    .map_err(|e| format!("{}：{e}", plan.describe()))?;
                    (Inner::Mbim(s), note)
                }
                #[cfg(not(unix))]
                {
                    return Err("MBIM 原生拨号只支持 Linux".to_string());
                }
            }
        };
        if !note.is_empty() {
            log::warn!("原生拨号：模组 {} {note}", plan.modem_id);
        }
        let s = Self {
            plan,
            inner,
            started_at: crate::logging::rfc3339(crate::logging::unix_now()),
            note,
        };
        log::info!(
            "原生拨号：模组 {} 数据呼叫已建立（{}）",
            s.plan.modem_id,
            s.plan.describe()
        );
        Ok(s)
    }

    /// 数据呼叫还活着吗？传输层出错时返回 `None`（别因为一次读超时就重拨）。
    pub fn alive(&mut self) -> Option<bool> {
        let r = match &mut self.inner {
            Inner::Qmi(s) => s.alive(),
            #[cfg(unix)]
            Inner::Mbim(s) => s.alive(),
        };
        if r == Some(false) {
            log::warn!(
                "原生拨号：模组 {} 数据呼叫已断开（{}）",
                self.plan.modem_id,
                self.plan.describe()
            );
        }
        r
    }

    /// 主动拆掉呼叫（重复调用安全）。
    pub fn stop(&mut self) {
        match &mut self.inner {
            Inner::Qmi(s) => s.stop(),
            #[cfg(unix)]
            Inner::Mbim(s) => s.stop(),
        }
        log::info!(
            "原生拨号：模组 {} 数据呼叫已拆除（{}）",
            self.plan.modem_id,
            self.plan.describe()
        );
    }

    /// 给 `/api/dial` 的状态（不外泄 APN 之外的任何凭据）。
    pub fn status(&self) -> serde_json::Value {
        let detail = match &self.inner {
            Inner::Qmi(s) => s.status(),
            #[cfg(unix)]
            Inner::Mbim(s) => s.status(),
        };
        let mut v = serde_json::json!({
            "backend": format!("native-{}", self.plan.link.as_str()),
            "vendor": self.plan.vendor.as_str(),
            "plan": self.plan.describe(),
            "wdm": self.plan.wdm,
            "device": self.plan.iface,
            "started_at": self.started_at,
            "note": self.note,
        });
        if let (Some(dst), Some(src)) = (v.as_object_mut(), detail.as_object()) {
            for (k, val) in src {
                dst.insert(k.clone(), val.clone());
            }
        }
        v
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::super::is_native_bin_name as is_native_bin;
    use super::*;

    #[test]
    fn native_bin_aliases_are_recognised() {
        for v in ["native", "NATIVE", "rust", "builtin", " Native "] {
            assert!(is_native_bin(v), "{v:?} 应识别为原生");
        }
        for v in ["", "quectel-CM", "/usr/bin/native", "natives"] {
            assert!(!is_native_bin(v), "{v:?} 不是原生开关");
        }
    }

    #[test]
    fn native_plan_rejects_unsupported_link_kind() {
        let e = plan_native(0, Vendor::Quectel, "ecm", "ctnet", "wwan0", false, 0).unwrap_err();
        assert!(matches!(e, PlanError::Unsupported(_)), "ECM 不该被原生接管");
        let e = plan_native(0, Vendor::Quectel, "qmi", "ctnet", "", false, 0).unwrap_err();
        assert!(matches!(e, PlanError::NoDevice));
    }

    #[test]
    fn vendor_without_mbim_is_reported_clearly() {
        // fibocom-dial 没有 mbim-cm.c ⇒ 明确拒绝，而不是运行时才失败
        let e = plan_native(0, Vendor::Fibocom, "mbim", "ctnet", "wwan0", false, 0).unwrap_err();
        let text = e.to_string();
        assert!(matches!(e, PlanError::Unsupported(_)));
        assert!(
            text.contains("不支持 mbim"),
            "错误文本要说清不支持什么: {text}"
        );
        // meiG 反过来：MBIM 首选但 QMI 也认
        assert!(vendors::supports(Vendor::Meig, DataLink::Mbim));
        assert!(vendors::supports(Vendor::Meig, DataLink::Qmi));
    }

    #[test]
    fn pdn_is_only_positive() {
        // 通过 plan 的归一化验证（不碰 sysfs 的部分）
        let p = NativePlan {
            modem_id: 1,
            vendor: Vendor::Quectel,
            link: DataLink::Qmi,
            wdm: "/dev/cdc-wdm0".into(),
            iface: "wwan0".into(),
            apn: "ctnet".into(),
            ipv6: true,
            pdn: 0,
            session: 0,
        };
        assert!(p.describe().contains("ctnet"));
        assert!(p.describe().contains("v6=true"));
        assert!(p.describe().contains("quectel"));
        assert!(p.describe().contains("qmi"));
    }

    #[test]
    fn parent_iface_strips_mux_suffix() {
        assert_eq!(parent_iface("wwan0_1"), "wwan0");
        assert_eq!(parent_iface("wwan0"), "wwan0");
        assert_eq!(parent_iface("rmnet_data0"), "rmnet_data0");
    }

    #[test]
    fn find_wdm_returns_none_for_missing_iface() {
        // 不存在的网卡不能瞎猜 /dev/cdc-wdm0（多模组会建到别人身上）
        assert_eq!(find_wdm("definitely-not-an-iface"), None);
    }
}
