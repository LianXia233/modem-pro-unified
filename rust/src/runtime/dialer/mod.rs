//! 拨号协调器：全仓**唯一**的连接决策与生命周期调度点。
//!
//! ## 它解决什么问题（重构前）
//!
//! 早先「要不要连、谁来连、失败了怎么重试」散在三处，各判各的：
//!
//! | 决策点 | 节拍 | 退避 | 覆盖范围 |
//! | --- | --- | --- | --- |
//! | `loops::spawn_auto_dial` | 30s | 固定 300s | 仅 AT 路径（ECM/RNDIS/NCM） |
//! | `DialerSupervisor::reconcile` | 15s | 5→60s 指数 | 仅 QMI/MBIM |
//! | `handlers::dial_connect` | 按请求 | 无 | 手动（两条路径都碰） |
//!
//! 三者对 `dial_enabled` / `manual_stop` 的判定各写一遍，状态也各存一份：
//! `DialState` 只被 AT 路径写过，QMI/MBIM 会话建好之后**没有任何人**把状态
//! 改成 `connected`（页面永远停在「拨号中」）；重拨失败各自退避，互不知情。
//!
//! ## 现在
//!
//! 本模块是**唯一**做「该不该连 / 该重试吗 / 该停吗」的地方，其它入口只提交意图：
//!
//! ```text
//! 手动 /api/dial/connect ──┐
//! 异步任务 dial.connect  ──┼─→ Modem::dial()（AT 后端，同步执行）
//! 自动调度（本模块 tick）──┘        └─ 代次门禁，成功后写 DialState
//!
//! 自动调度（本模块 tick）────→ native::Session::start()（QMI/MBIM 后端）
//!                                  └─ 成功后回调 Modem::note_native_up()
//!
//! /api/dial/disconnect ────→ stop_modem() + Modem::hangup()（bump 代次）
//! ```
//!
//! 统一规则：
//! - **一个模组同一时刻只有一个有效连接意图**（`Modem::dial_epoch` 代次）；
//! - **一套退避策略**（[`Backend`] 区分初始值/上限，见 [`retry_delay`]）；
//! - **一套状态写入点**：AT 后端在 `dial_inner`，原生后端在 `note_native_up/down`，
//!   断开在 `hangup`；本模块不直接改 `DialState`，只调用这些入口。
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
//! - 每轮组网循环（15s，`provision::run_once`）调一次 [`DialerSupervisor::reconcile`]；
//! - 模组拔了 / 手动断开 → 停掉；掉线 → 重拨；失败 → 按 [`retry_delay`] 退避；
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
//!
//! 2026-10-21 起 `http::loops::spawn_auto_dial` 并入本模块的 `reconcile`：
//! AT 路径的自动重拨与 QMI/MBIM 的托管共用同一个节拍、同一套意图判定与退避表。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::http::AppState;
use crate::modem::model::dial::{DIAL_CONNECTED, DIAL_DIALING};
#[cfg(unix)]
use crate::modem::model::Modem;
#[cfg(unix)]
use crate::netprov::provision::device_for;

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

/// 后端类型（只影响**重试节奏**，不影响连接操作本身）。
///
/// 为什么两种节奏：AT 路径的失败绝大多数是**持续性前置条件**（没插卡 / 无服务 /
/// 套餐欠费），快速重试只是白刷串口与射频；原生会话的失败更多是**瞬时性**的
/// （控制口刚出现、QMI 事务超时、网卡还没被 netifd 拉起来），值得快速重来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// 模组侧 AT 配方（ECM / RNDIS / NCM / unknown）。
    At,
    /// 主机侧内置拨号器（QMI / MBIM）。
    Native,
}

/// 「本协调器已派出一轮 AT 拨号」的 RAII 标记。
///
/// 用 Drop 而不是手写删除：工作线程即使中途 panic，标记也会被清掉，
/// 不会让这台模组从此再也等不到自动重拨。
#[cfg(unix)]
struct InFlight(u32);

