//! 主机侧拨号器托管（阶段 6）：QMI / MBIM 模组的数据呼叫由厂商拨号器做。
//!
//! ## 为什么需要它
//!
//! QMI / MBIM 模组的数据通道由**主机侧拨号器**建（quectel-CM 那套 QMI 客户端），
//! 而模组只有**一个数据承载** —— AT 侧再激活一个上下文会把承载占住，拨号器随后
//! 请求数据呼叫就被拒（实测 RM502Q-GL：每轮 `QMUXError 0xe`、`wwan0 link_state 0`）。
//! 所以在 `/api/dial/connect` 里对这两类是**拒绝**而不是代发 `AT+CGACT`。
//! 这个模块补上「谁来跑拨号器」那一半。
//!
//! ## 分层
//!
//! | 文件 | 职责 | 单测 |
//! | --- | --- | --- |
//! | `mod.rs`（本文件） | **纯策略 + 编排**：算命令行、决定该不该跑、按进程组回收 | 全部可 |
//! | `proc.rs` | **薄 IO**：fork、按进程组杀、探活 | 不测（要真进程） |
//!
//! ## 权威语义（`files/etc/modemd/config.toml` §`[net]`，逐条照搬）
//!
//! - 每轮组网循环把它拉起、看着、退出自动重试；**模组拔了 / 手动断开就停掉**；
//! - 进程退出前**按进程组一起收**（连它拉起的 busybox udhcpc 一起），
//!   不留占着控制口的尾巴；
//! - 二进制查找顺序：`/usr/lib/modemd/` → `/usr/bin` → `/usr/sbin`；
//!   `host_dial_bin` 指向不存在的路径**直接报错，不静默退回别的**；
//! - **MBIM 一律用 quectel-CM**（不管模组是哪家）：只有它实现了 MBIM 那套，
//!   而它认 MBIM 靠的是标准 USB CDC 描述符（与厂商无关）；
//! - MBIM 模式下 quectel-CM **必须有 APN**（没 `-s` 会直接 `exit(-404)`）；
//! - `-n <pdn>` 只传给 quectel-CM / fibocom-dial / meig-cm；
//!   **simcom-cm 不认这个参数**；
//! - QMAP 模组：`-i` 给**父口** `wwan0`（驱动属性在它上面），数据在 `wwan0_1`。
//!
//! ## ⚠️ 未验证
//!
//! 四家拨号器的源码**不在本仓库**（构建时由 Makefile 从 third_party 拉），
//! 所以命令行参数是照配置注释与各家的通用用法拼的，**没有在真机上跑过**。
//! 参数集中在本文件的 [`plan_dialer`]，对不上的时候只改那一处。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::http::AppState;
use crate::modem::model::Modem;
use crate::modem::vendor::Vendor;

use crate::netprov::provision::{pick_device, qmap_mode};

pub mod proc;

#[cfg(unix)]
mod mbim;
#[cfg(unix)]
mod native;
#[cfg(unix)]
mod qmi;

/// 全局托管器（`lib.rs` 的退出钩子与组网循环共用同一个）。
static SUPERVISOR: OnceLock<DialerSupervisor> = OnceLock::new();

/// 全局托管器（进程内单例）。
pub(crate) fn supervisor() -> &'static DialerSupervisor {
    SUPERVISOR.get_or_init(DialerSupervisor::new)
}

/// 停止由本进程托管的所有主机侧拨号器（进程退出时调，按进程组一起收）。
pub fn cleanup_all() {
    supervisor().stop_all();
}

/// 一个拨号器实例的计划（纯数据，便于单测）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DialerPlan {
    /// 解析到的可执行文件**绝对路径**。
    pub bin: String,
    /// 命令行参数（不含可执行文件名本身）。
    pub args: Vec<String>,
    /// 这个计划是给谁算的（日志 / 页面显示用）。
    pub modem_id: u32,
}

impl DialerPlan {
    /// 一行可读的命令行（日志里直接打它，排障时能复制出来手跑）。
    pub fn command_line(&self) -> String {
        let mut s = self.bin.clone();
        for a in &self.args {
            s.push(' ');
            s.push_str(a);
        }
        s
    }
}

