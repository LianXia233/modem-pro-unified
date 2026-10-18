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
use std::sync::Mutex;

use serde::Serialize;

use crate::config::WanMode;
use crate::http::AppState;
use crate::modem::model::{usb_netifs_for_port, Modem};

use super::uci;
use super::v6::{self, V6Mode};
use super::{diff_plan, plan_wan, ExistingIface, UciOp, V6Plan, FIREWALL_ZONE};

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
    //    已被别的接口（lan / wan / br-lan…）占用的口一律排除 —— 不能把用户正在上网的
    //    口当成数据口配掉（实机：eth0 是 LAN、eth1 是 WAN，数据口是 eth2）。
    let occupied: Vec<String> = existing
        .iter()
        .map(|e| e.device.trim().trim_start_matches('@').to_string())
        .filter(|d| !d.is_empty())
        .collect();
    let device = pick_device(modem, &occupied);
    if device.is_empty() {
        rep.skipped
            .push("没能确定这个模组的数据网卡（sysfs 里没有 net/ 子目录）".into());
    }

    // 350 系列（FM350 / RW350）的 USB 网卡不发 DHCP ⇒ 用模组上报的地址直接配 static
    // （参考实现同口径：RNDIS 点对点，地址可直接落在主机网卡上）。
    // 只在 dhcp 模式下改（modem_pro 由 netifd 协议自己取地址）；拿不到地址就维持原模式。
    let static_ipv4 = if matches!(mode, crate::config::WanMode::Dhcp)
        && crate::runtime::hostnic::quirk(&modem.model().unwrap_or_default()).0
    {
        let ip = modem.dial_state_ipv4();
        if ip.is_empty() {
            modem.query_own_ipv4().unwrap_or_default()
        } else {
            ip
        }
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
        let raw = {
            let cached = modem.dial_state_ipv6();
            if cached.is_empty() {
                modem.query_own_ipv6().unwrap_or_default()
            } else {
                cached
            }
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
    let v6_plan = V6Plan::of(v6_mode, &static_v6, state.cfg.net.lan_ipv6);
    let plan = plan_wan(
        modem.id,
        &usb.kind,
        mode,
        &device,
        v6_plan,
        state.cfg.net.wan_metric,
        state.cfg.net.host_dial,
        &static_ipv4,
    );
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
pub(crate) fn qmap_mode(name: &str) -> i64 {
    std::fs::read_to_string(format!("/sys/class/net/{name}/qmap_mode"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
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
}
