//! IPv6 取址方式与主机侧落地（口径对齐 `luci-app-fm350` 的 `net/v6.rs`）。
//!
//! ## 为什么 350 系列的 IPv6 不能照抄 IPv4 的静态方案
//!
//! FM350 / RW350 的 USB 数据网卡连 IPv4 的 DHCP 都不发（见 `runtime/hostnic` 的
//! `quirk`），IPv6 更不能用「照着 `AT+CGPADDR` 写 /128」这一套：
//!
//! | 模式 | 谁负责地址 | 本程序做什么 |
//! | --- | --- | --- |
//! | `dhcpv6`（默认） | odhcp6c（**用户态** raw socket 收 RA） | 只把 `modem<id>_6` 的形态配对（`proto=dhcpv6` + `device=@<主接口>` + `extendprefix=1`），不写地址、不改 sysctl |
//! | `ra` | 内核按 RA/SLAAC | `proto=none` + `accept_ra=2`，并**主动发 RS**（写 `router_solicitations`） |
//! | `static` | 本程序从模组侧读出后写 `/128` | 写 `ip6addr` + 补一条 `default dev` 路由 |
//! | `off` | 不托管 | 拆掉兄弟接口 |
//!
//! 默认选 `dhcpv6` 是有依据的：odhcp6c 在用户态收发 RS/RA 与 DHCPv6，**不经过**
//! 内核 `accept_ra`，所以 `forwarding=1`（接口进 WAN 区后的必然结果）也不会把它挡掉；
//! `extendprefix=1` 还能把拿到的 `/64` 继续委派给 LAN（内网设备也有 IPv6）。
//!
//! ## 三个实机坑（逐条都在下面代码里挡着）
//!
//! 1. **`accept_ra` 被 forwarding 关死**：蜂窝网卡进 WAN 区后
//!    `net.ipv6.conf.<dev>.forwarding=1`，内核默认 `accept_ra=1` 会**直接丢弃**
//!    所有 RA；OpenWrt 给 WAN 网卡写的常常是 0。只有 `2` 表示「即使转发也接收」。
//! 2. **只设 `accept_ra` 是被动等**：周期性 RA 间隔可达数百秒，等不到就表现为
//!    「一直没有 IPv6」。原厂 `mtk_netagent` 的硬动作是往
//!    `/proc/sys/net/ipv6/conf/<dev>/router_solicitations` 写 1，让内核**立刻**发 RS。
//! 3. **静态地址本身可能是残值**：`AT+CGPADDR` 的 v6 位在上下文去激活后仍返回上一轮
//!    的地址，而且可能是「只有接口标识、没有前缀」的占位（实机
//!    `::18df:9a01:f982:815a`）。照抄会配出一条永远不通的路由 —— 所以写入前必须过滤。
//!
//! `modem<id>_6` 的 UCI 形态由 [`crate::netprov::plan_wan`] 算（纯策略），本模块
//! 只放「判定 + 落地动作」（sysctl / RS / 路由），两者都不碰 AT。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::logging::unix_now;

/// IPv6 取址方式（`[net] wan_ipv6_mode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum V6Mode {
    /// 交给 netifd 的 odhcp6c（用户态收 RA + DHCPv6）。**默认**。
    #[default]
    Dhcpv6,
    /// 内核按运营商 RA 自动配置（需要 `accept_ra=2` + 主动 RS）。
    Ra,
    /// 从模组侧读出地址后写 `/128`（旧行为，仅在明确需要固定地址时用）。
    Static,
    /// 不托管 IPv6（不建兄弟接口）。
    Off,
}

impl V6Mode {
    /// 四种模式，顺序即界面上该有的顺序（默认的排第一）。
    pub const ALL: [V6Mode; 4] = [V6Mode::Dhcpv6, V6Mode::Ra, V6Mode::Static, V6Mode::Off];