/// 算不出计划的原因（每一种都要能在页面上说清楚）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// 找不到拨号器可执行文件（含 `host_dial_bin` 指了不存在的路径）。
    BinMissing(String),
    /// MBIM 模式没有 APN（拨号器会直接 `exit(-404)`）。
    MbimNeedsApn,
    /// 没能确定数据网卡（`-i` 不知道给谁）。
    NoDevice,
    /// 没找到 QMI 控制口（`/dev/cdc-wdmN`）—— 原生拨号要用。
    NoWdm(String),
    /// 这个类型不归原生拨号器管（目前只做 QMI）。
    Unsupported(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::BinMissing(p) => write!(f, "拨号器可执行文件不存在: {p}"),
            PlanError::MbimNeedsApn => {
                write!(f, "MBIM 模式必须给 APN（否则拨号器直接退出），请先配置 APN")
            }
            PlanError::NoDevice => write!(f, "没能确定这个模组的数据网卡"),
            PlanError::NoWdm(why) => write!(f, "{why}（原生 QMI 拨号需要 cdc-wdm 控制口）"),
            PlanError::Unsupported(why) => write!(f, "{why}"),
        }
    }
}

/// 二进制查找顺序（配置注释：都先看 `/usr/lib/modemd/`，再看 `/usr/bin`、`/usr/sbin`）。
pub const DIALER_DIRS: [&str; 3] = ["/usr/lib/modemd", "/usr/bin", "/usr/sbin"];

/// 按厂商（与 USB 类型）选拨号器可执行文件名。
///
/// **MBIM 一律 quectel-CM**：只有它实现了 MBIM 那套（`mbim-cm.c` + `SOFTWARE_MBIM`），
/// 而它认 MBIM 靠的是标准 USB CDC 描述符，与模组是哪家无关。
/// `fibocom-dial` / `simcom-cm` 源码里压根没有，`meig-cm` 有文件但没接上。
pub fn dialer_bin_name(vendor: Vendor, kind: &str) -> &'static str {
    if kind.eq_ignore_ascii_case("mbim") {
        return "quectel-CM";
    }
    vendor.dialer_bin()
}

/// 解析拨号器二进制路径（纯函数：只拼路径，不看文件系统）。
///
/// `override_path` 非空时**只用它**（配置注释：指向不存在的路径直接报错，
/// 不静默退回别的）；为空时按 [`DIALER_DIRS`] 的顺序拼。
pub fn resolve_bin_path(override_path: &str, vendor: Vendor, kind: &str) -> String {
    let p = override_path.trim();
    if !p.is_empty() {
        return p.to_string();
    }
    let name = dialer_bin_name(vendor, kind);
    format!("{}/{}", DIALER_DIRS[0], name)
}

/// 从数据网卡名反推 PDN / mux 序号（`wwan0_1` → `1`）。
///
/// 取最后一个 `_` 后面的部分，能解析成**正整数**才算。`wwan0` → `None`
/// （父口没有 mux 序号），`wwan0_1` → `Some(1)`，`rmnet_data0` → `Some(0)`
/// —— 0 不是有效 mux，所以返回 `None`（交给 QMAP 探测那条路）。
pub fn pdn_from_iface(iface: &str) -> Option<i64> {
    let tail = iface.rsplit('_').next()?;
    let n: i64 = tail.parse().ok()?;
    if n > 0 {
        Some(n)
    } else {
        None
    }
}

/// 该不该给拨号器传 `-n <pdn>`。
///
/// 口径（配置注释逐条照搬，`[net] host_dial_pdn`）：
///
/// - `> 0`：强制用这个序号（比如 APN 配在 cid 3 上就写 3）；
/// - `-1`：**永不传**（老行为：只按网卡名反推）；
/// - `0`（默认）：自动 —— 先按数据网卡名反推（`wwan0_1` → 1）；
///   反推不到、但驱动是 QMAP 模式（`qmap_mode = 1`）时按默认承载传 1。
///
/// ⚠️ **simcom-cm 不认这个参数**，所以对它一律不传。
pub fn resolve_pdn(iface: &str, cfg: i64, qmap: i64, accepts_pdn: bool) -> Option<i64> {
    if !accepts_pdn {
        return None;
    }
    match cfg {
        n if n > 0 => Some(n),
        -1 => pdn_from_iface(iface),
        _ => pdn_from_iface(iface).or((qmap == 1).then_some(1)),
    }
}

