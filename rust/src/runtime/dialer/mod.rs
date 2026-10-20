//! 主机侧拨号器托管：QMI / MBIM 模组的数据呼叫由**内置 Rust 实现**建立。
//!
//! ## 为什么需要它
//!
//! QMI / MBIM 模组的数据通道由**主机侧**建（发 QMI `WDS Start` 或 MBIM
//! `CONNECT`），而模组只有**一个数据承载** —— AT 侧再激活一个上下文会把承载
//! 占住，拨号器随后请求数据呼叫就被拒（实测 RM502Q-GL：每轮 `QMUXError 0xe`、
//! `wwan0 link_state 0`）。所以在 `/api/dial/connect` 里对这两类是**拒绝**而不是
//! 代发 `AT+CGACT`；这个模块补上「谁来拨号」那一半。
//!
//! ## 分层
//!
//! | 文件 | 职责 | 单测 |
//! | --- | --- | --- |
//! | `mod.rs`（本文件） | 编排：该不该跑、掉线重拨、失败退避、按模组回收 | 全部可 |
//! | `native/mod.rs` | 计划：控制口反查、QMAP 参数、链路分派 | 纯函数可测 |
//! | `native/qmi_link.rs` | QMI 传输 + WDS 数据呼叫 + QMAP 绑 mux | 需真设备 |
//! | `native/mbim_link.rs` | MBIM 传输 + Basic Connect | 需真设备 |
//! | `qmi.rs` / `mbim.rs` | 线协议编解码（golden bytes） | 全部可 |
//!
//! ## 权威语义（`files/etc/modemd/config.toml` §`[net]`）
//!
//! - 每轮组网循环把它拉起、看着、退出自动重试；**模组拔了 / 手动断开就停掉**；
//! - 掉线（`Get Packet Service Status` != connected）立即重拨，失败退避 5 秒；
//! - 地址/DNS/默认路由仍由 netifd 的 `proto=modem_pro`（udhcpc + odhcp6c）取，
//!   与外部拨号器时代完全一样 —— 切后端不改变接口形态；
//! - 控制口按 sysfs 反查（`cdc-wdm` / `GobiQMI` / `mhi_QMI`），**不猜**
//!   `/dev/cdc-wdm0`：多模组时会把呼叫建到别人身上；
//! - QMAP 模组（`qmap_mode > 0`）先发 `WDA Set Data Format`，再把 v4/v6
//!   两个 WDS client 绑到同一个 mux（`0x81` = PDN 1）。
//!
//! ## 与旧实现的关系
//!
//! 2026-10-19 起四家厂商的 C 拨号器（quectel-CM / fibocom-dial / meig-cm /
//! simcom-cm）已从本仓库移除，`host_dial_bin` 只剩「内置」一种取值；
//! 旧配置里写外部二进制名的行为是**忽略并告警**，不会静默退回别的实现。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::http::AppState;
#[cfg(unix)]
use crate::modem::model::Modem;
#[cfg(unix)]
use crate::netprov::provision::pick_device;

#[cfg(unix)]
pub mod native;

#[cfg(unix)]
mod mbim;
#[cfg(unix)]
mod qmi;

/// 全局托管器（`lib.rs` 的退出钩子与组网循环共用同一个）。
static SUPERVISOR: OnceLock<DialerSupervisor> = OnceLock::new();

/// 全局托管器（进程内单例）。
pub(crate) fn supervisor() -> &'static DialerSupervisor {
    SUPERVISOR.get_or_init(DialerSupervisor::new)
}

/// 停止所有会话（进程退出时调）。
pub fn cleanup_all() {
    supervisor().stop_all();
}

/// 算不出计划的原因（每一种都要能在页面上说清楚）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// MBIM 模式没有 APN（设备的 `CONNECT` 会直接失败）。
    MbimNeedsApn,
    /// 没能确定数据网卡（不知道拨谁）。
    NoDevice,
    /// 没找到 QMI/MBIM 控制口（`cdc-wdm` / `GobiQMI` / `mhi_QMI`）。
    NoWdm(String),
    /// 这个类型不归内置拨号器管（目前只做 QMI / MBIM）。
    Unsupported(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::MbimNeedsApn => {
                write!(
                    f,
                    "MBIM 模式必须给 APN（否则设备会拒绝 CONNECT），请先配置 APN"
                )
            }
            PlanError::NoDevice => write!(f, "没能确定这个模组的数据网卡"),
            PlanError::NoWdm(why) => write!(f, "{why}"),
            PlanError::Unsupported(why) => write!(f, "{why}"),
        }
    }
}