    /// 解析配置值。**认不出的值一律回落到 `dhcpv6`**（与参考实现 1.0.11 起同口径）：
    /// 早先把「不是那三个值」都当 RA，结果配置缺省/写错时掉进 RA 分支，
    /// 用户看到的就是「默认拿到的 IPv6 不对」。
    pub fn parse(raw: &str) -> V6Mode {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ra" | "slaac" => V6Mode::Ra,
            "static" | "fix" => V6Mode::Static,
            "off" | "none" | "disable" | "0" => V6Mode::Off,
            _ => V6Mode::Dhcpv6,
        }
    }

    /// 归一化后的配置值（写回 / 日志用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            V6Mode::Dhcpv6 => "dhcpv6",
            V6Mode::Ra => "ra",
            V6Mode::Static => "static",
            V6Mode::Off => "off",
        }
    }

    /// 一句中文说明（页面 / 报告用）。
    pub fn label(&self) -> &'static str {
        match self {
            V6Mode::Dhcpv6 => "DHCPv6（odhcp6c 收 RA，/64 委派给 LAN）",
            V6Mode::Ra => "RA/SLAAC（内核按 RA 配置，主动发 RS）",
            V6Mode::Static => "静态（照模组上报地址写 /128）",
            V6Mode::Off => "不托管",
        }
    }

    /// 要不要建 `modem<id>_6` 兄弟接口。
    pub fn wants_sibling(&self) -> bool {
        !matches!(self, V6Mode::Off)
    }

    /// 该接口的 proto（`none` 表示只把设备拉起来、地址交给内核 RA）。
    pub fn sibling_proto(&self) -> &'static str {
        match self {
            V6Mode::Dhcpv6 => "dhcpv6",
            V6Mode::Ra => "none",
            V6Mode::Static => "static",
            V6Mode::Off => "none",
        }
    }
}

/// 页面/接口清单里的四种模式（`/api/dial` 的 `wan_ipv6_modes`）。
pub fn v6_modes_payload() -> Vec<serde_json::Value> {
    V6Mode::ALL
        .iter()
        .map(|m| {
            serde_json::json!({
                "value": m.as_str(),
                "label": m.label(),
                "default": matches!(m, V6Mode::Dhcpv6),
            })
        })
        .collect()
}

// ---------------------------------------------------------------- 地址判定

/// 是否为 IPv6 链路本地地址（`fe80::/10`）。
pub fn is_link_local_v6(ip: &str) -> bool {
    let first = ip.trim().split(':').next().unwrap_or("");
    match u16::from_str_radix(first, 16) {
        Ok(v) => v & 0xffc0 == 0xfe80,
        Err(_) => false,
    }
}

/// 这个地址能不能当**全局地址**用（过滤链路本地 / 回环 / 组播 / 无前缀残值）。
///
/// 「无前缀残值」是实机踩出来的：`AT+CGPADDR=1` 在 PDP 只拿到 IPv4 时，
/// v6 位照样回 `0.0.0.0.0.0.0.0.24.223.154.1.249.130.129.90`
/// （解码即 `::18df:9a01:f982:815a`）—— 只有接口标识、前缀全零。
/// 这种地址写进 UCI 只会得到一条永远不通的默认路由，必须当成「没有」。
pub fn is_usable_global_v6(ip: &str) -> bool {
    let ip = ip.trim().trim_end_matches('/');
    if ip.is_empty() {
        return false;
    }
    let lower = ip.to_ascii_lowercase();
    // 未指定 / 回环 / 只有接口标识（`::xxxx:...`，前缀为零）
    if lower == "::" || lower == "::1" || lower.starts_with("::") {
        return false;
    }
    // 链路本地、组播
    if is_link_local_v6(&lower) {
        return false;
    }
    let first = lower.split(':').next().unwrap_or("");
    if let Ok(v) = u16::from_str_radix(first, 16) {
        if v & 0xff00 == 0xff00 {
            return false; // ff00::/8 组播
        }
    }
    // 只接受 8 组以内的冒号写法（`ip -o addr` 那种会带 `%eth2` 后缀）
    lower.contains(':')
}

/// 从模组侧读到的一串地址里挑一个能用的全局 IPv6（可带 `/prefix`）。
///
/// 返回**去掉前缀长度**的地址：`AT+CGPADDR` 不带前缀，写 `/128` 前也只需要地址本身。
pub fn usable_static_v6(raw: &str) -> Option<String> {
    let addr = raw.trim();
    let addr = addr.split('/').next().unwrap_or("").trim();
    if is_usable_global_v6(addr) {
        Some(addr.to_ascii_lowercase())
    } else {
        None
    }
}