/// 地址族参数。
///
/// `ipv6 = true`  ⇒ 不加（拨号器默认就是双栈 IPV4V6）
/// `ipv6 = false` ⇒ `-4`（明确只要 IPv4）
///
/// 为什么不照配置注释写「不加 `-6`」就完事：quectel-CM 的 `-6` 是「**只要**
/// IPv6」，不加才是双栈。配置注释里「不给拨号器加 -6」说的是**不额外去开
/// IPv6-only**，对应到参数上就是「关的时候明确给 `-4` 把它按成 v4」。
pub fn family_args(ipv6: bool) -> Vec<&'static str> {
    if ipv6 {
        Vec::new()
    } else {
        vec!["-4"]
    }
}

/// 哪些拨号器认 `-n <pdn>`（配置注释：simcom-cm 不认）。
pub fn accepts_pdn_flag(bin_name: &str) -> bool {
    !bin_name.eq_ignore_ascii_case("simcom-cm")
}

/// 算一个模组该跑什么拨号器（纯函数，无 IO）。
///
/// `device` 是**数据网卡**（QMAP 时是 `wwan0_1`）；`-i` 会自动取它的父口。
#[allow(clippy::too_many_arguments)] // 计划本来就是「模组 + 类型 + APN + 网卡 + 地址族 + PDN」
pub fn plan_dialer(
    modem_id: u32,
    bin_override: &str,
    vendor: Vendor,
    kind: &str,
    apn: &str,
    device: &str,
    ipv6: bool,
    pdn_cfg: i64,
) -> Result<DialerPlan, PlanError> {
    // MBIM 必须有 APN：没 -s 拨号器直接 exit(-404)（配置注释原话）
    if kind.eq_ignore_ascii_case("mbim") && apn.trim().is_empty() {
        return Err(PlanError::MbimNeedsApn);
    }
    if device.trim().is_empty() {
        return Err(PlanError::NoDevice);
    }
    let bin = resolve_bin_path(bin_override, vendor, kind);
    let name = bin.rsplit('/').next().unwrap_or(&bin).to_string();
    let mut args: Vec<String> = Vec::new();

    // APN：留空 = 用拨号器自己的默认 / 模组里已配的
    let apn = apn.trim();
    if !apn.is_empty() {
        args.push("-s".into());
        args.push(apn.to_string());
    }
    // 地址族
    for f in family_args(ipv6) {
        args.push(f.to_string());
    }
    // -i 给父口（QMAP 时驱动属性在 wwan0 上；数据口是 wwan0_1）
    args.push("-i".into());
    args.push(parent_iface(device));
    // -n <pdn>（只给认它的拨号器）
    let qmap = qmap_mode(device);
    if let Some(pdn) = resolve_pdn(device, pdn_cfg, qmap, accepts_pdn_flag(&name)) {
        args.push("-n".into());
        args.push(pdn.to_string());
    }
    Ok(DialerPlan {
        bin,
        args,
        modem_id,
    })
}

/// QMAP 的父口名（`wwan0_1` → `wwan0`；没有 `_` 就原样）。
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

/// 这个 USB 类型的数据呼叫是不是归主机侧拨号器。
///
/// 与 `model::dial::wan_host_dial_by_daemon` 同口径（QMI / MBIM），
/// 这里再包一层是为了让 `runtime` 不依赖 `model` 的内部命名。
pub fn needs_host_dialer(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("qmi") || kind.eq_ignore_ascii_case("mbim")
}

/// 这一轮该不该给这个模组跑拨号器（纯函数，把各种「停」的理由集中在一处）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// 该跑（带着算好的计划）。
    Run(DialerPlan),
    /// 不该跑（原因写给人看）。
    Stop(&'static str),
}

/// 一个在跑的拨号器。
///
/// **必须存 `Child`**：Unix 上父进程不 `wait` 的话，拨号器退出后会变成僵尸，
/// `/proc/<pid>` 依旧存在 —— 那时探活会永远认为它活着，「退出自动重试」失效。
/// 详见 `proc::spawn` 的文档。
#[derive(Debug)]
struct Slot {
    plan: DialerPlan,
    child: std::process::Child,
}

impl Slot {
    fn pid(&self) -> i32 {
        self.child.id() as i32
    }
}

