//! 组网编排：读现状 → 算计划 → diff → 应用 → 回报。
//!
//! 这一层把 `mod.rs`（纯策略）与 `uci.rs`（薄 IO）接起来，并由
//! `http/loops.rs::spawn_netprov` 每轮调用一次。
//!
//! ## 边界
//!
//! - **不自己开串口**：只处理 `is_running()` 的模组（没人查询过的模组
//!   不该被这个循环唤醒，与其它循环一致）；
//! - **不额外发 AT**：USB 类型走 `Modem::usb_type(false)` 的 30 秒缓存，
//!   命中缓存时一个 AT 都不发；
//! - **零写入就不 reload**：`diff_plan` 一条都不出时连 `uci commit` 都不跑 ——
//!   每轮循环跑到已配好的模组上必须是彻底的空操作，否则会把 UCI 的 commit
//!   时间戳刷成「刚改过」，触发无谓的 network reload。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::config::WanMode;
use crate::http::AppState;
use crate::modem::model::{usb_netifs_for_port, Modem};

use super::uci;
use super::v6::{self, V6Mode};
use super::{
    diff_plan, effective_wan_dns, iface_name, iface_name_v6, occupied_by_others, plan_wan,
    skip_without_device, ExistingIface, UciOp, V6Plan, FIREWALL_ZONE,
};

/// 一次组网的结果（`/api/dial` 的 `auto_wan` 字段 + 日志）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct AutoWanReport {
    /// 完成时刻（RFC3339）。
    pub at: String,
    pub modem_id: u32,
    /// 识别到的拨号类型（`qmi` / `mbim` / `ecm` / `ncm` / `rndis` / `unknown`）。
    pub kind: String,
    /// 生效的接口模式（`modem_pro` / `dhcp` / `both`）。
    pub mode: String,
    /// 计划内的接口名（顺序即主接口在前）。
    pub ifaces: Vec<String>,
    /// 实际下发的 UCI 操作（人话，给页面显示「刚才动了什么」）。
    pub applied: Vec<String>,
    /// 为什么什么都没做（空 = 做了，或本来就不用做）。
    pub skipped: Vec<String>,
    /// 有没有出错（出错时 `applied` 可能是半套）。
    pub ok: bool,
}

/// 各模组最近一次组网结果（`/api/dial` 读它，循环写它）。
#[derive(Default)]
pub struct ReportStore {
    inner: Mutex<HashMap<u32, AutoWanReport>>,
}

impl ReportStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次结果（每个模组只留最近一条）。
    pub fn put(&self, r: AutoWanReport) {
        self.inner.lock().unwrap().insert(r.modem_id, r);
    }

    /// 取某个模组最近一次结果（没有过就是 None）。
    pub fn get(&self, modem_id: u32) -> Option<AutoWanReport> {
        self.inner.lock().unwrap().get(&modem_id).cloned()
    }
}

/// 给一个模组做组网（有改动才落盘）。
pub fn provision_modem(state: &AppState, modem: &Modem) -> AutoWanReport {
    let Ok((existing, zone_sections)) = uci::read_existing() else {
        // 读不到现状就没法 diff；这一层不做「猜着建」这种事
        let mut rep = AutoWanReport {
            at: crate::logging::rfc3339(crate::logging::unix_now()),
            modem_id: modem.id,
            ok: false,
            ..AutoWanReport::default()
        };
        rep.skipped.push("读 UCI 失败".into());
        return rep;
    };
    provision_modem_with(state, modem, &existing, &zone_sections)
}