/// 从一行 `ip -o -6 addr` 输出里取仍在有效期内的全局 IPv6。
///
/// 只看 `valid_lft 0sec`：`preferred_lft 0sec` 只表示 deprecated，
/// 在 `valid_lft` 归零前仍是可用地址。
pub fn usable_v6_from_addr_line(line: &str) -> Option<String> {
    let p = line.find("inet6 ")?;
    let rest = &line[p + 6..];
    let addr = rest.split_whitespace().next()?;
    let ip = addr.split('/').next().unwrap_or("");
    if !is_usable_global_v6(ip) {
        return None;
    }
    // 有效期：`valid_lft 0sec` 的地址内核已经弃用，不能再当「有 IPv6」
    let mut it = line.split_whitespace();
    while let Some(tok) = it.next() {
        if tok == "valid_lft" {
            match it.next() {
                Some("forever") => break,
                Some(v) if v.ends_with("sec") => {
                    let n = v.trim_end_matches("sec").parse::<u64>().unwrap_or(1);
                    if n == 0 {
                        return None;
                    }
                    break;
                }
                _ => break,
            }
        }
    }
    Some(ip.to_string())
}

/// 与参考实现同名的 `has_global_v6`：设备上当前有没有可用的全局 IPv6。
pub fn has_global_v6(dev: &str) -> bool {
    if dev.trim().is_empty() {
        return false;
    }
    let Ok(out) = std::process::Command::new("ip")
        .args(["-o", "-6", "addr", "show", "dev", dev.trim()])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| usable_v6_from_addr_line(l).is_some())
}

// ---------------------------------------------------------------- 接口健康 / 重拉

/// `ifstatus <name>` 里我们关心的三个事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfaceLive {
    /// netifd 认为接口已经 up。
    pub up: bool,
    /// 接口本次 up 了多久（秒；取不到就是 0）。
    pub uptime: u64,
    /// data 里已经出现可用的全局 IPv6（地址或委派前缀）。
    pub has_v6: bool,
}

/// 读一次 `ifstatus`（OpenWrt 的 netifd 状态，JSON）。
///
/// 不用 `ip addr` 查主网卡：`modem<id>_6` 的设备是 `@modem0`，地址可能落在
/// netifd 动态建出来的设备上；`ifstatus` 报的就是这个**接口**最终拿到的
/// 地址/前缀，和「v6 接口有没有真的起来」是同一口径。
pub fn iface_live(name: &str) -> Option<IfaceLive> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let out = std::process::Command::new("ifstatus")
        .arg(name)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(IfaceLive {
        up: v.get("up").and_then(|x| x.as_bool()).unwrap_or(false),
        uptime: v.get("uptime").and_then(|x| x.as_u64()).unwrap_or(0),
        has_v6: status_has_v6(&v),
    })
}