#[cfg(unix)]
impl Drop for InFlight {
    fn drop(&mut self) {
        supervisor().in_flight.lock().unwrap().remove(&self.0);
    }
}

/// 第 `fails` 次连续失败后的重试间隔（统一策略，两种后端只差参数）。
///
/// | 后端 | 序列 | 理由 |
/// | --- | --- | --- |
/// | AT | 30 / 60 / 120 / 300s | 失败多为持续前置条件（无卡/无服务），重试太密会反复触发射频激活 |
/// | Native | 5 / 10 / 20 / 40 / 60s | 失败多为瞬时（控制口/网卡尚未就绪），快速重来能显著缩短恢复时间 |
pub fn retry_delay(backend: Backend, fails: u32) -> Duration {
    let f = fails.max(1);
    match backend {
        Backend::Native => {
            let secs = 5u64.saturating_mul(1 << (f - 1).min(4));
            Duration::from_secs(secs.min(60))
        }
        Backend::At => {
            // 30 → 60 → 120 → 240 → 300（封顶）
            let secs = 30u64.saturating_mul(1 << (f - 1).min(4)).min(300);
            Duration::from_secs(secs)
        }
    }
}

/// 每个模组的重试台账（一种后端运行时只会用到其中一种节奏）。
#[derive(Debug, Default, Clone)]
struct Retry {
    /// 上次失败时刻；`None` = 还没失败过（未退避）。
    last: Option<Instant>,
    /// 连续失败次数（成功即清零并删除条目）。
    fails: u32,
    /// 上次失败原因（用于「错误变了就立刻重试」）。
    reason: String,
}

/// 统一退避表：模组 id → 退避状态。
#[derive(Default)]
struct RetryTable(Mutex<HashMap<u32, Retry>>);

impl RetryTable {
    /// 现在允许重试吗（没失败过 / 已过退避窗口）。
    fn due(&self, modem_id: u32, backend: Backend) -> bool {
        let g = self.0.lock().unwrap();
        match g.get(&modem_id) {
            None => true,
            Some(r) => match r.last {
                None => true,
                Some(t) => t.elapsed() >= retry_delay(backend, r.fails.max(1)),
            },
        }
    }

    /// 记一次失败。
    ///
    /// 错误**变化**时清零重数：情况变了（例如控制口刚出现），值得立刻再试一次，
    /// 但连续同一个错误必须持续退避，不能每轮硬砸。
    fn fail(&self, modem_id: u32, reason: String) {
        let mut g = self.0.lock().unwrap();
        let e = g.entry(modem_id).or_default();
        if e.reason != reason {
            e.fails = 0;
            e.reason = reason;
        }
        e.fails = e.fails.saturating_add(1);
        e.last = Some(Instant::now());
    }

    /// 成功：清零并删除（下次失败从第一档重新开始）。
    fn ok(&self, modem_id: u32) {
        self.0.lock().unwrap().remove(&modem_id);
    }

    /// 显式清退避（用户重开开关时用：不该让人等完一个 5 分钟的窗口）。
    fn clear(&self, modem_id: u32) {
        self.0.lock().unwrap().remove(&modem_id);
    }