/// 同上，但 `existing` 由调用方提供（一轮里多个模组共用一次 `uci show`）。
///
/// 共用会不会串？不会：diff 只按 `modem<id>` 这个名字查现状，而各模组的名字
/// 互不相同；防火墙 zone 的 `add_list` 在 UCI 侧是幂等的（重复加不会进两遍）。
pub fn provision_modem_with(
    state: &AppState,
    modem: &Modem,
    existing: &[ExistingIface],
    zone_sections: &std::collections::HashMap<String, String>,
) -> AutoWanReport {
    let mut rep = AutoWanReport {
        at: crate::logging::rfc3339(crate::logging::unix_now()),
        modem_id: modem.id,
        ok: true,
        ..AutoWanReport::default()
    };

    // ① 总开关（`[net] auto_wan`）
    if !state.cfg.net.auto_wan {
        rep.skipped.push("auto_wan 关闭".into());
        return rep;
    }
    // ② 只在 Linux/OpenWrt 上动手（红线 2）
    if !uci::available() {
        rep.skipped.push("uci 不可用（非 OpenWrt）".into());
        return rep;
    }

    // ③ USB 类型（走 30 秒缓存，命中就不发 AT）
    let usb = modem.usb_type(false);
    rep.kind = usb.kind.clone();
    let mode: WanMode = state.wan_mode();
    rep.mode = mode.as_str().to_string();

    // ④ device：按**这个模组的 USB 设备**取网卡，不用全局扫描；
    //    已被**别的**接口（lan / wan / br-lan…）占用的口一律排除 —— 不能把用户正在上网的
    //    口当成数据口配掉（实机：eth0 是 LAN、eth1 是 WAN，数据口是 eth2）。
    //
    //    ⚠️ 排除名单里**不能有本模组自己的接口**（`modem<id>` / `modem<id>_6`）：
    //    第一轮建好 `modem0.device=wwan0` 之后，`existing` 里就有了 `device=wwan0`
    //    这一条；若把它也算「被占用」，`pick_device` 第二轮起就会跳过 wwan0 落到
    //    全局兜底（`detect_data_dev`）—— 那会随手抓一张 `eth2` 或**另一台模组的**
    //    `wwan1`，diff 于是下发 `set modem0.device=…`，接口漂到别的网卡上直接断网。
    //    「占用」的语义是「别人在用」，自己占的不算。
    let occupied: Vec<String> = occupied_by_others(existing, modem.id);
    let device = pick_device(modem, &occupied);
    // 数据网卡还没枚举出来（模组刚插上 / 驱动还没建 netdev）时**不建接口**：
    // 详见 [`super::skip_without_device`]。
    let iface_exists = existing.iter().any(|e| e.name == iface_name(modem.id));
    if let Some(why) = skip_without_device(&device, iface_exists) {
        let owned = usb_netifs_for_port(&modem.port());
        let detail = if owned.is_empty() {
            "sysfs 里还没有 net/ 子目录".to_string()
        } else {
            format!(
                "{} 已存在但都被别的接口占用（{}）",
                owned.join("/"),
                occupied.join("/")
            )
        };
        rep.skipped
            .push(format!("{why}（模组 {}：{detail}）", modem.id));
        return rep;
    }
    // 记下本轮选中的网卡：拨号器托管（`runtime/dialer`）必须拨同一张，
    // 否则「UCI 绑 A 口、拨号器拨 B 口」= 永远拿不到地址（见 [`device_for`]）。
    remember_device(modem.id, &device);

    // 350 系列（FM350 / RW350）的 USB 网卡不发 DHCP ⇒ 用模组上报的地址直接配 static
    // （参考实现同口径：RNDIS 点对点，地址可直接落在主机网卡上）。
    // 只在 dhcp 模式下改（modem_pro 由 netifd 协议自己取地址）；拿不到地址就维持原模式。
    // 350 系列怪癖转换覆盖 **dhcp 与 modem_pro 两种模式**：
    // modem_pro 的 v4 也是跑 udhcpc（mp-client wrapper），而 FM350 的模组内
    // DHCP 服务器实测长期无响应（broadcast discover 无 offer）——
    // 不转换的话 modem_pro 模式在 350 系列上永远拿不到 v4 地址。
    let is_350 = crate::runtime::hostnic::quirk(&modem.model().unwrap_or_default()).0;
    let static_ipv4 = if matches!(
        mode,
        crate::config::WanMode::Dhcp | crate::config::WanMode::ModemPro
    ) && is_350
    {
        // **实时查询优先**：拨号状态/缓存里的地址可能是上一条会话的
        // （重拨后地址会变，实测踩过），拿到新会话地址才算数；查不到
        // 再退回状态缓存（离线排障时至少有迹可循）。
        let ip = match modem.query_own_ipv4() {
            Ok(v) if !v.trim().is_empty() => v,
            _ => modem.dial_state_ipv4(),
        };
        ip
    } else {
        String::new()
    };
    if !static_ipv4.is_empty() {
        log::info!(
            "自动组网 模组 {}：350 系列不走 DHCP，按模组上报地址配 static（{static_ipv4}）",
            modem.id
        );
    }

    // IPv6 取址方式（`[net] wan_ipv6_mode`；口径对齐 `luci-app-fm350` 的 v6.rs）：
    //   · dhcpv6（默认）：只配对兄弟接口形态（proto=dhcpv6 + device=@主接口 +
    //     extendprefix），地址交给 odhcp6c —— 它在用户态收 RA，不吃 accept_ra 的坑；
    //   · ra：proto=none，本进程打开 accept_ra=2 并主动发 RS（见 v6::after_reload）；
    //   · static：把模组上报地址写 /128 —— **必须过滤无前缀残值**，否则会配出
    //     一条永远不通的默认路由（实机 `::18df:...`）；
    //   · off / `[net] wan_ipv6=false`：不建兄弟接口。
    let mut v6_mode = if !state.wan_ipv6() {
        V6Mode::Off
    } else {
        V6Mode::parse(&state.cfg.net.wan_ipv6_mode)
    };
    let mut static_v6 = String::new();
    if v6_mode == V6Mode::Static {
        // **缓存里的值不可用就立刻重查**，不能因为「非空」就信它：
        // ① 拨号状态里的 v6 可能是上一条 PDP 的旧地址；
        // ② `AT+CGPADDR` 在只拿到 v4 时会回一个无前缀残值（实机 `::18df:...`）。
        // 只判 `is_empty()` 会让这两种值永久挡住后续识别 —— 模组后来真拿到
        // v6 了，这边还在用旧残值，表现就是「一直认为没有 IPv6」。
        let cached = modem.dial_state_ipv6();
        let raw = if v6::usable_static_v6(&cached).is_some() {
            cached
        } else {
            modem.query_own_ipv6().unwrap_or(cached)
        };
        match v6::usable_static_v6(&raw) {
            Some(addr) => static_v6 = addr,
            None => {
                log::warn!(
                    "自动组网 模组 {}：IPv6 配成 static，但模组没报可用的全局地址（{:?}），本轮退回 dhcpv6",
                    modem.id,
                    raw
                );
                v6_mode = V6Mode::Dhcpv6;
            }
        }
    }
    // 350 系列：**永远建独立 v6 接口**，与「此刻有没有 v6 地址」解耦。
    // 地址是模组侧 PDP 激活后才有的，组网循环不可能刚好在那个瞬间跑；
    // 先建好 `modem<id>_6`（dhcpv6/ra 由客户端自己反复要地址），再靠下面的
    // `maybe_rekick` 在运行期重识别，才不会被「判断得太早」卡死。
    let v6_plan = V6Plan::of(v6_mode, &static_v6, state.cfg.net.lan_ipv6).dedicated(is_350);
    let mut plan = plan_wan(
        modem.id,
        &usb.kind,
        mode,
        &device,
        v6_plan,
        state.cfg.net.wan_metric,
        state.cfg.net.host_dial,
        &static_ipv4,
    );
    // 强制下发 DNS（`[net] wan_dns`，默认 `119.29.29.29 223.5.5.5 2402:4e00:: 2400:3200::1`）：
    // 只注入主接口，v6 兄弟接口不配 v4 DNS。蜂窝链路 DHCP 常拿不到 DNS（FM350 尤其如此），
    // 写进 network.modem<id>.dns 后 netifd 拉起接口即下发到 resolv.conf.auto。
    // 关掉 IPv6 时不下发 v6 那两条（见 [`super::effective_wan_dns`]）。
    if let Some(main) = plan.ifaces.first_mut() {
        main.dns = effective_wan_dns(&state.cfg.net.wan_dns, state.wan_ipv6());
    }
    rep.ifaces = plan.ifaces.iter().map(|i| i.name.clone()).collect();

    // ⑤ diff → 应用（现状已由调用方读好）
    let zone = if state.cfg.net.wan_zone.trim().is_empty() {
        FIREWALL_ZONE.to_string()
    } else {
        state.cfg.net.wan_zone.trim().to_string()
    };
    // zone 的真实 section 路径：实机的 wan zone 是匿名 `firewall.@zone[1]`，
    // 不能拿 zone 名当 section 名（`firewall.wan` 根本不存在）。
    let zone_section = zone_sections
        .get(&zone)
        .cloned()
        .unwrap_or_else(|| format!("firewall.{zone}"));
    let ops: Vec<UciOp> = diff_plan(existing, &plan, &zone, &zone_section);
    if ops.is_empty() {
        // 已配好：一条都不出。但 **netifd 里可能还没加载**（上一次 reload 失败、
        // 进程重启后配置还在但运行时丢了）—— 这时补一次 reload，否则接口永远 DOWN。
        if plan.ifaces.iter().any(|i| !uci::interface_loaded(&i.name)) {
            log::info!(
                "自动组网 模组 {}：UCI 已符合计划但 netifd 未加载 {}，补一次 reload",
                modem.id,
                plan.ifaces
                    .iter()
                    .map(|i| i.name.as_str())
                    .collect::<Vec<_>>()
                    .join("/")
            );
            if let Err(e) = uci::reload_network() {
                log::warn!("自动组网 模组 {}：补 reload 失败: {e}", modem.id);
            }
        }
        // 350 系列运行期再识别 IPv6：接口形态已经配好，但模组拿到 v6 的时刻
        // 晚于接口建立（PDP 激活 / RA 到达都可能滞后）。这里每轮看一次
        // `ifstatus`：up 着却一直没有全局地址/委派前缀 ⇒ 按退避重新 ifdown/ifup，
        // 让 odhcp6c 重新发 RS/DHCPv6；一旦拿到地址就自动停手。
        // 只对 350 做 —— 其它模组的 v6 由各自模式自己管，不能无端拆接口。
        if is_350 && v6_mode.wants_sibling() {
            let v6_iface = iface_name_v6(modem.id);
            if let Some(live) = v6::iface_live(&v6_iface) {
                if v6::maybe_rekick(&v6_iface, v6_mode, live) {
                    rep.skipped.push(format!("{v6_iface} 无全局 IPv6，已重新拉起"));
                }
            }
        }
        // 形态对但 v6 还没就绪时，按模式补动作（都是幂等的）：
        //   · RA：周期性 RA 可能几百秒才来一轮，主动再要一次（内部 60 秒节流）；
        //   · static：地址是 netifd 异步加的，第一轮 reload 时可能还没上内核，
        //     这里每轮确认一次「有地址就补默认路由」，否则那条路由永远补不上。
        match v6_mode {
            V6Mode::Static => v6::after_reload(v6_mode, &device, state.cfg.net.wan_metric),
            V6Mode::Ra => v6::maybe_resolicit(v6_mode, &device),
            _ => {}
        }
        rep.skipped.push("UCI 已符合计划，无改动".into());
        return rep;
    }
    rep.applied = ops.iter().map(|o| o.describe()).collect();
    log::info!(
        "自动组网 模组 {}（{}，模式 {}）：{}",
        modem.id,
        if rep.kind.is_empty() {
            "unknown"
        } else {
            &rep.kind
        },
        rep.mode,
        rep.applied.join("；")
    );
    match uci::apply(&ops) {
        Ok(_) => {
            // 改完 UCI 要 reload 才生效；失败不影响已写入的配置
            if let Err(e) = uci::reload_network() {
                log::warn!(
                    "自动组网 模组 {}：UCI 已写入，但 network reload 失败: {e}",
                    modem.id
                );
            }
            // IPv6 的落地动作（dhcpv6 什么都不做，交给 odhcp6c）：
            //   ra —— 打开 accept_ra=2 并主动发 RS；
            //   static —— 补一条 default dev 路由（没有 RA 就没有默认路由）。
            v6::after_reload(v6_mode, &device, state.cfg.net.wan_metric);
        }
        Err(e) => {
            rep.ok = false;
            rep.skipped.push(format!("应用 UCI 失败: {e}"));
            log::warn!("自动组网 模组 {} 失败: {e}", modem.id);
        }
    }
    rep
}

