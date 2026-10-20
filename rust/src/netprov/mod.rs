//! 模组上线自动组网（`[net] auto_wan`）：把新插上的模组变成能上网的 WAN 接口。
//!
//! ## 为什么要它
//!
//! 不做这个，新插上的模组只是「有信号、有 IP」，但系统里没有对应接口 ——
//! 拿不到默认路由、进不了防火墙 wan 区，用户每次都要去 LuCI 手工建接口。
//!
//! ## 分层（与其它域一脉相承：纯策略 + 薄 IO + 编排）
//!
//! | 文件 | 职责 | 能不能单测 |
//! | --- | --- | --- |
//! | `mod.rs`（本文件） | **纯策略**：算「该有哪些接口、各自什么 proto/device/metric」，以及「UCI 该动哪些行」 | 全部可，无 IO |
//! | `uci.rs` | **薄 IO**：跑 `uci` 读出现状、把 [`UciOp`] 落盘 | 解析有单测；执行只在 unix 上有意义 |
//! | `route.rs` | **只读 + 清理**：读 `/proc/net/*` 看默认路由；断开时清旧的 v4 地址/路由 | 解析有单测；`flush_stale` 会**真改内核** |
//! | `v6.rs` | IPv6 取址方式的**判定 + 落地动作**（sysctl / RS / 默认路由） | 判定有单测；落地要真机 |
//! | `provision.rs` | **编排**：读现状 → 算计划 → diff → 应用 → 回报 | 有单测 |
//!
//! ## 三条红线（配置注释里写明的口径，逐条照搬）
//!
//! 1. **我们没建过的接口不动**：只有 proto 落在「本程序会写的几个值」里的同名
//!    接口才归我们管（见 [`is_managed_proto`]）；用户手工建的 `pppoe` / `static`
//!    接口一律原样留着。手工配置优先。
//! 2. **只在 Linux/OpenWrt 上真正动手**：Windows / 桌面 Linux 上整段空操作
//!    （`uci` 不存在，见 [`uci::available`]）。
//! 3. **认不出的 `wan_mode` 一律按 modem_pro**：配置写错不该让网络换形态
//!    （见 `config/wan_mode.rs` 的回落口径）。
//!
//! ## proto 怎么定（2026-10-13 起的口径）
//!
//! 依据 Go `module/netprov/netprov_proto.go:102-111`（`WanProtoForKindMode`）。
//!
//! 先按 `[net] wan_mode`（见 `config/wan_mode.rs`）分：
//!
//! | 模式 | 主接口 proto |
//! | --- | --- |
//! | `dhcp` | `dhcp`（地址/路由/DNS 由 netifd 的 DHCP 取） |
//! | `modem_pro`（默认）/ `both` | `modem_pro` |
//!
//! 在**走 modem_pro 的那两种模式**下，qmi / ecm / ncm / rndis / mbim / 认不出的类型
//! **一律 modem_pro** —— 不再只认 QMI，MBIM 也不再例外：它的会话建立由 modemd
//! 内置的 MBIM 拨号做，接口跟其它类型一样是一张 `modem<id>`。
//!
//! **唯一的例外**：`host_dial` 关掉时 MBIM 退回 `proto=mbim` —— 那时没人拨号，
//! 只能靠 netifd 的 `mbim.sh` / `umbim`。
//!
//! ⚠️ 与上游的一处偏差：`WanProtoForKindMode` 另有 `modemProtoOK` 参数
//! （路由器上装没装 `luci-proto-modem-pro`），本仓库没有这个开关 ——
//! 协议脚本 `files/lib/netifd/proto/modem_pro.sh` 随本包一起安装，视为恒真。

pub mod provision;
pub mod route;
pub mod uci;
pub mod v6;

use serde::Serialize;

use crate::config::WanMode;
use v6::V6Mode;

/// 本程序会写的**主接口** proto 值 —— 落在表里的同名接口才归我们管。
///
/// 手工建的接口 proto 千奇百怪（`pppoe` / `static` / `qmi` / `3g`…），
/// 一律不在表内 ⇒ 不动它（红线 1：手工配置优先）。
///
/// ⚠️ `static` / `none` **不在这张表里**：这两个值太常见，光看 proto 会把用户
/// 手起的接口一并接管，所以它们只在 [`is_ours`] 里、配合「接口名是不是
/// `modem<N>` / `modem<N>_6`」才被认领。
pub const MANAGED_PROTOS: [&str; 4] = ["modem_pro", "dhcp", "dhcpv6", "mbim"];

/// 主接口 proto 的取值（`host_dial` 关掉时 MBIM 的退路）。
pub const PROTO_MBIM: &str = "mbim";
/// 主接口 proto：只用一张接口，IPv4+IPv6 都跑在它上面。**默认**。
pub const PROTO_MODEM_PRO: &str = "modem_pro";
/// 主接口 proto：传统 DHCP（模式 `dhcp`）。
pub const PROTO_DHCP: &str = "dhcp";
/// 兄弟接口 proto：`modem<id>_6`。
pub const PROTO_DHCPV6: &str = "dhcpv6";

/// 主接口/兄弟接口 proto：`none` —— 只把设备拉起来，地址交给内核按 RA 处理
/// （RA 模式专用，见 [`v6`]）。
pub const PROTO_NONE: &str = "none";

/// 兄弟接口 proto：`static`（`[net] wan_ipv6_mode = static` 时写模组上报地址）。
pub const PROTO_STATIC: &str = "static";

/// IPv6 侧的组网输入（纯数据，无 IO）。
///
/// 单独成结构体而不是再加三个参数：`plan_wan` 已经在 8 个参数上了，
/// 再把「模式 / 静态地址 / 要不要委派前缀」摊开就没法读了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V6Plan<'a> {
    /// 取址方式（`dhcpv6` / `ra` / `static` / `off`）。
    pub mode: V6Mode,
    /// `static` 模式要写的地址（调用方已过滤成「可用的全局地址」；空 = 没有）。
    pub static_addr: &'a str,
    /// `dhcpv6` 模式下把拿到的 `/64` 继续委派给 LAN（`extendprefix=1`）。
    pub extend_prefix: bool,
}

impl Default for V6Plan<'_> {
    fn default() -> Self {
        Self {
            mode: V6Mode::Off,
            static_addr: "",
            extend_prefix: false,
        }
    }
}

impl<'a> V6Plan<'a> {
    /// 不托管 IPv6（`[net] wan_ipv6 = false`）。
    pub fn off() -> Self {
        Self::default()
    }