    fn clear_all(&self) {
        self.0.lock().unwrap().clear();
    }
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

/// 自动连接的统一判定（纯函数，单测钉住所有拒绝理由）。
///
/// 输入全部是**已采集的事实**，这里不读时钟也不碰串口：
/// - `dial_enabled` / `auto_dial`：两个开关（`[net] dial_enabled` / `[net] auto_dial`）；
/// - `busy`：本模组已有拨号在执行（手动任务 or 本协调器自己派出的后台线程）；
/// - `backoff_due`：退避窗口是否已过（[`RetryTable::due`]）。
///
/// 拒绝理由的顺序也重要：先尊重「人」（`manual_stop`），再看开关，
/// 最后才是状态与退避 —— 这样日志里能说清「为什么这一轮没动它」。
pub fn auto_connect_allowed(
    st: &crate::modem::model::DialState,
    dial_enabled: bool,
    auto_dial: bool,
    busy: bool,
    backoff_due: bool,
) -> bool {
    if !dial_enabled || !auto_dial {
        return false;
    }
    if st.manual_stop {
        return false;
    }
    if st.state == DIAL_CONNECTED {
        return false;
    }
    // dialing 且确实有人在跑 → 等它；dialing 但没人跑（进程重启 / 上轮异常退出
    // 留下的标记）→ 允许重来，否则会永远卡在「拨号中」。
    if st.state == DIAL_DIALING && busy {
        return false;
    }
    if busy {
        return false;
    }
    backoff_due
}

/// 拨号协调器（一个进程一个，挂在全局 [`supervisor`] 上）。
#[derive(Default)]
pub struct DialerSupervisor {
    /// 原生会话（按模组 id）。
    #[cfg(unix)]
    native: Mutex<HashMap<u32, native::Session>>,
    /// 统一退避表（两种后端共用一张，按模组隔离）。
    retry: RetryTable,
    /// 本协调器自己派出的 AT 拨号线程（模组 id）。
    ///
    /// 与 `Modem::dial_busy` 的分工：`dial_busy` 防的是**任何**两路拨号并发
    /// （含手动）；这里只防「协调器每 15 秒重复派活」。两者都查，避免
    /// 自动派出的线程还没拿到 `dial_busy` 时又被派一次。
    #[cfg(unix)]
    in_flight: Mutex<std::collections::HashSet<u32>>,
    /// 正在建原生会话的模组（`Session::start` 要几秒，期间可能被 `kick` 并发进入）。
    ///
    /// 没有它时，节拍循环与「用户点了拨号后的立即 kick」会同时看到「没有会话」，
    /// 于是各建一个 —— 模组只有一个数据承载，第二个请求会被拒，但第一路可能
    /// 已经被顶掉。加锁后同一模组同时只有一个建会话流程。
    #[cfg(unix)]
    starting: Mutex<std::collections::HashSet<u32>>,
    /// `AppState` 的句柄（[`Self::kick`] 用）。
    ///
    /// 为什么不是 kick 的入参：`/api/dial/connect` 这类**同步** handler 只拿得到
    /// `&AppState`（路由层统一签名），而 kick 必须把 `Arc` 移进后台线程。
    /// 进程内只有一个协调器，装配完 `AppState` 后 `arm` 一次即可 ——
    /// 异步任务那条路（`tasks.rs`）原本自己持有 `Arc`，一并统一到这里。
    state: OnceLock<Arc<AppState>>,
}

impl DialerSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 停掉一个模组的会话并清退避（用户点了「断开」时调）。
    ///
    /// 顺序语义：只停**会话**，`DialState` 由 `Modem::hangup` 收尾（它会 bump
    /// 代次，作废所有在途流程）。这里不写状态，避免两个写入点。
    pub fn stop_modem(&self, modem_id: u32) {
        #[cfg(unix)]
        self.stop_session(modem_id);
        self.retry.clear(modem_id);
    }

    /// 用户重新打开某个开关（拨号总开关 / 自动拨号 / 关飞行模式）时清退避：
    /// 不该让人等完一个已经毫无意义的 5 分钟窗口。
    pub fn reset_backoff(&self, modem_id: u32) {
        self.retry.clear(modem_id);
    }

    /// 同上，全部模组（开关是全局的）。
    pub fn reset_backoff_all(&self) {
        self.retry.clear_all();
    }

    /// 挂上 `AppState`（进程启动时调一次；之后 [`Self::kick`] 才能用）。
    pub fn arm(&self, state: Arc<AppState>) {
        let _ = self.state.set(state);
    }