/// 各模组最近一次计划里选中的**数据网卡**（模组 id → 网卡名）。
///
/// 只服务于一件事：让拨号器托管（`runtime/dialer`）与组网（`UCI`）拨/绑同一张网卡。
/// 两边各自 `pick_device` 时，入参不同（组网要排除被占用的口、拨号器什么都不排除），
/// 同一台模组完全可能算出两个结果 —— 结果是「接口绑在 wwan0_1、拨号器把呼叫建到
/// wwan0」，QMAP 下地址永远出不来。`pick_device` 的文档里写着「两边必须是同一个
/// 网卡」，这里就是把它落实的机制：组网先选、拨号器跟随。
static PLANNED_DEVICE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();

/// 记下这台模组本轮选中的网卡（空 = 撤回记录，走拨号器自己的兜底）。
fn remember_device(modem_id: u32, device: &str) {
    let m = PLANNED_DEVICE.get_or_init(Default::default);
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if device.trim().is_empty() {
        g.remove(&modem_id);
    } else {
        g.insert(modem_id, device.trim().to_string());
    }
}

/// 拨号器托管要用的数据网卡：优先跟随组网这一轮的结果，没有才自己选。
pub(crate) fn device_for(modem: &Modem) -> String {
    if let Some(m) = PLANNED_DEVICE.get() {
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = g.get(&modem.id) {
            if !d.is_empty() {
                return d.clone();
            }
        }
    }
    // 组网没跑过（`auto_wan` 关着 / 非 OpenWrt / 本轮还没轮到它）：
    // 退回拨号器自己的口径（不排除被占用口 —— 那时没有「接口」可对齐）
    pick_device(modem, &[])
}