    /// 按配置搭一个（`static_addr` 只在 `static` 模式下有意义）。
    pub fn of(mode: V6Mode, static_addr: &'a str, extend_prefix: bool) -> Self {
        Self {
            mode,
            static_addr,
            extend_prefix,
        }
    }
}

/// 防火墙里放 WAN 接口的那个 zone（`[net] wan_zone`，默认 `wan`）。
pub const FIREWALL_ZONE: &str = "wan";

/// 主接口的 UCI section 名：`modem<id>`。
///
/// id 就是前端的 `modem_id`（从 0 起算），两处必须一致 —— 否则页面上点的
/// 「断开拨号」落到别的接口上。
pub fn iface_name(modem_id: u32) -> String {
    format!("modem{modem_id}")
}

/// 兄弟接口（IPv6）的 UCI section 名：`modem<id>_6`。
pub fn iface_name_v6(modem_id: u32) -> String {
    format!("modem{modem_id}_6")
}

/// 这个 proto 是不是本程序会写的（⇒ 同名接口归我们管）。
pub fn is_managed_proto(proto: &str) -> bool {
    let p = proto.trim();
    MANAGED_PROTOS.contains(&p)
}

/// 主接口该用什么 proto（纯策略，无 IO）。
///
/// 依据 Go `module/netprov/netprov_proto.go:102-111`（`WanProtoForKindMode`）。
///
/// 模式 `dhcp` ⇒ `dhcp`；`modem_pro` / `both` 下 QMI/ECM/NCM/RNDIS/未知一律
/// `modem_pro`，MBIM 同理 —— 除非 `host_dial` 关掉（那时退回 `mbim`，见模块头）。
///
/// ⚠️ 只是**算**出该写什么，不写 UCI；落盘在 [`uci::apply`]。
pub fn main_proto(kind: &str, mode: WanMode, host_dial: bool) -> &'static str {
    if !mode.uses_modem_pro() {
        return PROTO_DHCP;
    }
    if kind.eq_ignore_ascii_case("mbim") && !host_dial {
        return PROTO_MBIM;
    }
    PROTO_MODEM_PRO
}

/// 一个计划内的接口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedIface {
    /// UCI section 名（`network.modem1`）。
    pub name: String,
    /// proto。
    pub proto: &'static str,
    /// 网卡名（`wwan0` 等）；空串 = 不设 device，让 netifd 自己找。
    pub device: String,
    /// metric；`<= 0` 表示不设这一项。
    pub metric: i64,
    /// 是否随开机启动。
    pub auto: bool,
    /// static 模式要写的地址（空 = 不设这一项；`dhcp` / `modem_pro` 都留空）。
    pub ipaddr: String,
    /// static 模式的点分掩码（空 = 不设）。
    pub netmask: String,
    /// static 模式的网关（空 = 不设；netifd 会装 onlink 设备路由）。
    pub gateway: String,
    /// dhcpv6 兄弟接口要不要把拿到的 `/64` 前缀继续委派给 LAN（`extendprefix=1`）。
    ///
    /// 只有 `wan_ipv6_mode = dhcpv6` 且 `[net] lan_ipv6` 开着时才写：odhcp6c 拿到的
    /// 前缀要经 odhcpd 分给内网，否则内网设备只有 ULA/无 IPv6。
    pub extendprefix: bool,
    /// `static` 模式的 IPv6 地址（带 `/128`；空 = 不设这一项）。
    pub ip6addr: String,
    /// 强制下发的 DNS（空格分隔；空 = 不写 `dns` 项）。
    ///
    /// 只对主接口有意义 —— DNS 跟着默认路由走，v6 兄弟接口不配 v4 DNS。
    /// 值由 provision 从 `[net] wan_dns` 注入（默认 `223.5.5.5 119.29.29.29`）。
    pub dns: String,
}

/// 一个模组的组网计划。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WanPlan {
    pub modem_id: u32,
    /// 拨号类型（`qmi` / `mbim` / `ecm` / `ncm` / `rndis` / `unknown`）。
    pub kind: String,
    pub mode: WanMode,
    /// 该存在的接口（顺序即主接口在前）。
    pub ifaces: Vec<PlannedIface>,
    /// 该进防火墙 zone 的接口名。
    pub zone_networks: Vec<String>,
}