/// 拨号器托管器（一个进程一个，挂在全局 [`supervisor`] 上）。
#[derive(Default)]
pub struct DialerSupervisor {
    slots: Mutex<HashMap<u32, Slot>>,
    /// 原生 QMI 会话（`host_dial_bin = "native"` 时用）。与 `slots` 互斥：
    /// 同一个模组只会走其中一条路。
    #[cfg(unix)]
    native: Mutex<HashMap<u32, native::Session>>,
    /// 原生拨号连续失败时的退避（模组 id → (最后错误, 时刻)）。
    ///
    /// QModem 的重试循环是 `sleep 5` 后继续；这里等价，但**只在这个模组上退避**，
    /// 不影响别的模组，也不会每 15 秒把串口/控制口刷一遍。
    #[cfg(unix)]
    native_retry: Mutex<HashMap<u32, (String, std::time::Instant)>>,
}

impl DialerSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 每个模组最近一次拨号器状态（给页面 / 日志看）。
    pub fn status(&self, modem_id: u32) -> Option<DialerPlan> {
        self.slots
            .lock()
            .unwrap()
            .get(&modem_id)
            .map(|s| s.plan.clone())
    }

    /// 停掉一个模组的拨号器（用户点了「断开」时调）。
    pub fn stop_modem(&self, modem_id: u32) {
        #[cfg(unix)]
        self.stop_native(modem_id);
        let mut g = self.slots.lock().unwrap();
        let Some(mut s) = g.remove(&modem_id) else {
            return;
        };
        let pid = s.pid();
        log::info!(
            "拨号器托管：模组 {modem_id} 手动停止 {}",
            s.plan.command_line()
        );
        drop(g);
        // 先按进程树收（连 udhcpc），再 wait 掉僵尸
        proc::stop(pid);
        proc::reap(&mut s.child);
    }

    /// 全部停掉（进程退出；按进程树一起收，连 udhcpc）。
    pub fn stop_all(&self) {
        #[cfg(unix)]
        {
            let all: Vec<(u32, native::Session)> = self.native.lock().unwrap().drain().collect();
            for (id, mut s) in all {
                s.stop();
                log::info!("原生拨号：模组 {id} 随进程退出停止");
            }
        }
        let mut g = self.slots.lock().unwrap();
        let all: Vec<Slot> = g.drain().map(|(_, s)| s).collect();
        drop(g);
        for mut s in all {
            proc::stop(s.pid());
            proc::reap(&mut s.child);
        }
    }

    /// 收尸：拨号器自己退了就把记录删掉，下一段 `want_for` 会重拉。
    ///
    /// 配置注释的「退出自动重试」就是这一行。没有它是个隐蔽的坑：slot 还在、
    /// 计划也没变，`reconcile` 会认为「已经在跑同一个计划」而跳过 —— 于是
    /// **再也不起来**。这个 bug 单测抓不到：`reconcile` 的输入是真进程的死活，
    /// 纯函数测试里 `status()` 永远等于计划。是推演「拨号器自己退了会怎样」
    /// 发现的。
    ///
    /// 只 warn 不 error：拨号器退出有时候是**故意的**（比如 MBIM 拨号器发现
    /// 没 APN 自己 `exit(-404)`），那条路径在 `want_for` 里会给出人话原因。
    fn reap_dead(&self, modem_id: u32) {
        let mut g = self.slots.lock().unwrap();
        let Some(s) = g.get_mut(&modem_id) else {
            return;
        };
        let pid = s.pid();
        if proc::alive(pid) {
            return;
        }
        // 已退出（可能还是僵尸）：先把可能残留的 udhcpc 收掉，再 wait 干净
        let plan = s.plan.clone();
        proc::stop(pid);
        proc::reap(&mut s.child);
        g.remove(&modem_id);
        drop(g);
        log::warn!(
            "拨号器托管：模组 {modem_id} 的拨号器已退出（{}），下一轮重拉",
            plan.command_line()
        );
    }

    /// 每轮组网循环调用：该跑的拉起 / 看着，不该跑的收起。
    pub fn reconcile(&self, state: &AppState) {
        if !cfg!(unix) {
            return; // 进程组托管是 Unix 的事；Windows 上压根没有这些拨号器
        }
        let running: Vec<u32> = state.mgr.all().iter().map(|m| m.id).collect();
        // ① 模组没了 → 停（先收，再判「该跑什么」，避免拔掉后还挂着）
        {
            let mut g = self.slots.lock().unwrap();
            let gone: Vec<u32> = g
                .keys()
                .copied()
                .filter(|id| !running.contains(id))
                .collect();
            for id in gone {
                if let Some(s) = g.remove(&id) {
                    log::info!("拨号器托管：模组 {id} 已不在，停掉拨号器");
                    drop(g);
                    proc::stop(s.pid());
                    g = self.slots.lock().unwrap();
                }
            }
        }
        // 原生拨号是显式开关（默认仍用外部拨号器）。走不走原生由
        // 「链路类型 + 厂商策略」共同决定：比如 fibocom-dial 没有 MBIM 实现，
        // 它的 MBIM 模组就继续交给外部二进制，而不是让原生后端去试一遍再失败。
        let native_on = native_enabled(state);
        for modem in state.mgr.all() {
            if !modem.is_running() {
                self.stop_modem(modem.id);
                continue;
            }
            if native_on && native_takes(&modem) {
                #[cfg(unix)]
                self.reconcile_native_one(state, &modem);
                continue;
            }
            // ① 拨号器自己退了 ⇒ 收尸，下一段会重拉（配置注释：退出自动重试）
            self.reap_dead(modem.id);
            match want_for(state, &modem) {
                Want::Stop(reason) => {
                    if self.status(modem.id).is_some() {
                        log::info!("拨号器托管：模组 {} 停（{}）", modem.id, reason);
                        self.stop_modem(modem.id);
                    }
                }
                Want::Run(plan) => {
                    // 已在跑同一个计划 ⇒ 不动（每轮循环跑到已配好的模组必须是空操作）
                    if self.status(modem.id).as_ref() == Some(&plan) {
                        continue;
                    }
                    // 计划变了（换了 APN / 换了网卡）⇒ 停下旧的再起新的
                    if self.status(modem.id).is_some() {
                        self.stop_modem(modem.id);
                    }
                    match proc::spawn(&plan) {
                        Ok(child) => {
                            let pid = child.id() as i32;
                            log::info!(
                                "拨号器托管：模组 {} 拉起 {}（pid {pid}）",
                                modem.id,
                                plan.command_line()
                            );
                            self.slots
                                .lock()
                                .unwrap()
                                .insert(modem.id, Slot { plan, child });
                        }
                        Err(e) => log::warn!(
                            "拨号器托管：模组 {} 拉起失败（{}）: {e}",
                            modem.id,
                            plan.command_line()
                        ),
                    }
                }
            }
        }
    }
}