/// 模组没了 / 会话回收时清掉记录（id 会被新模组复用，旧网卡名不能留给下一位）。
pub(crate) fn forget_device(modem_id: u32) {
    if let Some(m) = PLANNED_DEVICE.get() {
        m.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&modem_id);
    }
}

/// 挑这个模组该绑的数据网卡（sysfs 按 USB 设备组织，多模组也不会串）。
///
/// `pub(crate)`：拨号器托管（`runtime/dialer`）也要用它给 `-i` 取值 ——
/// 两边必须是同一个网卡，否则 UCI 绑的与拨号器拨的不是一张。
///
/// 优先级（详见各处注释）：
/// 1. **QMAP mux 数据口**（`qmap_mode=1`）—— 父口 `wwan0` 没有 IP，地址在
///    `wwan0_1` 上，绑错父口 netifd 永远 NO-CARRIER；
/// 2. 强候选（名字里带 wwan / rmnet / usb / rndis…）；
/// 3. 其它候选里的第一个。
pub(crate) fn pick_device(modem: &Modem, occupied: &[String]) -> String {
    let onboard = |n: &str| crate::runtime::hostnic::is_onboard_iface(n);
    let names = usb_netifs_for_port(&modem.port());
    // ① QMAP mux 数据口（父口没 IP，地址在 wwan0_1）
    for n in &names {
        if qmap_mode(n) == 1 {
            return n.clone();
        }
    }
    // ② 强候选（**跳过板载口** —— 名字像不等于就是数据口，实机 eth0=LAN、eth1=WAN）
    for n in &names {
        if onboard(n) || occupied.iter().any(|o| o == n) {
            continue;
        }
        let (ok, strong) = crate::modem::model::dial::match_dial_iface(n);
        if ok && strong {
            return n.clone();
        }
    }
    // ③ 同 USB 设备下的第一个非板载、未被占用的口
    for n in &names {
        if !onboard(n) && !occupied.iter().any(|o| o == n) {
            return n.clone();
        }
    }
    // ④ 全局兜底：按**驱动**找数据口（FM350 参考实现同口径），仍排除已被占用的口
    crate::runtime::hostnic::detect_data_dev(occupied).unwrap_or_default()
}