/// 算一个模组的组网计划（纯函数，无 IO）。
///
/// - `device` 空串 ⇒ 不设 `device` 项（模组刚插上、网卡还没枚举出来时就是这样，
///   等下一轮再补）；
/// - `v6.mode = off` ⇒ 不建 `modem<id>_6`（`[net] wan_ipv6` 关着、或模式是 off）；
/// - `metric<=0` ⇒ 不设 `metric` 项。
///
/// IPv6 兄弟接口的形态按 [`v6::V6Mode`] 分四路（详见 `netprov/v6.rs` 模块头）：
/// `dhcpv6`（`proto=dhcpv6` + `@主接口` + `extendprefix`）、`ra`（`proto=none`，
/// 地址交给内核，sysctl/RS 在 `after_reload` 里做）、`static`（`proto=static` +
/// `ip6addr=<addr>/128`）、`off`（不建）。
///
/// 350 系列（FM350/RW350）实测：RNDIS 网卡不发 DHCPv6 的 RA 时，`dhcpv6` 这条路
/// 拿不拿得到地址取决于运营商；**绝不能**把 `AT+CGPADDR` 里那个「只有接口标识」的
/// 残值写成 `/128`（见 `v6::is_usable_global_v6`）。
#[allow(clippy::too_many_arguments)] // 计划参数就是这些，拆结构体会让每个调用点都更长
pub fn plan_wan(
    modem_id: u32,
    kind: &str,
    mode: WanMode,
    device: &str,
    v6: V6Plan<'_>,
    metric: i64,
    host_dial: bool,
    static_ipv4: &str,
) -> WanPlan {
    let mut proto = main_proto(kind, mode, host_dial);
    let mut ipaddr = String::new();
    let mut netmask = String::new();
    let mut gateway = String::new();
    // 350 系列（FM350 / RW350）的 USB 网卡不发 DHCP —— 由调用方判定并给出
    // 模组上报的地址后，这里把主接口改成 **proto=static + 该地址**（与参考实现同口径：
    // RNDIS 点对点，模组地址可直接落在主机网卡上，不需要主机侧另配一个同网段地址）。
    //
    // 覆盖 dhcp 与 modem_pro 两种模式（与 provision.rs 的转换条件同口径）：
    // modem_pro 的 v4 也是 udhcpc，FM350 的模组内 DHCP 服务器长期无响应，
    // 不覆盖的话 modem_pro 模式在 350 系列上永远拿不到 v4。
    //
    // 依据 QModem `modem_dial.sh:1723-1750`（FM350 静态地址）；
    // `both` 模式不覆盖 —— 那时要的就是 modem_pro 自己取址。
    let want_static =
        !static_ipv4.trim().is_empty() && matches!(mode, WanMode::Dhcp | WanMode::ModemPro);
    if want_static {
        proto = PROTO_STATIC;
        ipaddr = static_ipv4.trim().to_string();
        netmask = "255.255.255.0".to_string();
        gateway = derive_gateway(&ipaddr).unwrap_or_default();
    }
    let name = iface_name(modem_id);
    let mut ifaces = vec![PlannedIface {
        name: name.clone(),
        proto,
        device: device.trim().to_string(),
        metric,
        auto: true,
        ipaddr,
        netmask,
        gateway,
        extendprefix: false,
        ip6addr: String::new(),
        dns: String::new(),
    }];
    if v6.mode.wants_sibling() && mode.wants_dhcpv6_sibling() {
        let mut v6_mode = v6.mode;
        let mut ip6addr = String::new();
        // `static` 模式没有可用地址时退回 dhcpv6：写个没有地址的 static 接口
        // 等于永远拿不到 v6（比不建还难排查）。调用方通常已在前面判过，
        // 这里再兜一层，纯策略函数不该产出「必然不通」的计划。
        if v6_mode == V6Mode::Static {
            match v6::usable_static_v6(v6.static_addr) {
                Some(addr) => ip6addr = format!("{addr}/128"),
                None => v6_mode = V6Mode::Dhcpv6,
            }
        }
        ifaces.push(PlannedIface {
            name: iface_name_v6(modem_id),
            proto: v6_mode.sibling_proto(),
            // ★ 绑**主接口**而不是裸网卡名：主接口是 modem_pro 时它的设备是
            //   netifd 动态建出来的（wwan0_1 之类），裸名会绑错。
            //   依据 QModem `modem_dial.sh:852-853`（`ifname` / `device` 都写
            //   `@<主接口>`）；本仓库只写 `device`（新版 netifd 的键名）。
            device: format!("@{name}"),
            metric,
            auto: true,
            ipaddr: String::new(),
            netmask: String::new(),
            gateway: String::new(),
            extendprefix: v6_mode == V6Mode::Dhcpv6 && v6.extend_prefix,
            ip6addr,
            dns: String::new(),
        });
    }
    let zone_networks = ifaces.iter().map(|i| i.name.clone()).collect();
    WanPlan {
        modem_id,
        kind: kind.trim().to_string(),
        mode,
        ifaces,
        zone_networks,
    }
}

/// 从模组上报的地址推一个网关（同网段 `.1`；自己就是 `.1` 时用 `.254`）。
///
/// 蜂窝网关普遍不回应 ICMP 但 ARP 正常，所以只用来写 UCI，不做可达性探测。
pub fn derive_gateway(ipv4: &str) -> Option<String> {
    let parts: Vec<&str> = ipv4.trim().split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let last: u32 = parts[3].parse().ok()?;
    let gw_last = if last == 1 { 254 } else { 1 };
    Some(format!(
        "{}.{}.{}.{}",
        parts[0], parts[1], parts[2], gw_last
    ))
}

/// UCI 的一条操作（顺序即执行顺序）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum UciOp {
    /// 建 section：`uci set network.modem1=interface`。
    Create { section: String },
    /// `uci set network.modem1.proto='modem_pro'`。
    Set {
        section: String,
        key: String,
        value: String,
    },
    /// `uci add_list firewall.wan.network='modem1'`。
    AddList {
        section: String,
        key: String,
        value: String,
    },
    /// `uci del_list firewall.wan.network='modem1'`。
    DelList {
        section: String,
        key: String,
        value: String,
    },
    /// `uci delete network.modem1_6`（模式切换后不再需要的兄弟接口）。
    Remove { section: String },
    /// `uci delete network.modem1.ipaddr`（从 static 切回 dhcp 时清残留）。
    Unset { section: String, key: String },
}

impl UciOp {
    /// 给人看的一行话（日志、`/api/dial` 的 `auto_wan.applied` 用它）。
    pub fn describe(&self) -> String {
        match self {
            UciOp::Create { section } => format!("建接口 {section}"),
            UciOp::Set {
                section,
                key,
                value,
            } => format!("{section}.{key}={value}"),
            UciOp::AddList {
                section,
                key,
                value,
            } => {
                format!("{section}.{key} += {value}")
            }
            UciOp::DelList {
                section,
                key,
                value,
            } => {
                format!("{section}.{key} -= {value}")
            }
            UciOp::Remove { section } => format!("删接口 {section}"),
            UciOp::Unset { section, key } => format!("{section}.{key} 清空"),
        }
    }
}

/// UCI 里已存在的接口现状（从 `uci show network` / `uci show firewall` 解析来）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExistingIface {
    /// section 名（`modem1` / `modem1_6` / `wan`…）。
    pub name: String,
    pub proto: String,
    pub device: String,
    pub metric: String,
    pub auto: String,
    /// static 模式的三个选项（用于「切回 dhcp 时清残留」）。
    pub ipaddr: String,
    pub netmask: String,
    pub gateway: String,
    /// v6 兄弟接口的两个选项（用于「换模式时对齐/清理」）。
    ///
    /// `extendprefix` 原样存字符串（`"1"` / 空），`ip6addr` 带 `/128`。
    pub extendprefix: String,
    pub ip6addr: String,
    /// 已下发的 DNS（空格分隔；用于 diff 对齐，避免每轮重复写）。
    pub dns: String,
    /// 已经在哪些防火墙 zone 里（zone 名，如 `wan`）。
    pub zones: Vec<String>,
}