/// 这一个模组这一轮该跑什么（纯决策，IO 都在 `proc` 里）。
fn want_for(state: &AppState, modem: &Modem) -> Want {
    // 总开关
    if !state.cfg.net.host_dial {
        return Want::Stop("host_dial 关闭");
    }
    if !state.dial_enabled() {
        return Want::Stop("拨号总开关关闭");
    }
    let st = modem.dial_status();
    if st.manual_stop {
        return Want::Stop("用户手动断开");
    }
    let kind = modem.cached_dial_kind();
    if !needs_host_dialer(&kind) {
        return Want::Stop("非 QMI/MBIM 类型（走模组侧 AT 拨号）");
    }
    // device + APN
    // 拨号器用的是同一张数据口；这里没有 UCI 现状可排除，按驱动/VID 探测兜底
    let device = pick_device(modem, &[]);
    let apn = if st.apn.trim().is_empty() {
        modem.last_dial_apn()
    } else {
        st.apn.clone()
    };
    let vendor = modem.vendor();
    // `host_dial_bin` 写成 native 时，外部这条只可能是 MBIM：用默认二进制名，
    // 不能把 "native" 当路径（那会 BinMissing）。
    let raw_bin = state.cfg.net.host_dial_bin.trim().to_string();
    let bin_override = if is_native_bin_name(&raw_bin) {
        String::new()
    } else {
        raw_bin
    };
    match plan_dialer(
        modem.id,
        &bin_override,
        vendor,
        &kind,
        &apn,
        &device,
        state.wan_ipv6(),
        state.cfg.net.host_dial_pdn,
    ) {
        Ok(p) => Want::Run(p),
        Err(e) => {
            // 算不出计划也要把话说清楚，但**不要**每轮都刷一遍 warn：
            // MBIM 没配 APN 是常态（用户还没填），那属于「等配置」不是「出错」。
            match e {
                PlanError::MbimNeedsApn => Want::Stop("MBIM 缺 APN，等配置"),
                other => {
                    log::warn!("拨号器托管：模组 {} 算不出计划: {other}", modem.id);
                    Want::Stop("算不出拨号计划")
                }
            }
        }
    }
}