/// `ifstatus` 的 data 里有没有可用的全局 IPv6。
fn status_has_v6(v: &serde_json::Value) -> bool {
    let addr_ok = v
        .pointer("/data/ipv6-address")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter().any(|a| {
                a.get("address")
                    .and_then(|x| x.as_str())
                    .map(is_usable_global_v6)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if addr_ok {
        return true;
    }
    // 只拿到 PD 前缀（WAN 侧没有地址）也算「v6 链路已就绪」：LAN 已经能分到地址。
    v.pointer("/data/ipv6-prefix-assignment")
        .or_else(|| v.pointer("/data/ipv6-prefix"))
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter().any(|p| {
                p.get("address")
                    .and_then(|x| x.as_str())
                    .map(is_usable_global_v6)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 重启一个 v6 兄弟接口（`ifdown` + `ifup`）。
///
/// 只动 `modem<id>_6`，主接口与其 IPv4 不受影响。`ifdown` 失败不算错：
/// 接口本来就没 up 时，直接 `ifup` 就是想要的结果。
pub fn kick_sibling(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let _ = std::process::Command::new("ifdown").arg(name).output();
    match std::process::Command::new("ifup").arg(name).output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            log::warn!(
                "重拉 v6 接口 {name} 失败：{}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            log::warn!("跑不了 ifup {name}: {e}");
            false
        }
    }
}

/// 重拉间隔：首次 30 秒，之后 60 / 120 / 240 / 300 秒封顶。
pub fn kick_gap(attempts: u32) -> Duration {
    let secs = 30u64.saturating_mul(1u64 << attempts.min(4));
    Duration::from_secs(secs.min(300))
}

/// 接口起来多久之后才开始考虑「是不是没拿到 v6」。
///
/// 350 系列从 PDP 激活到 RA 到达实测要十几秒；太早重拉只会把刚发出的 RS 打断。
pub const KICK_MIN_UPTIME: u64 = 20;

/// 纯策略：现在该不该重拉 v6 接口。
///
/// - 只有 `dhcpv6` / `ra` 需要（`static` 的地址由 UCI 直接配，不走客户端）；
/// - 已经有全局 v6 ⇒ 永远不重拉；
/// - 接口还没 up ⇒ 立刻拉起（`ifup`，不是重启）；
/// - up 但不足 [`KICK_MIN_UPTIME`] ⇒ 再等等；
/// - 其余按 [`kick_gap`] 退避，避免每 15 秒一轮把接口反复拆掉。
pub fn should_kick(
    mode: V6Mode,
    up: bool,
    uptime: u64,
    has_v6: bool,
    attempts: u32,
    since_last: Option<Duration>,
) -> bool {
    if !matches!(mode, V6Mode::Dhcpv6 | V6Mode::Ra) || has_v6 {
        return false;
    }
    if !up {
        return true;
    }
    if uptime < KICK_MIN_UPTIME {
        return false;
    }
    match since_last {
        None => true,
        Some(d) => d >= kick_gap(attempts),
    }
}

#[derive(Debug, Default)]
struct KickState {
    last: Option<Instant>,
    attempts: u32,
}

static KICK_STATE: OnceLock<Mutex<HashMap<String, KickState>>> = OnceLock::new();

/// 每轮组网检查一次 v6 兄弟接口：没拿到地址就按退避重拉。
///
/// 这就是「模组后来才拿到 v6」的补偿路径：接口建得早、当时没有地址也没关系，
/// 只要它 up 着还没有全局 v6，就阶段性地 `ifdown/ifup` 让它重新发 RS / DHCPv6。
/// 拿到 v6 后计数清零，下次掉线重新从 30 秒开始。
pub fn maybe_rekick(name: &str, mode: V6Mode, live: IfaceLive) -> bool {
    let name = name.trim();
    if name.is_empty() || !matches!(mode, V6Mode::Dhcpv6 | V6Mode::Ra) {
        return false;
    }
    let map = KICK_STATE.get_or_init(Default::default);
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    let st = g.entry(name.to_string()).or_default();
    if live.has_v6 {
        st.last = None;
        st.attempts = 0;
        return false;
    }
    let now = Instant::now();
    let since = st.last.map(|t| now.saturating_duration_since(t));
    if !should_kick(mode, live.up, live.uptime, live.has_v6, st.attempts, since) {
        return false;
    }
    let attempt = st.attempts.saturating_add(1);
    st.last = Some(now);
    st.attempts = attempt;
    drop(g);
    log::info!(
        "v6 接口 {name} 已运行 {}s 仍无全局 IPv6（第 {} 次重拉，模式 {}）",
        live.uptime,
        attempt,
        mode.as_str()
    );
    kick_sibling(name)
}

// ---------------------------------------------------------------- 落地动作

/// 打开网卡的 RA 接收（`accept_ra=2` 是关键，见模块头坑 1）。
///
/// 直接写 `/proc/sys`，不依赖 sysctl 可执行文件；任何一项写失败都只记日志不报错
/// （容器 / 只读 procfs 上部署时不该因此中断组网）。
pub fn enable_ra(dev: &str) -> bool {
    let dev = dev.trim();
    if dev.is_empty() {
        return false;
    }
    let mut ok = true;
    for (key, value) in [
        ("accept_ra", "2"),
        ("accept_ra_defrtr", "1"),
        ("accept_ra_pinfo", "1"),
    ] {
        let path = format!("/proc/sys/net/ipv6/conf/{dev}/{key}");
        if let Err(e) = std::fs::write(&path, format!("{value}\n")) {
            log::debug!("写 {path} 失败: {e}");
            ok = false;
        }
    }
    ok
}

/// 主动索取一次 RA（原厂 `mtk_netagent` 的硬动作）。
///
/// 往 `router_solicitations` 写 1 ⇒ 内核立刻发 RS，把「等下一轮周期性 RA」
/// 变成「现在就要」。蜂窝链路单次 RS 丢包很常见，调用方按轮次重试。
pub fn trigger_rs(dev: &str) -> bool {
    let dev = dev.trim();
    if dev.is_empty() {
        return false;
    }
    let path = format!("/proc/sys/net/ipv6/conf/{dev}/router_solicitations");
    match std::fs::write(&path, "1\n") {
        Ok(_) => true,
        Err(e) => {
            log::debug!("触发 RS 失败（{path}）: {e}");
            false
        }
    }
}

/// 内核是否已按 RA 拿到 IPv6（默认路由或 `kernel_ra` 地址）。
pub fn ra_ready(dev: &str) -> bool {
    let dev = dev.trim();
    if dev.is_empty() {
        return false;
    }
    if let Ok(out) = std::process::Command::new("ip")
        .args(["-6", "route", "show", "default", "dev", dev])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        if text
            .lines()
            .any(|l| l.trim().starts_with("default") && l.contains("proto ra"))
        {
            return true;
        }
    }
    if let Ok(out) = std::process::Command::new("ip")
        .args(["-o", "-6", "addr", "show", "dev", dev])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        if text.lines().any(|l| l.contains("proto kernel_ra")) {
            return true;
        }
    }
    false
}

/// `static` 模式补一条默认路由（`ip -6 route replace default dev <dev> metric <m>`）。
///
/// 蜂窝侧不下发 RA 时，光有地址没有默认路由 ⇒ `ping6` 直连地址能通、出公网不通。
/// 只在设备上已经有全局地址时才补，避免装出一条黑洞路由。
pub fn ensure_static_route(dev: &str, metric: i64) -> bool {
    let dev = dev.trim();
    if dev.is_empty() || !has_global_v6(dev) {
        return false;
    }
    let metric = if metric > 0 { metric } else { 20 };
    match std::process::Command::new("ip")
        .args([
            "-6",
            "route",
            "replace",
            "default",
            "dev",
            dev,
            "metric",
            &metric.to_string(),
        ])
        .output()
    {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            log::warn!(
                "补 IPv6 默认路由失败（dev {dev}）：{}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            log::warn!("跑不了 ip -6 route（dev {dev}）: {e}");
            false
        }
    }
}

/// 每轮组网结束后的落地动作（`dhcpv6` 什么都不做 —— 交给 odhcp6c）。
pub fn after_reload(mode: V6Mode, dev: &str, metric: i64) {
    match mode {
        V6Mode::Ra => {
            enable_ra(dev);
            trigger_rs(dev);
        }
        V6Mode::Static => {
            ensure_static_route(dev, metric);
        }
        V6Mode::Dhcpv6 | V6Mode::Off => {}
    }
}

/// RA 模式下「这一轮还没拿到 IPv6」时的补索（带节流）。
///
/// 组网循环 15 秒一轮，每轮都发 RS 没必要（一次 RS 后至少等内核几个周期）；
/// 这里 60 秒最多一次，既能把「周期性 RA 要等几百秒」压下来，又不会刷爆链路。
pub fn maybe_resolicit(mode: V6Mode, dev: &str) {
    // 进程内「第一次组网」当作计时起点：netifd 刚拉起接口的十几秒里不抢，
    // 免得 RS 发在设备还没 UP 的时候（白费一次，还要等下一个 60 秒）。
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let uptime = START.get_or_init(Instant::now).elapsed();
    let has_v6 = ra_ready(dev) || has_global_v6(dev);
    if !should_resolicit(mode, dev, uptime, has_v6) {
        return;
    }
    static LAST: std::sync::Mutex<Option<(String, u64)>> = std::sync::Mutex::new(None);
    let now = unix_now() as u64;
    {
        let mut guard = LAST.lock().unwrap();
        if let Some((last_dev, at)) = guard.as_ref() {
            if last_dev == dev && now.saturating_sub(*at) < 60 {
                return;
            }
        }
        *guard = Some((dev.to_string(), now));
    }
    enable_ra(dev);
    trigger_rs(dev);
}

/// 服务器启动多久之后才允许补索（给 netifd 把接口拉起来的时间）。
pub const RESOLICIT_MIN_UPTIME: Duration = Duration::from_secs(30);

/// 判断「现在该不该补一次 RS」（纯逻辑，便于单测）。
pub fn should_resolicit(mode: V6Mode, dev: &str, uptime: Duration, has_v6: bool) -> bool {
    mode == V6Mode::Ra && !dev.trim().is_empty() && uptime >= RESOLICIT_MIN_UPTIME && !has_v6
}

#[cfg(test)]
mod tests;