/// 这个现状接口是不是「我们建的」⇒ 可以按计划改造（纯策略，无 IO、不改 UCI）。
///
/// 两档判据：
///
/// 1. proto 落在 [`MANAGED_PROTOS`] 里 ⇒ 直接算我们的；
/// 2. `static` / `none` 太常见，光看 proto 会把用户手起的接口一并接管，
///    所以这两个值**额外**要求接口名是本程序命名空间（`modem<N>` / `modem<N>_6`）
///    —— 那是红线 1（手工配置优先）。
///
/// `pppoe` / `qmi` / `3g` 之类一律落到 `_ => false`。
pub fn is_ours(existing: &ExistingIface) -> bool {
    if is_managed_proto(&existing.proto) {
        return true;
    }
    match existing.proto.as_str() {
        // 350 系列（FM350 / RW350）在 dhcp 模式下由我们写成 static；
        // 判据要带 `ipaddr`、且只认主接口（`modem<N>`，不含 `_6` 兄弟接口），
        // 否则用户手起的 static 接口会被接管。
        PROTO_STATIC => {
            // 主接口看 `ipaddr`、v6 兄弟接口看 `ip6addr` —— 两种都是本程序写的
            // （350 系列 IPv4 / `wan_ipv6_mode=static` 的 /128），必须能被下一轮
            // 继续对齐（否则切回 dhcpv6 时旧地址永远清不掉）。
            is_our_iface_name(&existing.name)
                && (!existing.ipaddr.is_empty() || !existing.ip6addr.is_empty())
        }
        // RA 模式的 v6 兄弟接口（只把设备拉起来，地址交给内核）。
        // 只有 `modem<N>_6` 这种名字才归我们。
        PROTO_NONE => is_our_iface_name(&existing.name) && existing.name.ends_with("_6"),
        _ => false,
    }
}

/// 这个名字是不是本程序建的接口（`modem<N>` / `modem<N>_6`）。
pub fn is_our_iface_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("modem") else {
        return false;
    };
    let rest = rest.strip_suffix("_6").unwrap_or(rest);
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// 计划 vs 现状 ⇒ 该做的 UCI 操作。
///
/// **只在真有差异时动手**：现状已经符合计划就一条都不出。这样每轮组网循环
/// （≤15 秒一轮）跑到已配好的模组上也是零写入，不会把 UCI 的 commit 时间戳
/// 刷成「刚改过」而触发无谓的 network reload。
///
/// 顺带处理「模式切换后不再需要的兄弟接口」：`modem<id>_6` 在计划里没有、
/// 现状里有、且 proto 归我们管 ⇒ 删掉（config.toml：按需补/删 `modem<N>_6`）。
pub fn diff_plan(
    existing: &[ExistingIface],
    plan: &WanPlan,
    zone: &str,
    zone_section: &str,
) -> Vec<UciOp> {
    let mut ops: Vec<UciOp> = Vec::new();
    let find = |name: &str| existing.iter().find(|e| e.name == name);

    for p in &plan.ifaces {
        match find(&p.name) {
            None => {
                ops.push(UciOp::Create {
                    section: p.name.clone(),
                });
                ops.push(UciOp::Set {
                    section: p.name.clone(),
                    key: "proto".into(),
                    value: p.proto.to_string(),
                });
                push_if_set(&mut ops, &p.name, "device", &p.device);
                if p.metric > 0 {
                    push_if_set(&mut ops, &p.name, "metric", &p.metric.to_string());
                }
                push_if_set(&mut ops, &p.name, "auto", if p.auto { "1" } else { "0" });
                push_if_set(&mut ops, &p.name, "ipaddr", &p.ipaddr);
                push_if_set(&mut ops, &p.name, "netmask", &p.netmask);
                push_if_set(&mut ops, &p.name, "gateway", &p.gateway);
                push_if_set(&mut ops, &p.name, "dns", &p.dns);
                if p.extendprefix {
                    push_if_set(&mut ops, &p.name, "extendprefix", "1");
                }
                push_if_set(&mut ops, &p.name, "ip6addr", &p.ip6addr);
            }
            Some(e) => {
                // 同名但 proto 不归我们管 ⇒ 用户手工建的，整条不动
                if !is_ours(e) {
                    continue;
                }
                if e.proto != p.proto {
                    ops.push(UciOp::Set {
                        section: p.name.clone(),
                        key: "proto".into(),
                        value: p.proto.to_string(),
                    });
                }
                if !p.device.is_empty() && e.device != p.device {
                    push_if_set(&mut ops, &p.name, "device", &p.device);
                }
                if p.metric > 0 && e.metric != p.metric.to_string() {
                    push_if_set(&mut ops, &p.name, "metric", &p.metric.to_string());
                }
                let want_auto = if p.auto { "1" } else { "0" };
                if e.auto != want_auto {
                    push_if_set(&mut ops, &p.name, "auto", want_auto);
                }
                // 静态项：计划里有就补齐；计划里没有（dhcp / modem_pro）而现状有 ⇒ 清掉，
                // 否则从 static 切回 dhcp 时旧地址会跟着接口重新 ifup 进内核。
                if !p.ipaddr.is_empty() {
                    if e.ipaddr != p.ipaddr {
                        push_if_set(&mut ops, &p.name, "ipaddr", &p.ipaddr);
                    }
                    if e.netmask != p.netmask {
                        push_if_set(&mut ops, &p.name, "netmask", &p.netmask);
                    }
                    if e.gateway != p.gateway {
                        push_if_set(&mut ops, &p.name, "gateway", &p.gateway);
                    }
                } else {
                    for (key, cur) in [
                        ("ipaddr", &e.ipaddr),
                        ("netmask", &e.netmask),
                        ("gateway", &e.gateway),
                    ] {
                        if !cur.is_empty() {
                            ops.push(UciOp::Unset {
                                section: p.name.clone(),
                                key: key.to_string(),
                            });
                        }
                    }
                }
                // v6 兄弟接口的两个选项：
                //   · `extendprefix` 只在 dhcpv6 模式下写；换到 ra/static/off 时清掉
                //     （留着会让 netifd 以为还要 PD）；
                //   · `ip6addr` 只在 static 模式下写；换到别的模式必须清掉，否则
                //     旧 /128 会跟着接口 ifup 进内核，和 odhcp6c/RA 抢地址。
                if p.extendprefix {
                    if e.extendprefix != "1" {
                        push_if_set(&mut ops, &p.name, "extendprefix", "1");
                    }
                } else if !e.extendprefix.is_empty() {
                    ops.push(UciOp::Unset {
                        section: p.name.clone(),
                        key: "extendprefix".into(),
                    });
                }
                if !p.ip6addr.is_empty() {
                    if e.ip6addr != p.ip6addr {
                        push_if_set(&mut ops, &p.name, "ip6addr", &p.ip6addr);
                    }
                } else if !e.ip6addr.is_empty() {
                    ops.push(UciOp::Unset {
                        section: p.name.clone(),
                        key: "ip6addr".into(),
                    });
                }
                // DNS 强制下发（见 PlannedIface.dns）：计划有就补齐/更新；
                // 计划没有而现状有 ⇒ 清掉（切到 carrier 模式不残留旧 DNS）。
                if !p.dns.is_empty() {
                    if e.dns != p.dns {
                        push_if_set(&mut ops, &p.name, "dns", &p.dns);
                    }
                } else if !e.dns.is_empty() {
                    ops.push(UciOp::Unset {
                        section: p.name.clone(),
                        key: "dns".into(),
                    });
                }
            }
        }
        // 防火墙 zone：计划里的接口都得在（已在了就不重复 add_list）
        if let Some(e) = find(&p.name) {
            if is_ours(e) && !e.zones.iter().any(|z| z == zone) {
                ops.push(UciOp::AddList {
                    section: zone_section.to_string(),
                    key: "network".into(),
                    value: p.name.clone(),
                });
            }
        } else {
            // 刚建的接口也照样进 zone
            ops.push(UciOp::AddList {
                section: zone_section.to_string(),
                key: "network".into(),
                value: p.name.clone(),
            });
        }
    }

    // 模式切换：计划里没有、现状里有的兄弟接口，归我们管就删
    let planned: Vec<&str> = plan.ifaces.iter().map(|i| i.name.as_str()).collect();
    for e in existing {
        let is_sibling = e.name == iface_name_v6(plan.modem_id);
        if is_sibling && !planned.contains(&e.name.as_str()) && is_ours(e) {
            ops.push(UciOp::Remove {
                section: e.name.clone(),
            });
            if e.zones.iter().any(|z| z == zone) {
                ops.push(UciOp::DelList {
                    section: zone_section.to_string(),
                    key: "network".into(),
                    value: e.name.clone(),
                });
            }
        }
    }
    ops
}