/// `host_dial_bin` 里的原生开关（`native` / `rust` / `builtin`，空值同义）。
///
/// 空值是历史默认（旧版本表示「按厂商标自动找二进制」）；外部二进制移除后
/// 它等同于内置实现，所以这里一并认。
pub fn is_native_bin_name(raw: &str) -> bool {
    let b = raw.trim();
    b.is_empty()
        || b.eq_ignore_ascii_case("native")
        || b.eq_ignore_ascii_case("rust")
        || b.eq_ignore_ascii_case("builtin")
}

/// 这个模组能不能交给内置后端（链路类型 + 厂商策略都支持才交给它）。
///
/// 链路类型走 `ensure_dial_kind` 而不是 `cached_dial_kind`：后者在「没拨过号、
/// 页面没拉过 `/api/dial`」时是空串，会把刚插上的 QMI/MBIM 模组全部判成
/// 「不接管」，拨号器永远不启动（根因与修复详见
/// [`crate::modem::model::Modem::ensure_dial_kind`]）。
#[cfg(unix)]
fn native_takes(modem: &Modem) -> bool {
    let kind = modem.ensure_dial_kind();
    let link = match kind.trim().to_ascii_lowercase().as_str() {
        "qmi" => native::vendors::DataLink::Qmi,
        "mbim" => native::vendors::DataLink::Mbim,
        _ => return false,
    };
    native::vendors::supports(modem.vendor(), link)
}

/// 旧配置里写了外部二进制名时提示一次（每进程一次，不刷日志）。
#[cfg(unix)]
fn warn_legacy_bin_once(state: &AppState) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    let raw = state.cfg.net.host_dial_bin.trim();
    if raw.is_empty() || is_native_bin_name(raw) || WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    log::warn!("配置 host_dial_bin = {raw:?} 已失效：厂商拨号器已移除，改用内置 QMI/MBIM 实现");
}

/// 拨号器托管器（一个进程一个，挂在全局 [`supervisor`] 上）。
#[derive(Default)]
pub struct DialerSupervisor {
    /// 原生会话（按模组 id）。
    #[cfg(unix)]
    native: Mutex<HashMap<u32, native::Session>>,
    /// 连续失败时的退避（模组 id → (最后错误, 时刻)）。
    ///
    /// QModem 的重试循环是 `sleep 5` 后继续；这里等价，但**只在这个模组上退避**，
    /// 不影响别的模组，也不会每 15 秒把控制口刷一遍。
    #[cfg(unix)]
    native_retry: Mutex<HashMap<u32, (String, std::time::Instant)>>,
    /// 连续失败次数（模组 id → 次数）；成功一次即清零。
    ///
    /// 用来做指数退避：5 → 10 → 20 → 40 → 60（封顶）。固定 5 秒重试在
    /// 「SIM 未插 / 套餐欠费 / 频段不可用」这类持续故障上会把控制口刷爆。
    #[cfg(unix)]
    native_fails: Mutex<HashMap<u32, u32>>,
}

/// 第 `fails` 次连续失败后的重试间隔：5 → 10 → 20 → 40 → 60（封顶）。
#[cfg(unix)]
fn retry_delay(fails: u32) -> std::time::Duration {
    let secs = 5u64.saturating_mul(1 << fails.saturating_sub(1).min(4));
    std::time::Duration::from_secs(secs.min(60))
}