    /// 立刻跑一轮协调（不等 15 秒节拍）。
    ///
    /// 只在**后台线程**里跑：调用方刚下发完拨号，不想再等一个节拍才看到会话
    /// 建立，但也不能把请求本身拖住。并发安全由 `starting` 保证 ——
    /// 同一模组同时只会有一个建会话流程。
    ///
    /// QMI/MBIM 必须走这一步：`Modem::dial()` 对这两类只做「交办」，
    /// 真正的会话由本协调器建 —— 页面上点「拨号」后不 kick，就要干等最多
    /// 15 秒才从 `dialing` 变 `connected`。
    pub fn kick(&'static self) {
        let Some(state) = self.state.get().cloned() else {
            log::debug!("拨号协调器还没挂上 AppState，本次 kick 跳过");
            return;
        };
        std::thread::spawn(move || self.reconcile(&state));
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

    /// 每轮组网循环（15 秒）调用：**所有**后端的连接决策都在这里。
    ///
    /// 顺序：先回收已消失模组的会话，再逐台判定（原生 / AT 二选一）。
    #[cfg(unix)]
    pub fn reconcile(&self, state: &AppState) {
        warn_legacy_bin_once(state);
        let running: Vec<std::sync::Arc<Modem>> = state.mgr.all();
        let ids: Vec<u32> = running.iter().map(|m| m.id).collect();

        // 模组没了 → 先收，避免拔掉后还挂着
        let gone: Vec<u32> = self
            .native
            .lock()
            .unwrap()
            .keys()
            .copied()
            .filter(|id| !ids.contains(id))
            .collect();
        for id in gone {
            log::info!("原生拨号：模组 {id} 已不在，停掉会话");
            self.stop_session(id);
            self.retry.clear(id);
            self.in_flight.lock().unwrap().remove(&id);
            crate::netprov::provision::forget_device(id);
        }

        for modem in running {
            if !modem.is_running() {
                self.stop_session(modem.id);
                continue;
            }
            if native_takes(&modem) {
                self.reconcile_native_one(state, &modem);
            } else {
                // 类型切走（例如用户把 usbnet 从 qmi 改成 rndis）时，
                // 旧的原生会话必须收掉，否则两条路径会同时持有承载。
                self.stop_session(modem.id);
                self.reconcile_at_one(state, &modem);
            }
        }
    }

    /// 非 Unix：没有 sysfs / 控制口，什么都不做（与旧实现一致）。
    #[cfg(not(unix))]
    pub fn reconcile(&self, _state: &AppState) {}
}

#[cfg(unix)]
impl DialerSupervisor {
    /// 停掉一个模组的原生会话（重复调用安全；**不清退避**）。
    fn stop_session(&self, modem_id: u32) {
        let s = self.native.lock().unwrap().remove(&modem_id);
        if let Some(mut s) = s {
            s.stop();
            log::info!("原生拨号：模组 {modem_id} 会话已停止");
        }
    }

    /// 该不该继续托管这个模组（三个开关 + 用户意图）。
    ///
    /// 注意 `host_dial` 只关**主机侧拨号器**，不影响 AT 路径 —— 两个开关
    /// 管的是两件事，别合并判。
    fn stop_reason(
        &self,
        state: &AppState,
        st: &crate::modem::model::DialState,
    ) -> Option<&'static str> {
        if !state.cfg.net.host_dial {
            Some("host_dial 关闭")
        } else if !state.dial_enabled() {
            Some("拨号总开关关闭")
        } else if st.manual_stop {
            Some("用户手动断开")
        } else {
            None
        }
    }