/// 值非空才下发（空串 = 「不设这一项」，见 [`plan_wan`]）。
fn push_if_set(ops: &mut Vec<UciOp>, section: &str, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    ops.push(UciOp::Set {
        section: section.to_string(),
        key: key.to_string(),
        value: value.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 断言样板 ----------
    //
    // `ops.contains(&UciOp::Set{...})` 每个断言要写 5 行、`section/key/value`
    // 三处 `.into()`；同一个契约在十几个用例里反复出现。抽成 helper 后
    // 断言只剩一行，改动 key 名时也只有一个地方要同步。

    /// 断言 ops 里有一条 `section.key = value`。
    fn assert_set(ops: &[UciOp], section: &str, key: &str, value: &str) {
        assert!(
            ops.iter().any(|o| matches!(
                o,
                UciOp::Set { section: s, key: k, value: v } if s == section && k == key && v == value
            )),
            "期望 {section}.{key}={value}，实际 ops：{:?}",
            ops.iter().map(|o| o.describe()).collect::<Vec<_>>()
        );
    }

    /// 断言 ops 里**没有**动过这个 key（空值不下发 / 网卡未知不写 device 一类口径）。
    fn assert_no_set(ops: &[UciOp], key: &str) {
        assert!(
            !ops.iter()
                .any(|o| matches!(o, UciOp::Set { key: k, .. } if k == key)),
            "期望不写 {key}，实际 ops：{:?}",
            ops.iter().map(|o| o.describe()).collect::<Vec<_>>()
        );
    }

    // ---------- 命名 ----------

    #[test]
    fn iface_names_follow_modem_id() {
        assert_eq!(iface_name(0), "modem0");
        assert_eq!(iface_name(3), "modem3");
        assert_eq!(iface_name_v6(3), "modem3_6");
    }

    // ---------- proto 选择 ----------

    #[test]
    fn main_proto_modem_pro_by_default_for_all_kinds() {
        // 三种模式下 qmi / ecm / ncm / rndis / mbim / 未知 一律 modem_pro
        for kind in ["qmi", "ecm", "ncm", "rndis", "mbim", "unknown", ""] {
            assert_eq!(
                main_proto(kind, WanMode::ModemPro, true),
                PROTO_MODEM_PRO,
                "kind={kind}"
            );
        }
    }

    #[test]
    fn main_proto_dhcp_mode_uses_dhcp() {
        assert_eq!(main_proto("qmi", WanMode::Dhcp, true), PROTO_DHCP);
        assert_eq!(main_proto("mbim", WanMode::Dhcp, true), PROTO_DHCP);
        assert_eq!(main_proto("qmi", WanMode::Dhcp, false), PROTO_DHCP);
    }

    #[test]
    fn mbim_falls_back_only_when_host_dial_off() {
        assert_eq!(main_proto("mbim", WanMode::ModemPro, false), PROTO_MBIM);
        assert_eq!(main_proto("mbim", WanMode::Both, false), PROTO_MBIM);
        // host_dial 开着就不能退：会话由 modemd 内置的 MBIM 拨号建
        assert_eq!(main_proto("mbim", WanMode::ModemPro, true), PROTO_MODEM_PRO);
        assert_eq!(main_proto("mbim", WanMode::Both, true), PROTO_MODEM_PRO);
    }

    // ---------- 计划 ----------

    #[test]
    fn plan_modem_pro_is_single_iface() {
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        assert_eq!(p.ifaces.len(), 1);
        assert_eq!(p.ifaces[0].name, "modem1");
        assert_eq!(p.ifaces[0].proto, PROTO_MODEM_PRO);
        assert_eq!(p.ifaces[0].device, "wwan0");
        assert_eq!(p.ifaces[0].metric, 20);
        assert_eq!(p.zone_networks, vec!["modem1".to_string()]);
    }

    #[test]
    fn plan_mbim_without_host_dial_uses_mbim_proto() {
        // host_dial 关掉时 MBIM 退回 proto=mbim（netifd 的 mbim.sh / umbim 接管）
        let p = plan_wan(
            0,
            "mbim",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            false,
            "",
        );
        assert_eq!(p.ifaces.len(), 1, "mbim 模式也一样是单接口");
        assert_eq!(p.ifaces[0].proto, PROTO_MBIM);
        assert_eq!(p.ifaces[0].name, "modem0");
    }

    #[test]
    fn plan_mbim_with_host_dial_uses_modem_pro() {
        // 会话由 modemd 内置的 MBIM 拨号建 ⇒ 接口跟其它类型一样
        let p = plan_wan(
            0,
            "mbim",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        assert_eq!(p.ifaces[0].proto, PROTO_MODEM_PRO);
    }

    #[test]
    fn plan_dhcp_mode_has_v6_sibling() {
        let p = plan_wan(
            2,
            "qmi",
            WanMode::Dhcp,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        assert_eq!(p.ifaces.len(), 2);
        assert_eq!(p.ifaces[1].name, "modem2_6");
        assert_eq!(p.ifaces[1].proto, PROTO_DHCPV6);
        assert_eq!(
            p.zone_networks,
            vec!["modem2".to_string(), "modem2_6".to_string()]
        );
    }

    // ---------- IPv6 取址方式（对齐 luci-app-fm350） ----------

    #[test]
    fn plan_dhcpv6_sibling_shape_matches_reference() {
        let p = plan_wan(
            2,
            "qmi",
            WanMode::Dhcp,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let s = &p.ifaces[1];
        // 绑**主接口**：主接口是 modem_pro 时设备是 netifd 动态建的，裸名会绑错
        assert_eq!(s.device, "@modem2");
        assert!(s.extendprefix, "dhcpv6 要把 /64 继续委派给 LAN");
        assert!(s.ip6addr.is_empty(), "dhcpv6 一律不写静态地址");
        // lan_ipv6 关掉 ⇒ 不委派
        let p = plan_wan(
            2,
            "qmi",
            WanMode::Dhcp,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", false),
            20,
            true,
            "",
        );
        assert!(!p.ifaces[1].extendprefix);
    }

    #[test]
    fn plan_ra_mode_uses_none_proto() {
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Both,
            "eth2",
            V6Plan::of(V6Mode::Ra, "", true),
            20,
            true,
            "",
        );
        let s = &p.ifaces[1];
        assert_eq!(s.proto, PROTO_NONE, "RA 模式只把设备拉起来，地址交给内核");
        assert!(!s.extendprefix, "RA 模式不写 dhcpv6 的选项");
        assert!(s.ip6addr.is_empty(), "RA 模式绝不能写 /128");
    }

    #[test]
    fn plan_static_mode_writes_slash_128_and_rejects_placeholder() {
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::of(V6Mode::Static, "2001:db8:1:2::8", true),
            20,
            true,
            "",
        );
        assert_eq!(p.ifaces[1].proto, PROTO_STATIC);
        assert_eq!(p.ifaces[1].ip6addr, "2001:db8:1:2::8/128");
        assert!(!p.ifaces[1].extendprefix, "static 不写 extendprefix");

        // 实机那种只有接口标识的残值：不能写成 /128，退回 dhcpv6
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::of(V6Mode::Static, "::18df:9a01:f982:815a", false),
            20,
            true,
            "",
        );
        assert_eq!(p.ifaces[1].proto, PROTO_DHCPV6);
        assert!(p.ifaces[1].ip6addr.is_empty());
    }

    #[test]
    fn diff_upgrades_old_sibling_shape() {
        // 旧版本建的：device 是裸网卡名、没有 extendprefix ⇒ 本轮要对齐
        let cur = vec![
            ExistingIface {
                name: "modem0".into(),
                proto: PROTO_STATIC.into(),
                device: "eth2".into(),
                ipaddr: "10.42.0.16".into(),
                ..Default::default()
            },
            ExistingIface {
                name: "modem0_6".into(),
                proto: PROTO_DHCPV6.into(),
                device: "eth2".into(),
                ..Default::default()
            },
        ];
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "10.42.0.16",
        );
        let ops = diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan");
        assert_set(&ops, "modem0_6", "device", "@modem0");
        assert_set(&ops, "modem0_6", "extendprefix", "1");
    }

    #[test]
    fn diff_clears_static_v6_when_leaving_static_mode() {
        let cur = vec![
            ExistingIface {
                name: "modem0".into(),
                proto: PROTO_STATIC.into(),
                device: "eth2".into(),
                ipaddr: "10.42.0.16".into(),
                ..Default::default()
            },
            ExistingIface {
                name: "modem0_6".into(),
                proto: PROTO_STATIC.into(),
                device: "@modem0".into(),
                ip6addr: "2001:db8:1:2::8/128".into(),
                ..Default::default()
            },
        ];
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "10.42.0.16",
        );
        let ops = diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan");
        assert!(ops.contains(&UciOp::Set {
            section: "modem0_6".into(),
            key: "proto".into(),
            value: PROTO_DHCPV6.into()
        }));
        assert!(
            ops.contains(&UciOp::Unset {
                section: "modem0_6".into(),
                key: "ip6addr".into()
            }),
            "切回 dhcpv6 必须清掉旧 /128，否则会跟 odhcp6c 抢地址"
        );
    }

    #[test]
    fn plan_both_mode_has_sibling_too() {
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Both,
            "usb0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        assert_eq!(p.ifaces.len(), 2);
        assert_eq!(p.ifaces[0].proto, PROTO_MODEM_PRO);
        assert_eq!(p.ifaces[1].proto, PROTO_DHCPV6);
    }

    #[test]
    fn plan_without_ipv6_has_no_sibling() {
        let p = plan_wan(
            1,
            "qmi",
            WanMode::Dhcp,
            "wwan0",
            V6Plan::off(),
            20,
            true,
            "",
        );
        assert_eq!(p.ifaces.len(), 1, "wan_ipv6 关着就不建 modem<id>_6");
    }

    #[test]
    fn plan_without_device_omits_device() {
        // 模组刚插上、网卡还没枚举出来：device 空 ⇒ 不设这一项
        let p = plan_wan(
            0,
            "qmi",
            WanMode::ModemPro,
            "",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        assert!(p.ifaces[0].device.is_empty());
    }

    // ---------- diff ----------

    #[test]
    fn diff_creates_missing_iface() {
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let ops = diff_plan(&[], &p, FIREWALL_ZONE, "firewall.wan");
        assert_eq!(
            ops,
            vec![
                UciOp::Create {
                    section: "modem1".into()
                },
                UciOp::Set {
                    section: "modem1".into(),
                    key: "proto".into(),
                    value: "modem_pro".into()
                },
                UciOp::Set {
                    section: "modem1".into(),
                    key: "device".into(),
                    value: "wwan0".into()
                },
                UciOp::Set {
                    section: "modem1".into(),
                    key: "metric".into(),
                    value: "20".into()
                },
                UciOp::Set {
                    section: "modem1".into(),
                    key: "auto".into(),
                    value: "1".into()
                },
                UciOp::AddList {
                    section: "firewall.wan".into(),
                    key: "network".into(),
                    value: "modem1".into()
                },
            ]
        );
    }

    #[test]
    fn diff_is_empty_when_already_configured() {
        // 已配好就一条都不出 —— 每轮组网循环零写入
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let cur = vec![ExistingIface {
            name: "modem1".into(),
            proto: "modem_pro".into(),
            device: "wwan0".into(),
            metric: "20".into(),
            auto: "1".into(),
            zones: vec!["wan".into()],
            ..Default::default()
        }];
        assert!(diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan").is_empty());
    }

    #[test]
    fn diff_only_touches_what_differs() {
        // device 变了（网卡重新枚举）但 proto/metric/zone 都对 ⇒ 只改 device
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan1",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let cur = vec![ExistingIface {
            name: "modem1".into(),
            proto: "modem_pro".into(),
            device: "wwan0".into(),
            metric: "20".into(),
            auto: "1".into(),
            zones: vec!["wan".into()],
            ..Default::default()
        }];
        let ops = diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan");
        assert_eq!(
            ops,
            vec![UciOp::Set {
                section: "modem1".into(),
                key: "device".into(),
                value: "wwan1".into()
            }]
        );
    }

    #[test]
    fn diff_changes_proto_on_mode_switch() {
        // 模式从 dhcp 切到 modem_pro：就地改主接口的 proto（配置注释的口径）
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let cur = vec![ExistingIface {
            name: "modem1".into(),
            proto: "dhcp".into(),
            device: "wwan0".into(),
            metric: "20".into(),
            auto: "1".into(),
            zones: vec!["wan".into()],
            ..Default::default()
        }];
        let ops = diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan");
        assert_eq!(
            ops,
            vec![UciOp::Set {
                section: "modem1".into(),
                key: "proto".into(),
                value: "modem_pro".into()
            }]
        );
    }

    #[test]
    fn diff_leaves_hand_made_iface_alone() {
        // 红线 1：同名但 proto 不归我们管 ⇒ 整条不动，也不拉进 zone
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let cur = vec![ExistingIface {
            name: "modem1".into(),
            proto: "pppoe".into(),
            device: "eth0".into(),
            metric: "10".into(),
            auto: "1".into(),
            zones: vec![],
            ..Default::default()
        }];
        assert!(diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan").is_empty());
    }

    #[test]
    fn fm350_dhcp_mode_becomes_static() {
        // 350 系列的 USB 网卡不发 DHCP ⇒ dhcp 模式下用模组上报地址直接配 static
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            false,
            "10.42.0.16",
        );
        assert_eq!(p.ifaces[0].proto, "static");
        assert_eq!(p.ifaces[0].ipaddr, "10.42.0.16");
        assert_eq!(p.ifaces[0].netmask, "255.255.255.0");
        assert_eq!(p.ifaces[0].gateway, "10.42.0.1");
        assert_eq!(p.ifaces[0].device, "eth2");
        // v6 兄弟接口不变（仍是 dhcpv6）
        assert_eq!(p.ifaces[1].proto, "dhcpv6");
    }

    #[test]
    fn static_only_when_address_given() {
        // 350 系列怪癖（模组内 DHCP 无响应）覆盖 dhcp 与 modem_pro 两种模式：
        // 给了模组上报地址就转 static；modem_pro 的 v4 也是 udhcpc（mp-client），
        // 不转换的话 350 系列在 modem_pro 模式下永远拿不到 v4（实机踩过）。
        let p = plan_wan(
            0,
            "ncm",
            WanMode::ModemPro,
            "eth2",
            V6Plan::off(),
            20,
            false,
            "10.0.0.5",
        );
        assert_eq!(p.ifaces[0].proto, "static");
        assert_eq!(p.ifaces[0].ipaddr, "10.0.0.5");
        // 没给地址 ⇒ 维持 dhcp
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::off(),
            20,
            false,
            "",
        );
        assert_eq!(p.ifaces[0].proto, "dhcp");
    }

    #[test]
    fn gateway_derivation() {
        assert_eq!(derive_gateway("10.42.0.16").as_deref(), Some("10.42.0.1"));
        assert_eq!(derive_gateway("10.42.0.1").as_deref(), Some("10.42.0.254"));
        assert_eq!(derive_gateway("bad"), None);
    }

    #[test]
    fn diff_writes_static_and_clears_on_switch_back() {
        let p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::off(),
            20,
            false,
            "10.42.0.16",
        );
        let ops = diff_plan(&[], &p, FIREWALL_ZONE, "firewall.wan");
        assert_set(&ops, "modem0", "ipaddr", "10.42.0.16");
        assert_set(&ops, "modem0", "gateway", "10.42.0.1");

        // 切回 dhcp：旧静态项要清掉，否则会跟着接口重新 ifup 进内核
        let back = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::off(),
            20,
            false,
            "",
        );
        let cur = vec![ExistingIface {
            name: "modem0".into(),
            proto: "static".into(),
            device: "eth2".into(),
            metric: "20".into(),
            auto: "1".into(),
            ipaddr: "10.42.0.16".into(),
            netmask: "255.255.255.0".into(),
            gateway: "10.42.0.1".into(),
            zones: vec![FIREWALL_ZONE.into()],
            ..Default::default()
        }];
        let ops = diff_plan(&cur, &back, FIREWALL_ZONE, "firewall.wan");
        for key in ["ipaddr", "netmask", "gateway"] {
            assert!(
                ops.contains(&UciOp::Unset {
                    section: "modem0".into(),
                    key: key.into()
                }),
                "切回 dhcp 应清掉 {key}"
            );
        }
    }

    #[test]
    fn static_iface_is_ours_only_in_our_namespace() {
        assert!(is_our_iface_name("modem0"));
        assert!(is_our_iface_name("modem12"));
        assert!(is_our_iface_name("modem0_6"));
        assert!(!is_our_iface_name("modem"));
        assert!(!is_our_iface_name("wan"));
        assert!(!is_our_iface_name("modem_x"));
        let mut e = ExistingIface {
            name: "modem0".into(),
            proto: "static".into(),
            ipaddr: "10.42.0.16".into(),
            ..Default::default()
        };
        assert!(is_ours(&e));
        e.name = "mybridge".into();
        assert!(!is_ours(&e), "用户手起的 static 接口不能被接管");
    }

    #[test]
    fn diff_adds_missing_zone_only() {
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let cur = vec![ExistingIface {
            name: "modem1".into(),
            proto: "modem_pro".into(),
            device: "wwan0".into(),
            metric: "20".into(),
            auto: "1".into(),
            zones: vec![], // 没进 zone
            ..Default::default()
        }];
        let ops = diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan");
        assert_eq!(
            ops,
            vec![UciOp::AddList {
                section: "firewall.wan".into(),
                key: "network".into(),
                value: "modem1".into()
            }]
        );
    }

    #[test]
    fn diff_removes_stale_v6_sibling_on_mode_switch() {
        // 从 dhcp 切到 modem_pro：modem1_6 不再需要 ⇒ 删掉并移出 zone
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let cur = vec![
            ExistingIface {
                name: "modem1".into(),
                proto: "dhcp".into(),
                device: "wwan0".into(),
                metric: "20".into(),
                auto: "1".into(),
                zones: vec!["wan".into()],
                ..Default::default()
            },
            ExistingIface {
                name: "modem1_6".into(),
                proto: "dhcpv6".into(),
                device: "wwan0".into(),
                metric: "20".into(),
                auto: "1".into(),
                zones: vec!["wan".into()],
                ..Default::default()
            },
        ];
        let ops = diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan");
        assert_set(&ops, "modem1", "proto", "modem_pro");
        assert!(ops.contains(&UciOp::Remove {
            section: "modem1_6".into()
        }));
        assert!(ops.contains(&UciOp::DelList {
            section: "firewall.wan".into(),
            key: "network".into(),
            value: "modem1_6".into()
        }));
    }

    #[test]
    fn diff_keeps_hand_made_v6_sibling() {
        // 兄弟接口是用户手工建的（proto 不归我们管）⇒ 不删
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let cur = vec![ExistingIface {
            name: "modem1_6".into(),
            proto: "static".into(),
            device: "wwan0".into(),
            metric: "20".into(),
            auto: "1".into(),
            zones: vec!["wan".into()],
            ..Default::default()
        }];
        assert!(!diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan")
            .iter()
            .any(|o| matches!(o, UciOp::Remove { .. })));
    }

    #[test]
    fn diff_creates_sibling_and_zones_both() {
        let p = plan_wan(
            0,
            "qmi",
            WanMode::Dhcp,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let ops = diff_plan(&[], &p, FIREWALL_ZONE, "firewall.wan");
        // 两个接口都要进 zone
        let adds: Vec<&str> = ops
            .iter()
            .filter_map(|o| match o {
                UciOp::AddList { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(adds, vec!["modem0", "modem0_6"]);
    }

    #[test]
    fn diff_honours_custom_zone_name() {
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let ops = diff_plan(&[], &p, "wan_modem", "firewall.wan_modem");
        assert!(ops.contains(&UciOp::AddList {
            section: "firewall.wan_modem".into(),
            key: "network".into(),
            value: "modem1".into()
        }));
    }

    #[test]
    fn diff_skips_metric_when_not_configured() {
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            0,
            true,
            "",
        );
        let ops = diff_plan(&[], &p, FIREWALL_ZONE, "firewall.wan");
        assert_no_set(&ops, "metric");
    }

    #[test]
    fn diff_skips_device_when_unknown() {
        // 网卡还没枚举出来：只建接口与 proto，不写 device（写了反而绑死）
        let p = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "",
            V6Plan::of(V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        let ops = diff_plan(&[], &p, FIREWALL_ZONE, "firewall.wan");
        assert_no_set(&ops, "device");
        assert_set(&ops, "modem1", "proto", "modem_pro");
    }

    #[test]
    fn op_describe_is_human_readable() {
        assert_eq!(
            UciOp::Set {
                section: "modem1".into(),
                key: "proto".into(),
                value: "modem_pro".into()
            }
            .describe(),
            "modem1.proto=modem_pro"
        );
        assert_eq!(
            UciOp::AddList {
                section: "firewall.wan".into(),
                key: "network".into(),
                value: "modem1".into()
            }
            .describe(),
            "firewall.wan.network += modem1"
        );
        assert_eq!(
            UciOp::Remove {
                section: "modem1_6".into()
            }
            .describe(),
            "删接口 modem1_6"
        );
    }

    #[test]
    fn managed_protos_table() {
        for p in ["modem_pro", "dhcp", "dhcpv6", "mbim"] {
            assert!(is_managed_proto(p), "{p} 应归我们管");
        }
        for p in ["pppoe", "static", "qmi", "3g", "", "MODEM_PRO "] {
            assert!(!is_managed_proto(p), "{p} 不该归我们管");
        }
    }

    // ---------- DNS 强制下发（`[net] wan_dns`） ----------

    #[test]
    fn diff_forces_and_updates_wan_dns() {
        // 模拟 provision 注入后的计划：主接口带强制 DNS
        let mut p = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::off(),
            20,
            false,
            "10.42.0.16",
        );
        p.ifaces[0].dns = "223.5.5.5 119.29.29.29".to_string();

        // 1) 新建接口：应写 dns
        let ops = diff_plan(&[], &p, FIREWALL_ZONE, "firewall.wan");
        assert_set(&ops, "modem0", "dns", "223.5.5.5 119.29.29.29");

        // 2) 已配好同值：零写入（每轮组网循环不重复动手）
        let cur = vec![ExistingIface {
            name: "modem0".into(),
            proto: PROTO_STATIC.into(),
            device: "eth2".into(),
            metric: "20".into(),
            auto: "1".into(),
            ipaddr: "10.42.0.16".into(),
            netmask: "255.255.255.0".into(),
            gateway: "10.42.0.1".into(),
            dns: "223.5.5.5 119.29.29.29".into(),
            zones: vec![FIREWALL_ZONE.into()],
            ..Default::default()
        }];
        assert!(diff_plan(&cur, &p, FIREWALL_ZONE, "firewall.wan").is_empty());

        // 3) 值变了：更新
        let mut cur2 = cur.clone();
        cur2[0].dns = "8.8.8.8".into();
        let ops = diff_plan(&cur2, &p, FIREWALL_ZONE, "firewall.wan");
        assert_set(&ops, "modem0", "dns", "223.5.5.5 119.29.29.29");

        // 4) 计划不强制（carrier 模式）：清掉现状里的 dns
        let mut p_off = plan_wan(
            0,
            "ncm",
            WanMode::Dhcp,
            "eth2",
            V6Plan::off(),
            20,
            false,
            "10.42.0.16",
        );
        p_off.ifaces[0].dns = String::new();
        let ops = diff_plan(&cur, &p_off, FIREWALL_ZONE, "firewall.wan");
        assert!(ops.contains(&UciOp::Unset {
            section: "modem0".into(),
            key: "dns".into()
        }));
    }
}