/// `host_dial_bin` 里的原生开关（`native` / `rust` / `builtin`）。
pub fn is_native_bin_name(raw: &str) -> bool {
    let b = raw.trim();
    b.eq_ignore_ascii_case("native")
        || b.eq_ignore_ascii_case("rust")
        || b.eq_ignore_ascii_case("builtin")
}

/// 这一轮是不是该用原生拨号（`host_dial_bin` 显式写成 native）。
fn native_enabled(state: &AppState) -> bool {
    cfg!(unix) && is_native_bin_name(&state.cfg.net.host_dial_bin)
}

/// 这个模组能不能交给原生后端（链路类型 + 厂商策略都支持才交给它）。
fn native_takes(modem: &Modem) -> bool {
    let kind = modem.cached_dial_kind();
    let link = match kind.trim().to_ascii_lowercase().as_str() {
        "qmi" => crate::runtime::dialer::native::vendors::DataLink::Qmi,
        "mbim" => crate::runtime::dialer::native::vendors::DataLink::Mbim,
        _ => return false,
    };
    crate::runtime::dialer::native::vendors::supports(modem.vendor(), link)
}

#[cfg(unix)]
impl DialerSupervisor {
    /// 停掉一个模组的原生会话（重复调用安全）。
    fn stop_native(&self, modem_id: u32) {
        let s = self.native.lock().unwrap().remove(&modem_id);
        self.native_retry.lock().unwrap().remove(&modem_id);
        if let Some(mut s) = s {
            s.stop();
            log::info!("原生拨号：模组 {modem_id} 会话已停止");
        }
    }

    /// 原生后端的一轮对账（QMI / MBIM 同一套）：该建的建、该停的停、掉线重拨、失败退避 5 秒。
    ///
    /// 与外部拨号器的 `want_for` 共用同一套「停」的条件（总开关 / 手动断开），
    /// 所以用户点「断开」时两条路的行为一致。
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