impl DialerSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 停掉一个模组的会话（用户点了「断开」时调）。
    pub fn stop_modem(&self, modem_id: u32) {
        #[cfg(unix)]
        self.stop_native(modem_id);
    }

    /// 全部停掉（进程退出）。
    pub fn stop_all(&self) {
        #[cfg(unix)]
        {
            let all: Vec<(u32, native::Session)> = self.native.lock().unwrap().drain().collect();
            for (id, mut s) in all {
                s.stop();
                log::info!("原生拨号：模组 {id} 随进程退出停止");
            }
        }
    }

    /// 每轮组网循环调用：该跑的拉起 / 看着，不该跑的收起。
    #[cfg(unix)]
    pub fn reconcile(&self, state: &AppState) {
        warn_legacy_bin_once(state);
        let running: Vec<u32> = state.mgr.all().iter().map(|m| m.id).collect();
        // 模组没了 → 先收，避免拔掉后还挂着
        let gone: Vec<u32> = self
            .native
            .lock()
            .unwrap()
            .keys()
            .copied()
            .filter(|id| !running.contains(id))
            .collect();
        for id in gone {
            log::info!("原生拨号：模组 {id} 已不在，停掉会话");
            self.stop_native(id);
        }
        for modem in state.mgr.all() {
            if !modem.is_running() || !native_takes(&modem) {
                self.stop_native(modem.id);
                continue;
            }
            self.reconcile_native_one(state, &modem);
        }
    }

    /// 非 Unix：没有 sysfs / 控制口，什么都不做（与旧实现一致）。
    #[cfg(not(unix))]
    pub fn reconcile(&self, _state: &AppState) {}
}

#[cfg(unix)]
impl DialerSupervisor {
    /// 停掉一个模组的原生会话（重复调用安全）。
    fn stop_native(&self, modem_id: u32) {
        let s = self.native.lock().unwrap().remove(&modem_id);
        self.native_retry.lock().unwrap().remove(&modem_id);
        self.native_fails.lock().unwrap().remove(&modem_id);
        if let Some(mut s) = s {
            s.stop();
            log::info!("原生拨号：模组 {modem_id} 会话已停止");
        }
    }