/// 读 `/sys/class/net/<name>/qmap_mode`（不存在或读不到 = 0）。
///
/// `pub(crate)`：拨号器托管靠它决定要不要兜底传 `-n 1`（见 `[net] host_dial_pdn`）。
///
/// 三条路径与 quectel-CM `ql_qmi_qmap_mode_detect()`（`qmap_bridge_mode.c:241-259`）
/// 一致：
///   ① `/sys/class/net/<name>/qmap_mode`（最常见，qmi_wwan 每网卡一个属性）；
///   ② `/sys/module/<驱动>/parameters/qmap_mode`（属性没导出时的模块参数）；
///   ③ `/sys/class/net/<name>/device/driver/module/parameters/qmap_mode`（②的
///      按设备定位版，多驱动共存时不会读到别人的）。
/// 早先只有 ①：OpenWrt 的 `qmi_wwan_q` / `qmi_wwan_f` 只把参数挂在模块上，
/// ① 不存在 ⇒ 漏判 QMAP ⇒ 不下发 `WDA Set Data Format` + `Bind Mux` ⇒
/// 驱动不 `add_mux`，`wwan0_1` 永远 NO-CARRIER（接口初始化不出来的典型根因）。
pub(crate) fn qmap_mode(name: &str) -> i64 {
    let direct = format!("/sys/class/net/{name}/qmap_mode");
    if let Ok(s) = std::fs::read_to_string(&direct) {
        if let Ok(v) = s.trim().parse() {
            return v;
        }
    }
    // ②/③ 需要驱动名：从网卡的 device/driver 符号链接取最后一段
    let driver = std::fs::read_link(format!("/sys/class/net/{name}/device/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();
    if !driver.is_empty() {
        for p in [
            format!("/sys/module/{driver}/parameters/qmap_mode"),
            format!("/sys/class/net/{name}/device/driver/module/parameters/qmap_mode"),
        ] {
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok(v) = s.trim().parse() {
                    return v;
                }
            }
        }
    }
    0
}

/// 跑一轮组网（`http/loops.rs::spawn_netprov` 调用）。
///
/// 一整轮只 fork 两个 `uci show`（不管有几个模组）—— 每个模组各读一遍既浪费
/// 又没必要：diff 只按 `modem<id>` 查现状，各模组名字互不相同。
///
/// ⚠️ 拨号器托管**无条件**跑，不受 `auto_wan` / `uci` 影响：这两个开关管的是
/// 「建不建 UCI 接口」，而拨号器管的是「QMI/MBIM 的数据呼叫谁来做」——
/// 两件事可以独立开关（`auto_wan=false` + `host_dial=true` 是合理组合：
/// 接口用户自己建，拨号器还是 modemd 托管）。
pub fn run_once(state: &AppState) {
    if state.cfg.net.auto_wan && uci::available() {
        match uci::read_existing() {
            Ok((existing, zone_sections)) => {
                for modem in state.mgr.all() {
                    if !modem.is_running() {
                        continue;
                    }
                    let rep = provision_modem_with(state, &modem, &existing, &zone_sections);
                    if !rep.skipped.is_empty() && rep.applied.is_empty() {
                        // 什么都没做：debug 就够（已配好 是常态）
                        log::debug!(
                            "自动组网 模组 {} 跳过：{}",
                            modem.id,
                            rep.skipped.join("；")
                        );
                    }
                    state.netprov.put(rep);
                }
            }
            Err(e) => {
                log::warn!("自动组网：读不到 UCI 现状（network / firewall），本轮跳过: {e}")
            }
        }
    }
    // 拨号器托管放在 UCI 之后：先把接口建好，拨号器起来时网卡才在
    // （QMAP 的 wwan0_1 要等 netifd 把接口拉起来才有）。
    reconcile_global(state);
}

/// 跑一轮拨号器托管（给 `run_once` 调；`OnceLock` 里的全局实例）。
pub(crate) fn reconcile_global(state: &AppState) {
    crate::runtime::dialer::supervisor().reconcile(state);
}

/// 仅供测试/排障：确认 `iface_name` 与计划一致（避免两边写歪）。
#[cfg(test)]
pub(crate) fn sanity() -> bool {
    use super::iface_name;
    let p = plan_wan(
        0,
        "qmi",
        WanMode::ModemPro,
        "wwan0",
        V6Plan::of(V6Mode::Dhcpv6, "", true),
        20,
        true,
        "",
    );
    p.ifaces[0].name == iface_name(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_store_keeps_latest_per_modem() {
        let s = ReportStore::new();
        assert!(s.get(0).is_none());
        s.put(AutoWanReport {
            modem_id: 0,
            applied: vec!["a".into()],
            ..AutoWanReport::default()
        });
        s.put(AutoWanReport {
            modem_id: 0,
            applied: vec!["b".into()],
            ..AutoWanReport::default()
        });
        assert_eq!(s.get(0).unwrap().applied, vec!["b".to_string()]);
        assert!(s.get(1).is_none(), "不同模组互不覆盖");
    }

    #[test]
    fn sanity_plan_name_matches_helper() {
        assert!(sanity());
    }

    #[test]
    fn qmap_mode_of_missing_iface_is_zero() {
        // 三条 sysfs 路径都不存在（测试机 / 非 Linux）时必须是 0，不能 panic：
        // 0 的语义是「不是 QMAP」，拨号器据此跳过 WDA/BindMux，行为确定。
        assert_eq!(qmap_mode("definitely-not-an-iface"), 0);
    }

    #[test]
    fn dialer_follows_the_device_chosen_by_provision() {
        // 拨号器与组网必须拨/绑同一张网卡（QMAP 下绑错就永远 NO-CARRIER）。
        // 只钉「跟随 + 回收」两件事：没跑过组网时的兜底依赖本机网卡，不测。
        let m = crate::modem::model::Modem::new(
            0xF1F1_0001,
            "/definitely/not/a/tty".into(),
            115200,
            std::time::Duration::from_millis(50),
            crate::modem::vendor::Vendor::Unknown,
        );
        let id = m.id;
        forget_device(id);
        remember_device(id, "  wwan0_1  ");
        assert_eq!(
            device_for(&m),
            "wwan0_1",
            "拨号器跟随组网选中的网卡（去空白）"
        );
        remember_device(id, "wwan0");
        assert_eq!(device_for(&m), "wwan0", "换网卡后跟随新值");
        forget_device(id);
        assert!(
            device_for(&m) != "wwan0_1",
            "模组回收后不能把旧网卡留给复用这个 id 的下一位"
        );
    }
}