        let apn = if st.apn.trim().is_empty() {
            modem.last_dial_apn()
        } else {
            st.apn.clone()
        };
        let device = pick_device(modem, &[]);
        let plan = match native::plan_native(
            id,
            modem.vendor(),
            &modem.cached_dial_kind(),
            &apn,
            &device,
            state.wan_ipv6(),
            state.cfg.net.host_dial_pdn,
        ) {
            Ok(p) => p,
            Err(e) => {
                self.stop_native(id);
                // 错误不变就只在第一次打 warn（否则每 15 秒刷一行）
                let mut g = self.native_retry.lock().unwrap();
                let msg = e.to_string();
                let changed = g.get(&id).map(|(m, _)| m != &msg).unwrap_or(true);
                if changed {
                    log::warn!("原生拨号：模组 {id} 算不出计划: {e}");
                }
                g.insert(id, (msg, std::time::Instant::now()));
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

        // 失败退避 5 秒（QModem 的重试循环同节奏）
        {
            let g = self.native_retry.lock().unwrap();
            if let Some((_, at)) = g.get(&id) {
                if at.elapsed() < std::time::Duration::from_secs(5) {
                    return;
                }
            }
        }

        match native::Session::start(plan.clone()) {
            Ok(s) => {
                self.native_retry.lock().unwrap().remove(&id);
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
            }
        }
    }
}

impl DialerSupervisor {
    /// 这个模组当前用的是哪种后端（给 `/api/dial` 的 `host_dialer` 字段）。
    pub fn describe(&self, modem_id: u32) -> serde_json::Value {
        #[cfg(unix)]
        if let Some(s) = self.native.lock().unwrap().get(&modem_id) {
            return s.status();
        }
        if let Some(p) = self.status(modem_id) {
            return serde_json::json!({
                "backend": "external",
                "bin": p.bin,
                "args": p.args,
                "cmd": p.command_line(),
            });
        }
        serde_json::Value::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(
        kind: &str,
        apn: &str,
        device: &str,
        ipv6: bool,
        pdn: i64,
    ) -> Result<DialerPlan, PlanError> {
        plan_dialer(0, "", Vendor::Quectel, kind, apn, device, ipv6, pdn)
    }

    // ---------- 二进制选择 ----------

    #[test]
    fn mbim_always_uses_quectel_cm_regardless_of_vendor() {
        // 只有 quectel-CM 实现了 MBIM；认它靠标准 USB CDC 描述符，与厂商无关
        for v in [
            Vendor::Fibocom,
            Vendor::Simcom,
            Vendor::Meig,
            Vendor::TDTech,
        ] {
            assert_eq!(dialer_bin_name(v, "mbim"), "quectel-CM");
            assert_eq!(dialer_bin_name(v, "MBIM"), "quectel-CM");
        }
        assert_eq!(dialer_bin_name(Vendor::Quectel, "qmi"), "quectel-CM");
        assert_eq!(dialer_bin_name(Vendor::Fibocom, "qmi"), "fibocom-dial");
        assert_eq!(dialer_bin_name(Vendor::Meig, "qmi"), "meig-cm");
        assert_eq!(dialer_bin_name(Vendor::Simcom, "qmi"), "simcom-cm");
    }

    #[test]
    fn override_bin_wins_and_is_not_silently_replaced() {
        // 配置注释：指向不存在的路径直接报错，不静默退回别的
        let p = resolve_bin_path("/opt/my-dialer", Vendor::Fibocom, "qmi");
        assert_eq!(p, "/opt/my-dialer");
        let p = resolve_bin_path("  /opt/my-dialer  ", Vendor::Simcom, "qmi");
        assert_eq!(p, "/opt/my-dialer");
    }

    #[test]
    fn auto_bin_lives_in_lib_dir() {
        assert_eq!(
            resolve_bin_path("", Vendor::Quectel, "qmi"),
            "/usr/lib/modemd/quectel-CM"
        );
        assert_eq!(
            resolve_bin_path("", Vendor::Fibocom, "qmi"),
            "/usr/lib/modemd/fibocom-dial"
        );
    }

    #[test]
    fn dialer_dirs_are_the_documented_order() {
        assert_eq!(DIALER_DIRS, ["/usr/lib/modemd", "/usr/bin", "/usr/sbin"]);
    }

    // ---------- PDN ----------

    #[test]
    fn pdn_from_iface_takes_mux_suffix() {
        assert_eq!(pdn_from_iface("wwan0_1"), Some(1));
        assert_eq!(pdn_from_iface("wwan0_2"), Some(2));
        assert_eq!(pdn_from_iface("wwan0if"), None);
        // 父口没有 mux 序号
        assert_eq!(pdn_from_iface("wwan0"), None);
        // 0 不是有效 mux（拨号器的 -n 0 表示自动）
        assert_eq!(pdn_from_iface("rmnet_data0"), None);
        assert_eq!(pdn_from_iface(""), None);
    }

    #[test]
    fn resolve_pdn_auto_uses_iface_then_qmap() {
        assert!(accepts_pdn_flag("quectel-CM"));
        assert!(accepts_pdn_flag("fibocom-dial"));
        assert!(accepts_pdn_flag("meig-cm"));
        // simcom-cm 不认这个参数：一律不传
        assert!(!accepts_pdn_flag("simcom-cm"));
        assert_eq!(resolve_pdn("wwan0_1", 0, 0, true), Some(1));
        // 反推不到、驱动是 QMAP ⇒ 按默认承载传 1
        assert_eq!(resolve_pdn("wwan0", 0, 1, true), Some(1));
        assert_eq!(resolve_pdn("wwan0", 0, 0, true), None);
    }

    #[test]
    fn resolve_pdn_cfg_overrides() {
        assert_eq!(resolve_pdn("wwan0_1", 3, 0, true), Some(3), "强制用配置值");
        assert_eq!(resolve_pdn("wwan0", 3, 1, true), Some(3));
        // -1 = 永不按 QMAP 兜底，只按网卡名反推
        assert_eq!(resolve_pdn("wwan0", -1, 1, true), None);
        assert_eq!(resolve_pdn("wwan0_1", -1, 0, true), Some(1));
    }

    #[test]
    fn resolve_pdn_respects_dialer_capability() {
        assert_eq!(resolve_pdn("wwan0_1", 0, 1, false), None);
        assert_eq!(resolve_pdn("wwan0_1", 5, 0, false), None, "强制值也不硬塞");
    }

    // ---------- 地址族 ----------

    #[test]
    fn family_args_only_pin_v4_when_ipv6_off() {
        assert!(family_args(true).is_empty(), "双栈是拨号器默认，不用传");
        assert_eq!(family_args(false), vec!["-4"]);
    }

    // ---------- 计划 ----------

    #[test]
    fn plan_qmi_full() {
        let p = plan("qmi", "cmnet", "wwan0_1", true, 0).unwrap();
        assert_eq!(p.bin, "/usr/lib/modemd/quectel-CM");
        // -s apn、-i 父口、-n 1（从网卡名反推）；双栈不加地址族参数
        assert_eq!(p.args, vec!["-s", "cmnet", "-i", "wwan0", "-n", "1"]);
    }

    #[test]
    fn plan_qmi_v4_only_adds_dash4() {
        let p = plan("qmi", "cmnet", "wwan0", false, 0).unwrap();
        assert_eq!(p.args, vec!["-s", "cmnet", "-4", "-i", "wwan0"]);
    }

    #[test]
    fn plan_without_apn_omits_dash_s() {
        // 留空 = 用拨号器自己的默认 / 模组里已配的
        let p = plan("qmi", "  ", "wwan0", true, 0).unwrap();
        assert!(!p.args.iter().any(|a| a == "-s"));
    }

    #[test]
    fn plan_mbim_requires_apn() {
        // 没 -s 拨号器直接 exit(-404)
        assert_eq!(
            plan("mbim", "", "wwan0", true, 0),
            Err(PlanError::MbimNeedsApn)
        );
        let p = plan("mbim", "cmnet", "wwan0", true, 0).unwrap();
        assert_eq!(p.bin, "/usr/lib/modemd/quectel-CM");
        assert_eq!(p.args, vec!["-s", "cmnet", "-i", "wwan0"]);
    }

    #[test]
    fn plan_mbim_ignores_vendor_choice() {
        let p = plan_dialer(1, "", Vendor::Fibocom, "mbim", "apn", "wwan0", true, 0).unwrap();
        assert_eq!(p.bin, "/usr/lib/modemd/quectel-CM");
    }

    #[test]
    fn plan_simcom_never_gets_dash_n() {
        let p = plan_dialer(0, "", Vendor::Simcom, "qmi", "apn", "wwan0_1", true, 0).unwrap();
        assert_eq!(p.bin, "/usr/lib/modemd/simcom-cm");
        assert!(!p.args.iter().any(|a| a == "-n"), "simcom-cm 不认 -n");
        assert_eq!(p.args, vec!["-s", "apn", "-i", "wwan0"]);
    }

    #[test]
    fn plan_uses_parent_iface_for_dash_i() {
        // QMAP：驱动属性在父口 wwan0 上，数据在 wwan0_1
        assert_eq!(parent_iface("wwan0_1"), "wwan0");
        assert_eq!(parent_iface("wwan0"), "wwan0");
        assert_eq!(parent_iface("usb0"), "usb0");
        assert_eq!(parent_iface("_1"), "_1", "空前缀不砍");
    }

    #[test]
    fn plan_requires_device() {
        assert_eq!(plan("qmi", "apn", "  ", true, 0), Err(PlanError::NoDevice));
    }

    #[test]
    fn command_line_is_copy_pasteable() {
        let p = plan("qmi", "cmnet", "wwan0_1", true, 0).unwrap();
        assert_eq!(
            p.command_line(),
            "/usr/lib/modemd/quectel-CM -s cmnet -i wwan0 -n 1"
        );
    }

    // ---------- 判据 ----------

    #[test]
    fn needs_host_dialer_matches_dial_model() {
        assert!(needs_host_dialer("qmi"));
        assert!(needs_host_dialer("QMI"));
        assert!(needs_host_dialer("mbim"));
        assert!(!needs_host_dialer("ecm"));
        assert!(!needs_host_dialer("ncm"));
        assert!(!needs_host_dialer("rndis"));
        assert!(!needs_host_dialer("unknown"));
        assert!(!needs_host_dialer(""));
    }

    #[test]
    fn plan_error_messages_are_actionable() {
        assert!(PlanError::MbimNeedsApn.to_string().contains("APN"));
        assert!(PlanError::BinMissing("/x".into())
            .to_string()
            .contains("/x"));
        assert!(PlanError::NoDevice.to_string().contains("网卡"));
    }

    #[test]
    fn plans_compare_by_content() {
        // reconcile 靠它判断「计划没变就别动」
        let a = plan("qmi", "cmnet", "wwan0_1", true, 0).unwrap();
        let b = plan("qmi", "cmnet", "wwan0_1", true, 0).unwrap();
        let c = plan("qmi", "other", "wwan0_1", true, 0).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