    /// 原生后端（QMI/MBIM）的一轮对账。
    fn reconcile_native_one(&self, state: &AppState, modem: &Modem) {
        let id = modem.id;
        let st = modem.dial_status();
        if let Some(why) = self.stop_reason(state, &st) {
            if self.native.lock().unwrap().contains_key(&id) {
                log::info!("原生拨号：模组 {id} 停（{why}）");
                self.stop_session(id);
                // manual_stop 由 hangup 收尾（已经是 idle）；另外两种是策略停用，
                // 状态必须落到 disconnected，不能继续显示「已连接」。
                if !st.manual_stop {
                    modem.note_stopped(why);
                }
            }
            return;
        }

        // APN 四级回退（与一键拨号 / 自动拨号同一套优选语义，见
        // `params::pick_dial_context` 与 `apn_auto::decide`）：拨号状态里记的 →
        // 上次手动拨号用的 → **模组里已配置的上下文**（走 15 秒缓存）→
        // **按识别到的运营商下发预制 APN**（只读缓存证据，本线程不发 AT ——
        // APN 作为拨号器参数直接进 WDS Start / MBIM CONNECT，会话建立即生效）。
        // 早先只有前三级：固件里存了 APN 但从没在页面上拨过的 MBIM 模组，
        // 会陷入「算不出计划（缺 APN）→ 5 秒退避 → 再来」的死循环；
        // 而需要显式下发 APN 的模组（上下文为空）则永远拨不上。
        let apn = if st.apn.trim().is_empty() {
            let last = modem.last_dial_apn();
            if !last.is_empty() {
                last
            } else {
                crate::modem::model::pick_dial_context(&modem.params().contexts)
                    .map(|c| c.apn)
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| {
                        let s = modem.auto_apn_suggestion();
                        (!s.is_empty()).then_some(s)
                    })
                    .unwrap_or_default()
            }
        } else {
            st.apn.clone()
        };
        let device = device_for(modem);
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
                // 算不出计划时**没有会话可停**（压根还没建起来）。退避状态必须
                // 跨轮保留：否则「错误不变就静默 + 指数退避」整体失效（每轮
                // fails 从 0→1，退避永远停在第一档）。错误变化时才由 fail() 清零。
                //
                // 已有会话**不动**：算不出计划 ≠ 现有会话坏了（典型：控制口被
                // 临时占用、网卡名还没出现），贸然拆掉会把唯一能用的连接弄丢。
                let msg = e.to_string();
                self.retry.fail(id, msg.clone());
                log::warn!("原生拨号：模组 {id} 算不出计划: {e}");
                return;
            }
        };

        // 已在跑同一个计划：探活；掉线就重建
        {
            let mut g = self.native.lock().unwrap();
            if let Some(s) = g.get_mut(&id) {
                if s.plan == plan {
                    match s.alive() {
                        Some(false) => {
                            // 真掉线：先把权威状态拉回 dialing（页面立刻能看到），
                            // **不置 manual_stop** —— 这不是用户的断开意图。
                            let epoch = modem.dial_epoch();
                            modem.note_native_down(epoch, "主机侧数据呼叫断开，准备重拨");
                            log::warn!("原生拨号：模组 {id} 数据呼叫已断开，重拨");
                        }
                        // 传输层出错（None）不当作掉线：一次读超时不该把呼叫拆了
                        _ => return,
                    }
                } else {
                    // 连接参数变了（APN / PDN / IPv6 / 网卡）：按新计划重建。
                    // 重建完成前状态是 dialing，页面不会假装还连着。
                    let epoch = modem.dial_epoch();
                    modem.note_native_down(epoch, "连接参数变化，重建会话");
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

        if !self.retry.due(id, Backend::Native) {
            return;
        }

        // 同一模组同时只允许一个建会话流程（节拍循环与 kick 可能撞车）。
        {
            let mut g = self.starting.lock().unwrap();
            if !g.insert(id) {
                return;
            }
        }
        struct Starting(u32);
        impl Drop for Starting {
            fn drop(&mut self) {
                supervisor().starting.lock().unwrap().remove(&self.0);
            }
        }
        let _starting = Starting(id);

        // 代次在**建会话之前**记下：建的过程中用户可能点断开（`hangup` bump），
        // 建好后必须再比对一次 —— 代次变了就立刻拆掉，别让没人要的连接挂着。
        let epoch = modem.dial_epoch();
        match native::Session::start(plan.clone()) {
            Ok(mut s) => {
                if !modem.dial_epoch_is(epoch) {
                    s.stop();
                    log::info!("原生拨号：模组 {id} 会话建好时代次已变，立即拆除");
                    return;
                }
                self.retry.ok(id);
                log::info!("原生拨号：模组 {id} 拉起 {}", plan.describe());
                // 权威状态由后端回调写入（这是早先缺失的一环：页面永远停在拨号中）
                if !modem.note_native_up(epoch, &format!("native-{kind}"), &plan.describe()) {
                    s.stop();
                    return;
                }
                self.native.lock().unwrap().insert(id, s);
            }
            Err(e) => {
                log::warn!(
                    "原生拨号：模组 {id} 建立会话失败（{}）: {e}",
                    plan.describe()
                );
                self.retry.fail(id, e);
            }
        }
    }

    /// AT 路径的一轮对账（ECM / RNDIS / NCM / unknown）。
    ///
    /// 决策与原生后端共用 [`auto_connect_allowed`] 与同一张退避表；
    /// 真正的拨号在**工作线程**里跑，避免一条几十秒的 AT 流程把组网循环
    /// （其它模组的接口配置）堵住。
    fn reconcile_at_one(&self, state: &AppState, modem: &std::sync::Arc<Modem>) {
        let id = modem.id;
        let st = modem.dial_status();
        let busy = modem.dial_in_progress() || self.in_flight.lock().unwrap().contains(&id);
        if !auto_connect_allowed(
            &st,
            state.dial_enabled(),
            state.auto_dial(),
            busy,
            self.retry.due(id, Backend::At),
        ) {
            return;
        }

        let dial_enabled = state.dial_enabled();
        let activate_ims = state.cfg.dial.activate_ims;
        let m = modem.clone();
        let from_state = st.state.clone();
        self.in_flight.lock().unwrap().insert(id);
        let guard = InFlight(id);
        std::thread::spawn(move || {
            let _keep = guard; // 线程结束时（含 panic）自动清标记
            let sup = supervisor();
            // APN 智能配置：识别运营商（COPS/QNWINFO）→ 需要时下发预制 APN
            // （cmnet / 3gnet / ctnet / cbnet）→ 回读确认 → 才进拨号。
            // 识别失败 / 用户显式配置 / 自定义 APN 一律沿用现状（见 apn_auto::decide）。
            // 放在工作线程里做：这几条 AT 不能在组网循环里发（会推迟其它模组的接口对账）。
            let prep = m.auto_apn_prepare();
            if !prep.note.is_empty() {
                log::info!("[auto-dial] modem #{} APN 准备：{}", m.id, prep.note);
            }
            let req = crate::modem::model::DialRequest {
                cid: prep.cid,
                apn: prep.apn,
                pdp_type: prep.pdp_type,
                reconnect: false,
                force: false,
            };
            let r = m.dial(&req, dial_enabled, activate_ims);
            match r {
                Ok(_) => {
                    sup.retry.ok(m.id);
                    log::info!(
                        "[auto-dial] modem #{} 自动拨号成功（此前 state={from_state}）",
                        m.id
                    );
                }
                Err(e) => {
                    // 按**状态**分类，不靠错误文案：
                    //   · 仍是 idle ⇒ 用户断开 / 代次作废，不该记成失败；
                    //   · 仍是 dialing ⇒ 另一路（手动）正在拨，本次只是撞了 busy；
                    //   · failed ⇒ 真失败，进退避。
                    let now = m.dial_status().state;
                    if now == crate::modem::model::dial::DIAL_IDLE
                        || now == DIAL_DIALING
                        || now == DIAL_CONNECTED
                    {
                        log::debug!("[auto-dial] modem #{} 本次未完成（state={now}）：{e}", m.id);
                        return;
                    }
                    let msg = e.to_string();
                    let delay = retry_delay(Backend::At, 1).as_secs();
                    sup.retry.fail(m.id, msg.clone());
                    log::warn!(
                        "[auto-dial] modem #{} 自动拨号失败（至少 {delay} 秒后重试）: {msg}",
                        m.id
                    );
                }
            }
        });
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

    #[test]
    fn retry_delays_share_shape_but_differ_in_scale() {
        // 原生：快速重来（会话失败多为瞬时）
        assert_eq!(retry_delay(Backend::Native, 1).as_secs(), 5);
        assert_eq!(retry_delay(Backend::Native, 2).as_secs(), 10);
        assert_eq!(retry_delay(Backend::Native, 3).as_secs(), 20);
        assert_eq!(retry_delay(Backend::Native, 4).as_secs(), 40);
        assert_eq!(retry_delay(Backend::Native, 5).as_secs(), 60);
        assert_eq!(retry_delay(Backend::Native, 99).as_secs(), 60);
        // AT：持续前置条件，慢一点、封顶 300s（与 fm350 同量级）
        assert_eq!(retry_delay(Backend::At, 1).as_secs(), 30);
        assert_eq!(retry_delay(Backend::At, 2).as_secs(), 60);
        assert_eq!(retry_delay(Backend::At, 3).as_secs(), 120);
        assert_eq!(retry_delay(Backend::At, 4).as_secs(), 240);
        assert_eq!(retry_delay(Backend::At, 5).as_secs(), 300);
        assert_eq!(retry_delay(Backend::At, 99).as_secs(), 300);
    }

    #[test]
    fn retry_table_clears_on_changed_error_and_success() {
        let t = RetryTable::default();
        assert!(t.due(1, Backend::Native), "没失败过就该允许");
        t.fail(1, "A".into());
        assert!(!t.due(1, Backend::Native), "刚失败要退避");
        t.fail(1, "B".into());
        // 错误变化 → 重数（fails 回到 1，窗口 5 秒，仍然没到）
        assert!(!t.due(1, Backend::Native));
        t.ok(1);
        assert!(t.due(1, Backend::Native), "成功后清零，立刻允许");
    }

    #[test]
    fn auto_connect_rejects_each_reason() {
        use crate::modem::model::dial::{DIAL_CONNECTED, DIAL_DIALING, DIAL_FAILED, DIAL_IDLE};
        let mk = |state: &str, manual: bool| crate::modem::model::dial::DialState {
            state: state.into(),
            manual_stop: manual,
            ..Default::default()
        };
        // 正常空闲 → 允许
        assert!(auto_connect_allowed(
            &mk(DIAL_IDLE, false),
            true,
            true,
            false,
            true
        ));
        // 任一开关关掉 → 拒绝
        assert!(!auto_connect_allowed(
            &mk(DIAL_IDLE, false),
            false,
            true,
            false,
            true
        ));
        assert!(!auto_connect_allowed(
            &mk(DIAL_IDLE, false),
            true,
            false,
            false,
            true
        ));
        // 用户手动断开 → 拒绝（尊重人的意图）
        assert!(!auto_connect_allowed(
            &mk(DIAL_IDLE, true),
            true,
            true,
            false,
            true
        ));
        // 已连接 → 拒绝（复用现有连接，绝不无谓断开重拨）
        assert!(!auto_connect_allowed(
            &mk(DIAL_CONNECTED, false),
            true,
            true,
            false,
            true
        ));
        // 拨号中且确实有人在跑 → 等它
        assert!(!auto_connect_allowed(
            &mk(DIAL_DIALING, false),
            true,
            true,
            true,
            true
        ));
        // 拨号中但没人跑（重启残留）→ 允许重来，避免永远卡在「拨号中」
        assert!(auto_connect_allowed(
            &mk(DIAL_DIALING, false),
            true,
            true,
            false,
            true
        ));
        // 失败 + 退避未到 → 拒绝
        assert!(!auto_connect_allowed(
            &mk(DIAL_FAILED, false),
            true,
            true,
            false,
            false
        ));
        // 失败 + 退避已过 → 允许
        assert!(auto_connect_allowed(
            &mk(DIAL_FAILED, false),
            true,
            true,
            false,
            true
        ));
    }
}