    /// 原生后端的一轮对账：该建的建、该停的停、掉线重拨、失败退避 5 秒。
    fn reconcile_native_one(&self, state: &AppState, modem: &Modem) {
        let id = modem.id;
        let st = modem.dial_status();
        let stop_reason = if !state.cfg.net.host_dial {
            Some("host_dial 关闭")
        } else if !state.dial_enabled() {
            Some("拨号总开关关闭")
        } else if st.manual_stop {
            Some("用户手动断开")
        } else {
            None
        };
        if let Some(why) = stop_reason {
            if self.native.lock().unwrap().contains_key(&id) {
                log::info!("原生拨号：模组 {id} 停（{why}）");
            }
            self.stop_native(id);
            return;
        }

        // APN 三级回退（与一键拨号 / 自动拨号同一套优选语义，见
        // `params::pick_dial_context`）：拨号状态里记的 → 上次手动拨号用的 →
        // **模组里已配置的上下文**（走 15 秒缓存，代价可忽略）。
        // 早先只有前两级：固件里存了 APN 但从没在页面上拨过的 MBIM 模组，
        // 会陷入「算不出计划（缺 APN）→ 5 秒退避 → 再来」的死循环。
        let apn = if st.apn.trim().is_empty() {
            let last = modem.last_dial_apn();
            if !last.is_empty() {
                last
            } else {
                crate::modem::model::pick_dial_context(&modem.params().contexts)
                    .map(|c| c.apn)
                    .unwrap_or_default()
            }
        } else {
            st.apn.clone()
        };
        let device = pick_device(modem, &[]);
        let kind = modem.ensure_dial_kind();
        let plan = match native::plan_native(
            id,
            modem.vendor(),
            &kind,
            &apn,
            &device,
            state.wan_ipv6(),
            state.cfg.net.host_dial_pdn,
        ) {
            Ok(p) => p,
            Err(e) => {
                // 算不出计划时**没有会话可停**（压根还没建起来），但原实现在
                // 这里调了 `stop_native` —— 它会把 `native_retry` /
                // `native_fails` 一并清掉，于是「错误不变就静默 + 指数退避」
                // 整体失效：fails 每轮都从 0→1，退避永远停在第一档 5 秒
                // （循环本身 15 秒一轮，等于每轮硬试）。持续故障（缺 APN /
                // 控制口还没出现 / 网卡没被 netifd 拉起）必须靠退避压住，
                // 否则 15 秒一轮地把控制口和 sysfs 刷一遍。
                // 退避状态跨轮保留；错误**变化**时才清零（情况有变，值得立刻再试）。
                let mut g = self.native_retry.lock().unwrap();
                let msg = e.to_string();
                let changed = g.get(&id).map(|(m, _)| m != &msg).unwrap_or(true);
                if changed {
                    log::warn!("原生拨号：模组 {id} 算不出计划: {e}");
                    self.native_fails.lock().unwrap().remove(&id);
                }
                g.insert(id, (msg, std::time::Instant::now()));
                {
                    let mut f = self.native_fails.lock().unwrap();
                    *f.entry(id).or_insert(0) += 1;
                }
                return;
            }
        };

        // 已在跑同一个计划：探活；掉线就重建
        {
            let mut g = self.native.lock().unwrap();
            if let Some(s) = g.get_mut(&id) {
                if s.plan == plan {
                    match s.alive() {
                        Some(false) => log::warn!("原生拨号：模组 {id} 数据呼叫已断开，重拨"),
                        // 传输层出错（None）不当作掉线：一次读超时不该把呼叫拆了
                        _ => return,
                    }
                } else {
                    log::info!("原生拨号：模组 {id} 计划变化，重建会话");
                }
            }
            // 取出旧会话（Drop 会 stop）；锁外再停，避免 QMI 请求占着锁
            let old = g.remove(&id);
            drop(g);
            if let Some(mut s) = old {
                s.stop();
            }
        }

        // 失败退避：按连续失败次数指数增长（5/10/20/40/60s）
        {
            let g = self.native_retry.lock().unwrap();
            let fails = self
                .native_fails
                .lock()
                .unwrap()
                .get(&id)
                .copied()
                .unwrap_or(0);
            if let Some((_, at)) = g.get(&id) {
                if at.elapsed() < retry_delay(fails.max(1)) {
                    return;
                }
            }
        }

        match native::Session::start(plan.clone()) {
            Ok(s) => {
                self.native_retry.lock().unwrap().remove(&id);
                self.native_fails.lock().unwrap().remove(&id);
                log::info!("原生拨号：模组 {id} 拉起 {}", plan.describe());
                self.native.lock().unwrap().insert(id, s);
            }
            Err(e) => {
                log::warn!(
                    "原生拨号：模组 {id} 建立会话失败（{}）: {e}",
                    plan.describe()
                );
                self.native_retry
                    .lock()
                    .unwrap()
                    .insert(id, (e, std::time::Instant::now()));
                {
                    let mut f = self.native_fails.lock().unwrap();
                    *f.entry(id).or_insert(0) += 1;
                }
            }
        }
    }
}

impl DialerSupervisor {
    /// 这个模组当前的后端状态（给 `/api/dial` 的 `host_dialer` 字段）。
    pub fn describe(&self, modem_id: u32) -> serde_json::Value {
        #[cfg(unix)]
        if let Some(s) = self.native.lock().unwrap().get(&modem_id) {
            return s.status();
        }
        let _ = modem_id;
        serde_json::Value::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_bin_aliases_are_recognised() {
        for v in ["native", "NATIVE", "rust", "builtin", " Native ", "", "  "] {
            assert!(is_native_bin_name(v), "{v:?} 应识别为内置（含空值）");
        }
        for v in ["quectel-CM", "/usr/bin/native", "natives", "fibocom-dial"] {
            assert!(!is_native_bin_name(v), "{v:?} 不是内置开关");
        }
    }
}

#[cfg(all(test, unix))]
mod retry_tests {
    use super::retry_delay;

    /// 固定 5 秒会把「持续失败」变成每 5 秒刷一次控制口；必须指数退避并封顶。
    #[test]
    fn retry_delay_backs_off_then_caps() {
        assert_eq!(retry_delay(1).as_secs(), 5);
        assert_eq!(retry_delay(2).as_secs(), 10);
        assert_eq!(retry_delay(3).as_secs(), 20);
        assert_eq!(retry_delay(4).as_secs(), 40);
        assert_eq!(retry_delay(5).as_secs(), 60);
        assert_eq!(retry_delay(99).as_secs(), 60);
    }
}
