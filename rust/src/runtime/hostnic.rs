//! 主机侧网卡配置（`/api/host/nic*`）。
//!
//! 背景：个别机型（**FM350 系列**）的 USB 网卡不给主机发 DHCP —— 模组侧拨号明明成功，
//! 主机那张网卡却长期挂在 `169.254.x.x`（链路本地）上、没有默认路由，于是"上不了网"。
//! 这种机器要在**主机侧**把网卡配成与模组同网段、网关指向模组（FM350 实测如此）。
//!
//! 本文件只做三件事，都不占串口：
//!   · 纯逻辑：机型特例判定、掩码/前缀互转、配置校验与归一化、建议值、
//!     三组命令（renew / static / dhcp）+ OpenWrt 的 UCI 持久化命令；
//!   · 本机枚举：`interfaces()`（去掉环回与明显虚拟网卡，模组口排前面）；
//!   · 台账：`HostNicStore`（`hostnic.json`，与配置文件同目录，重启不丢）。
//!
//! ⚠️ 命令**默认只生成不执行**：改网卡要 root / 管理员，一键改 `/etc/config/network`
//! 风险太大（接口名、防火墙分区得人判断）。`execute=true` 时才由本程序下发，
//! 且走 `prog + args`（**不拼 shell**、不经过 sh），失败把输出原样回给页面。

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{msg, Result};
use crate::logging::timestamp;

// ------------------------------------------------------------------ 机型特例 ---

/// 该机型是否需要"手工配主机网卡"；第二个返回值是给页面看的解释（不需要时为空）。
///
/// ⚠️ 判据用 `contains` 而不是前缀匹配：调用方可能传的是 **ATI 原文**
/// （`ATI|Manufacturer: Fibocom|Model: FM350-GL|…`）。别的机型别往这里加 ——
/// ECM/NCM/MBIM/QMI 正常机型的主机侧地址由模组 DHCP 或主机拨号器负责。
pub fn quirk(model: &str) -> (bool, &'static str) {
    const WHY: &str = "广和通 350 系列（FM350 / RW350）的 USB 网卡 DHCP 很慢/不稳：实测要么几十秒到一两分钟才回，\
        要么干脆不回 —— 主机侧就长期挂在 169.254.x.x 上、没有默认路由，模组侧拨号成功也上不了网。\
        两条路：等它 DHCP（「先试 DHCP」），或把网卡 IP 手工配成与模组对应（同网段 + 网关指向模组）。";
    if crate::modem::model::is_fibocom_350(model) {
        (true, WHY)
    } else {
        (false, "")
    }
}

// ------------------------------------------------------------------ 掩码 / 前缀 ---

/// 掩码 → 前缀长度。两种写法都接受：`"255.255.255.0"` 或 `"24"`。
pub fn prefix_from_mask(mask: &str) -> Option<i32> {
    let m = mask.trim();
    if m.is_empty() {
        return None;
    }
    if let Ok(n) = m.parse::<i32>() {
        return (0..=32).contains(&n).then_some(n);
    }
    let octets = parse_v4(m)?;
    let bits = mask_bits(octets);
    // 非连续掩码（如 255.0.255.0）不算合法：回写点分掩码比对一次
    if mask_from_prefix(bits)? != format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3])
    {
        return None;
    }
    Some(bits)
}

/// 前缀长度 → 点分掩码（0~32 之外返回 None）。
pub fn mask_from_prefix(n: i32) -> Option<String> {
    if !(0..=32).contains(&n) {
        return None;
    }
    let m: u32 = if n == 0 { 0 } else { u32::MAX << (32 - n) };
    Some(format!(
        "{}.{}.{}.{}",
        (m >> 24) & 0xff,
        (m >> 16) & 0xff,
        (m >> 8) & 0xff,
        m & 0xff
    ))
}

/// 连续的 1 的个数（点分掩码 → 前缀）。
fn mask_bits(o: [u8; 4]) -> i32 {
    let v = u32::from_be_bytes(o);
    if v == 0 {
        return 0;
    }
    // 从左起数第一个 0 之前的 1
    let mut n = 0;
    for i in (0..32).rev() {
        if (v >> i) & 1 == 1 {
            n += 1;
        } else {
            break;
        }
    }
    n
}

/// 点分 IPv4 → 四个字节（不合法返回 None）。
fn parse_v4(s: &str) -> Option<[u8; 4]> {
    let s = s.trim();
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) || p.len() > 3 {
            return None;
        }
        out[i] = p.parse::<u8>().ok()?;
    }
    Some(out)
}

/// 是否合法的 IPv4 地址。
pub fn is_v4(s: &str) -> bool {
    parse_v4(s).is_some()
}

// ------------------------------------------------------------------ 配置 ---

/// 一份主机网卡配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HostNicPlan {
    /// 网卡名（Linux：`eth1`；Windows：`以太网 3`）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub iface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ip: String,
    /// 掩码（点分 `255.255.255.0` 或前缀 `24`；归一化后统一点分）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mask: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gateway: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,
}

/// 校验一份配置：不合法就别拿去生成命令（半套命令比没有更糟）。
pub fn validate_plan(p: &HostNicPlan) -> Result<()> {
    if p.iface.trim().is_empty() {
        return Err(msg("先选一张网卡"));
    }
    if !is_v4(&p.ip) {
        return Err(msg(format!("IP 不是合法的 IPv4 地址：{:?}", p.ip)));
    }
    if prefix_from_mask(&p.mask).is_none() {
        return Err(msg(
            "掩码要么写点分格式（255.255.255.0），要么写前缀长度（24）",
        ));
    }
    let gw = p.gateway.trim();
    if !gw.is_empty() && !is_v4(gw) {
        return Err(msg(format!("网关不是合法的 IPv4 地址：{gw:?}")));
    }
    for d in &p.dns {
        let d = d.trim();
        if d.is_empty() {
            continue;
        }
        if !is_v4(d) {
            return Err(msg(format!("DNS 不是合法的 IPv4 地址：{d:?}")));
        }
    }
    Ok(())
}

/// 归一化：去首尾空格、掩码统一点分、DNS 去空去重。
pub fn normalize_plan(p: &HostNicPlan) -> HostNicPlan {
    let mut out = HostNicPlan {
        iface: p.iface.trim().to_string(),
        ip: p.ip.trim().to_string(),
        mask: p.mask.trim().to_string(),
        gateway: p.gateway.trim().to_string(),
        dns: Vec::new(),
    };
    if let Some(n) = prefix_from_mask(&out.mask) {
        if let Some(m) = mask_from_prefix(n) {
            out.mask = m;
        }
    }
    for d in &p.dns {
        let d = d.trim();
        if d.is_empty() || out.dns.iter().any(|x| x == d) {
            continue;
        }
        out.dns.push(d.to_string());
    }
    out
}

/// 按模组上报的本机 IP 推一份**建议值**（页面预填用，仅供参考）。
///
/// 思路就是"IP 对应"：主机取同网段里模组的邻居地址、网关指向模组本身。
/// 真正的网段/网关只有模组那份配置知道，所以这份值在页面上**可改**。
pub fn suggest_plan(modem_ip: &str) -> Option<HostNicPlan> {
    let o = parse_v4(modem_ip)?;
    // 同网段里最后一个字节取 2：避开 .0 / .1（网络号与常见网关）
    let mut last: u8 = 2;
    if o[3] == 2 {
        last = 3;
    }
    Some(HostNicPlan {
        iface: String::new(),
        ip: format!("{}.{}.{}.{}", o[0], o[1], o[2], last),
        mask: mask_from_prefix(24).unwrap_or_default(),
        gateway: format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]),
        dns: Vec::new(),
    })
}

// ------------------------------------------------------------------ 命令 ---

/// 三组动作。
pub const MODE_RENEW: &str = "renew"; // 先试 DHCP：重新请求租约
pub const MODE_STATIC: &str = "static"; // 配成静态 IP（FM350 要的就是这个）
pub const MODE_DHCP: &str = "dhcp"; // 恢复自动获取（撤销手工配置）

/// DHCP 请求的超时：FM350 对 DHCP **会应答但极慢**（实测几十秒到一两分钟），
/// 等满只代表"这次没等到"，不能下"没 DHCP"的结论。
pub const RENEW_TIMEOUT: Duration = Duration::from_secs(90);
/// 单条命令的默认超时（`ip` / `netsh` 都是秒级）。
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(20);

/// 一条要执行的命令。`text` 给人看/复制，`prog`+`args` 是真正执行用的（不拼 shell）。
#[derive(Debug, Clone, Serialize)]
pub struct HostNicCommand {
    pub label: String,
    pub text: String,
    /// 需要管理员 / root（页面把「直接执行」按钮标出来）
    #[serde(skip_serializing_if = "is_false")]
    pub admin_only: bool,
    /// 只显示不执行（会改固件自己的网络配置，得人看一眼再贴）
    #[serde(skip_serializing_if = "is_false")]
    pub manual: bool,
    #[serde(skip)]
    pub prog: String,
    #[serde(skip)]
    pub args: Vec<String>,
    #[serde(skip)]
    pub timeout: Option<Duration>,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 一组动作 + 该组要执行的命令。
#[derive(Debug, Clone, Serialize)]
pub struct HostNicActionSet {
    pub mode: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub commands: Vec<HostNicCommand>,
}

/// 命令行**展示**时的引号处理（执行走 args，不拼串）。
fn quote_arg(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }
    if s.contains([' ', '\t', '"']) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

fn cmd_text(prog: &str, args: &[String]) -> String {
    let mut parts = vec![prog.to_string()];
    parts.extend(args.iter().map(|a| quote_arg(a)));
    parts.join(" ")
}

fn cmd(label: &str, prog: &str, args: &[&str]) -> HostNicCommand {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    HostNicCommand {
        label: label.to_string(),
        text: cmd_text(prog, &args),
        admin_only: false,
        manual: false,
        prog: prog.to_string(),
        args,
        timeout: None,
    }
}

/// 生成某组动作要执行的命令（`platform`：`windows` / `linux` / `openwrt`）。
///
/// 认不出的平台**返回空**（不乱编命令：页面会显示"本平台没有现成命令"）。
pub fn commands(mode: &str, plan: &HostNicPlan, platform: &str) -> Vec<HostNicCommand> {
    let p = normalize_plan(plan);
    let prefix = prefix_from_mask(&p.mask).unwrap_or(0);
    let iface = p.iface.as_str();
    match platform {
        "windows" => match mode {
            MODE_RENEW => {
                let mut c = cmd(
                    "向模组重新请求 DHCP 租约（最多等 90 秒，这个模组的 DHCP 很慢）",
                    "ipconfig",
                    &["/renew", iface],
                );
                c.admin_only = true;
                c.timeout = Some(RENEW_TIMEOUT);
                vec![c]
            }
            MODE_STATIC => {
                // 没填网关就不拼那一段 —— netsh 允许不带网关，拼个空串反而报错
                let mut addr_args: Vec<String> = vec![
                    format!("name={iface}"),
                    "static".into(),
                    p.ip.clone(),
                    p.mask.clone(),
                ];
                let mut addr_text = format!(
                    "netsh interface ip set address name=\"{iface}\" static {} {}",
                    p.ip, p.mask
                );
                if !p.gateway.is_empty() {
                    addr_args.push(p.gateway.clone());
                    addr_text.push(' ');
                    addr_text.push_str(&p.gateway);
                }
                let mut full = vec![
                    "interface".to_string(),
                    "ip".into(),
                    "set".into(),
                    "address".into(),
                ];
                full.extend(addr_args);
                let mut first = cmd(
                    "把网卡配成静态 IP（会自动关掉该网卡的 DHCP）",
                    "netsh",
                    &full.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                );
                first.admin_only = true;
                first.text = addr_text;
                let mut out = vec![first];
                for (i, d) in p.dns.iter().enumerate() {
                    let (label, args, text) = if i == 0 {
                        (
                            "DNS（主）".to_string(),
                            vec![
                                "interface".to_string(),
                                "ip".into(),
                                "set".into(),
                                "dns".into(),
                                format!("name={iface}"),
                                "static".into(),
                                d.clone(),
                            ],
                            format!("netsh interface ip set dns name=\"{iface}\" static {d}"),
                        )
                    } else {
                        (
                            format!("DNS（第 {} 个）", i + 1),
                            vec![
                                "interface".to_string(),
                                "ip".into(),
                                "add".into(),
                                "dns".into(),
                                format!("name={iface}"),
                                d.clone(),
                                format!("index={}", i + 1),
                            ],
                            format!(
                                "netsh interface ip add dns name=\"{iface}\" {d} index={}",
                                i + 1
                            ),
                        )
                    };
                    let mut c = cmd(
                        &label,
                        "netsh",
                        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    );
                    c.admin_only = true;
                    c.text = text;
                    out.push(c);
                }
                out
            }
            MODE_DHCP => {
                let mut a = cmd(
                    "恢复成自动获取 IP",
                    "netsh",
                    &[
                        "interface",
                        "ip",
                        "set",
                        "address",
                        &format!("name={iface}"),
                        "source=dhcp",
                    ],
                );
                a.admin_only = true;
                a.text = format!("netsh interface ip set address name=\"{iface}\" source=dhcp");
                let mut b = cmd(
                    "恢复成自动获取 DNS",
                    "netsh",
                    &[
                        "interface",
                        "ip",
                        "set",
                        "dns",
                        &format!("name={iface}"),
                        "source=dhcp",
                    ],
                );
                b.admin_only = true;
                b.text = format!("netsh interface ip set dns name=\"{iface}\" source=dhcp");
                vec![a, b]
            }
            _ => Vec::new(),
        },
        "linux" | "openwrt" => {
            let openwrt = platform == "openwrt";
            match mode {
                MODE_RENEW => {
                    let mut c = cmd(
                        "向模组重新请求 DHCP 租约（最多等 90 秒，这个模组的 DHCP 很慢）",
                        "udhcpc",
                        &["-i", iface],
                    );
                    c.admin_only = true;
                    c.timeout = Some(RENEW_TIMEOUT);
                    vec![c]
                }
                MODE_STATIC => {
                    let cidr = if prefix > 0 {
                        format!("{}/{}", p.ip, prefix)
                    } else {
                        p.ip.clone()
                    };
                    let mut out = vec![
                        admin(cmd(
                            "先清掉旧地址（含 169.254 那条）",
                            "ip",
                            &["addr", "flush", "dev", iface],
                        )),
                        admin(cmd(
                            "配静态地址",
                            "ip",
                            &["addr", "add", &cidr, "dev", iface],
                        )),
                        admin(cmd("网卡上线", "ip", &["link", "set", iface, "up"])),
                    ];
                    if !p.gateway.is_empty() {
                        // replace 而不是 add：重复点「应用」时 add 会报 File exists
                        out.push(admin(cmd(
                            "默认路由指向模组（replace；报 File exists 就是 busybox 的 ip 不支持 replace，改用 UCI 块）",
                            "ip",
                            &["route", "replace", "default", "via", &p.gateway, "dev", iface],
                        )));
                    }
                    // OpenWrt 不吃 /etc/resolv.conf（DNS 由 dnsmasq 管）→ 放到 UCI 块里
                    if !openwrt {
                        for d in &p.dns {
                            let line = format!("echo 'nameserver {d}' >> /etc/resolv.conf");
                            let mut c = cmd(
                                "DNS（写进 resolv.conf；其它发行版可能该用 netplan/networkd）",
                                "sh",
                                &["-c", &line],
                            );
                            c.admin_only = true;
                            c.text = line;
                            out.push(c);
                        }
                    }
                    out
                }
                MODE_DHCP => vec![
                    admin(cmd("清掉静态地址", "ip", &["addr", "flush", "dev", iface])),
                    admin(cmd("重新用 DHCP 拿地址", "udhcpc", &["-i", iface])),
                ],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

fn admin(mut c: HostNicCommand) -> HostNicCommand {
    c.admin_only = true;
    c
}

/// 把配置写进固件自己的网络配置（目前只做 OpenWrt 的 UCI）。
///
/// ⚠️ 为什么需要它：上面的 `ip addr` / `ip route` 是**运行期**配置 —— OpenWrt 上
/// `/etc/init.d/network reload`（或重启、从 LuCI 改任何网络设置）都会把它抹掉，
/// 因为这些接口归 netifd 按 `/etc/config/network` 管。
/// 这些命令**只显示不执行**（`manual=true`）：接口名/防火墙分区要人判断。
pub fn persist_commands(plan: &HostNicPlan, platform: &str) -> Vec<HostNicCommand> {
    if platform != "openwrt" {
        return Vec::new();
    }
    let p = normalize_plan(plan);
    let iface = p.iface.clone();
    let mut out: Vec<HostNicCommand> = Vec::new();
    let mut push = |label: &str, args: &[&str]| {
        let mut c = cmd(label, args[0], &args[1..]);
        c.manual = true;
        out.push(c);
    };
    push(
        "先删同名的旧接口（没有也不报错）",
        &["uci", "-q", "delete", "network.modem"],
    );
    push(
        "建一个独立接口（别把模组口并进 br-lan）",
        &["uci", "set", "network.modem=interface"],
    );
    push(
        "静态 IP 协议",
        &["uci", "set", "network.modem.proto=static"],
    );
    push(
        "绑到模组那张网卡",
        &["uci", "set", &format!("network.modem.device={iface}")],
    );
    if !p.ip.is_empty() {
        push(
            "主机侧地址",
            &["uci", "set", &format!("network.modem.ipaddr={}", p.ip)],
        );
    }
    if !p.mask.is_empty() {
        push(
            "掩码",
            &["uci", "set", &format!("network.modem.netmask={}", p.mask)],
        );
    }
    if !p.gateway.is_empty() {
        push(
            "网关指向模组",
            &[
                "uci",
                "set",
                &format!("network.modem.gateway={}", p.gateway),
            ],
        );
    }
    for d in &p.dns {
        push(
            "DNS（交给 dnsmasq）",
            &["uci", "set", &format!("network.modem.dns={d}")],
        );
    }
    // WAN 也有一条 default，metric 小的优先
    push(
        "（可选）想让模组这条默认路由优先就调小 metric",
        &["uci", "set", "network.modem.metric=10"],
    );
    push("提交", &["uci", "commit", "network"]);
    push("重载生效", &["/etc/init.d/network", "reload"]);
    out
}

/// 三组动作（含提示语），页面按这个顺序排按钮。
pub fn action_sets(plan: &HostNicPlan, platform: &str) -> Vec<HostNicActionSet> {
    vec![
        HostNicActionSet {
            mode: MODE_RENEW.into(),
            label: "先试 DHCP".into(),
            hint: "让网卡重新请求一次租约：能拿到地址就说明模组发 DHCP，不用配静态。\
                   最多等 90 秒 —— 这个模组的 DHCP 实测很慢（几十秒到一两分钟），等满只代表“这次没等到”，\
                   过一会儿点「刷新」看网卡地址变没变；一直没有就手工配静态。"
                .into(),
            commands: commands(MODE_RENEW, plan, platform),
        },
        HostNicActionSet {
            mode: MODE_STATIC.into(),
            label: "应用静态配置".into(),
            hint: "FM350 要的就是这一步：把网卡配成与模组同网段、网关指向模组。".into(),
            commands: commands(MODE_STATIC, plan, platform),
        },
        HostNicActionSet {
            mode: MODE_DHCP.into(),
            label: "恢复自动获取".into(),
            hint: "撤销手工配置，恢复成 DHCP（换机型 / 还原现场时用）。".into(),
            commands: commands(MODE_DHCP, plan, platform),
        },
    ]
}

/// 执行一条命令（超时按命令自带的值；返回 stdout / stderr）。
pub(crate) fn run_command(c: &HostNicCommand) -> HostNicResult {
    let mut child = match std::process::Command::new(&c.prog)
        .args(&c.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(ch) => ch,
        Err(e) => {
            return HostNicResult {
                ok: false,
                output: String::new(),
                error: e.to_string(),
            };
        }
    };
    // 简单轮询等待（不引第三方依赖）：到点没退就 kill
    let budget = c.timeout.unwrap_or(EXEC_TIMEOUT);
    let deadline = std::time::Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return HostNicResult {
                        ok: false,
                        output: String::new(),
                        error: format!("等了 {} 秒还没返回（已终止）", budget.as_secs()),
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return HostNicResult {
                    ok: false,
                    output: String::new(),
                    error: e.to_string(),
                };
            }
        }
    }
    match child.wait_with_output() {
        Ok(out) => HostNicResult {
            ok: out.status.success(),
            output: decode_output(&out.stdout),
            error: decode_output(&out.stderr),
        },
        Err(e) => HostNicResult {
            ok: false,
            output: String::new(),
            error: e.to_string(),
        },
    }
}

/// 一条命令的执行结果。
#[derive(Debug, Clone, Serialize)]
pub struct HostNicResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// 命令输出 → 合法 UTF-8（Windows 的 netsh/ipconfig 可能输出系统代码页，这里只做兜底）。
fn decode_output(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap_or_else(|e| {
        let v = e.into_bytes();
        String::from_utf8_lossy(&v).into_owned()
    })
}

// ------------------------------------------------------------------ 本机 ---

/// 主机上的一张网卡（页面下拉框用）。
#[derive(Debug, Clone, Serialize)]
pub struct HostNicIface {
    pub name: String,
    pub up: bool,
    /// 形如 `10.0.0.2/24`
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub addrs: Vec<String>,
    /// 名字本身像 USB / WWAN 数据口
    pub strong: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// 当前平台：`windows` / `linux` / `openwrt`（后者决定要不要出 UCI 持久化块）。
pub fn platform() -> &'static str {
    if std::env::consts::OS == "linux" && is_openwrt() {
        "openwrt"
    } else {
        std::env::consts::OS
    }
}

/// 本机是不是 OpenWrt / LEDE（两个判据有一个就够）。
pub fn is_openwrt() -> bool {
    std::path::Path::new("/etc/openwrt_release").exists()
        || std::path::Path::new("/etc/config/network").exists()
}

/// 本进程是否具备改主机网卡的权限（Linux 看 euid 是不是 0；Windows 试开物理磁盘句柄）。
pub fn elevated() -> bool {
    #[cfg(windows)]
    {
        std::fs::OpenOptions::new()
            .read(true)
            .open(r"\\.\PHYSICALDRIVE0")
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        // 没有 libc 依赖：读 /proc/self/status 的 Uid 行（第一项即 euid）
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Uid:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse::<i32>().ok())
            })
            .map(|uid| uid == 0)
            .unwrap_or(false)
    }
}

/// 明显与模组数据口无关的网卡（虚拟 / 无线 / 蓝牙 / 桥接 / 容器）。
fn is_virtual_iface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "lo",
        "docker",
        "br-",
        "virbr",
        "veth",
        "tun",
        "tap",
        "wlan",
        "wifi",
        "wl",
        "ra",
        "rai",
        "bluetooth",
        "bnep",
        "sit",
        "gre",
        "ifb",
        "dummy",
        "pppoe",
        // OpenWrt 的无线口（phy0-ap0 / phy1-ap0）：与模组数据口无关
        "phy",
    ]
    .iter()
    .any(|b| n.contains(b))
}

/// 数据通道网卡的驱动（与 FM350 参考实现同口径）。
///
/// ⚠️ 这里**没有** `ccmni`：那是模组内部 AP 域的 CCCI 网卡，主机侧不存在。
pub const DATA_DRIVERS: &[&str] = &["rndis_host", "cdc_ether", "cdc_ncm", "qmi_wwan", "mbim"];

/// 板载 / 虚拟 / 无线网卡的驱动：它们**不是**模组数据口（选错会把 LAN/WAN 口配成蜂窝口）。
const ONBOARD_DRIVERS: &[&str] = &[
    "mtk_soc_eth",
    "mt798x-wmac",
    "bridge",
    "dummy",
    "bonding",
    "veth",
    "tun",
    "tap",
    "ifb",
    "rtl8367",
    "rtl83xx",
    "dsa",
    "gmac",
    "stmmac",
    "bcmgenet",
    "xgmac",
];

/// 已知模组厂商的 USB VID（探测到多个数据口时优先挑它们，与 FM350 参考实现同思路）。
const MODEM_VIDS: &[&str] = &[
    "2cb7", // Fibocom 广和通
    "0e8d", // MediaTek（FM350 那类 MTK 平台模组）
    "2c7c", // Quectel 移远
    "1e0e", // SIMCom 芯讯通
    "2dee", // MeiG 美格
    "19d2", // ZTE 中兴
    "1782", // TD Tech 鼎桥
    "2df3", // Unisoc 展锐
];

/// 读某张网卡的驱动名（sysfs `device/driver` 的 basename；拿不到返回空串）。
pub fn iface_driver(name: &str) -> String {
    std::fs::read_link(format!("/sys/class/net/{name}/device/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
        .unwrap_or_default()
}

/// 读某张网卡所属 USB 设备的 `idVendor`（sysfs 逐级向上找）。
pub fn iface_usb_vid(name: &str) -> Option<String> {
    let mut dir = std::path::PathBuf::from(format!("/sys/class/net/{name}/device"));
    for _ in 0..6 {
        if let Ok(v) = std::fs::read_to_string(dir.join("idVendor")) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// 驱动是否属于板载 / 虚拟 / 无线（不该被当成模组数据口）。
pub fn is_onboard_iface(name: &str) -> bool {
    let drv = iface_driver(name).to_ascii_lowercase();
    if drv.is_empty() {
        return false;
    }
    ONBOARD_DRIVERS.iter().any(|d| drv.contains(d))
}

/// 名字像模组数据口（只在拿不到驱动时兜底用；**不含 eth0/eth1** —— 那俩通常是板载 LAN/WAN）。
fn is_strong_iface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "wwan", "rmnet", "usb0", "wwan0", "cmh", "qmi", "mbim", "ecm", "ncm", "rndis",
    ]
    .iter()
    .any(|b| n.contains(b))
}

/// 探测数据通道网卡（FM350 参考实现 `detect_dev` 同口径，多厂商版本）。
///
/// 顺序：① 驱动命中的网卡里挑（多个时先挑**模组 VID** 的那张，再按名字排序保证可复现）；
/// ② 兜底按常见名字 `eth2` / `eth3` / `wwan0` / `usb0`。
/// `exclude` 是被别的接口（lan / wan / br-lan…）占用的网卡 —— 绝不能把用户正在上网的口
/// 当成模组数据口配掉（实机踩过：eth0 是 LAN、eth1 是 WAN）。
pub fn detect_data_dev(exclude: &[String]) -> Option<String> {
    let rd = std::fs::read_dir("/sys/class/net").ok()?;
    let mut by_driver: Vec<String> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if exclude.iter().any(|x| x == &name) || is_virtual_iface(&name) || is_onboard_iface(&name)
        {
            continue;
        }
        let drv = iface_driver(&name).to_ascii_lowercase();
        if DATA_DRIVERS.iter().any(|d| drv.contains(d)) {
            by_driver.push(name);
        }
    }
    if !by_driver.is_empty() {
        by_driver.sort();
        if let Some(hit) = by_driver.iter().find(|n| {
            iface_usb_vid(n)
                .as_deref()
                .is_some_and(|v| MODEM_VIDS.contains(&v))
        }) {
            return Some(hit.clone());
        }
        return Some(by_driver.remove(0));
    }
    for name in ["eth2", "eth3", "wwan0", "usb0"] {
        if std::path::Path::new(&format!("/sys/class/net/{name}")).exists()
            && !exclude.iter().any(|x| x == name)
        {
            return Some(name.to_string());
        }
    }
    None
}

/// 列出主机上网卡（去掉环回与明显虚拟网卡），模组口排前面。
pub fn interfaces() -> Vec<HostNicIface> {
    let raw = list_ifaces();
    let mut out: Vec<HostNicIface> = raw
        .into_iter()
        .filter(|it| !is_virtual_iface(&it.name))
        .collect();
    for it in &mut out {
        let has_v4 = it
            .addrs
            .iter()
            .any(|a| is_v4(a.split('/').next().unwrap_or("")));
        let has_other4 = it.addrs.iter().any(|a| match a.split('/').next() {
            Some(ip) => is_v4(ip) && !ip.starts_with("169.254."),
            None => false,
        });
        let drv = iface_driver(&it.name).to_ascii_lowercase();
        it.strong = DATA_DRIVERS.iter().any(|d| drv.contains(d)) || is_strong_iface(&it.name);
        if it.addrs.is_empty() || !has_v4 {
            it.note = "没有 IPv4 地址".to_string();
        } else if !has_other4 {
            // 只有 169.254 = 没拿到 DHCP。只有**像模组口**的那张才提 FM350 那件事，
            // 否则蓝牙 / 手机网络这些也挂 169.254，全写一遍会误导。
            it.note = if it.strong {
                "只有 169.254.x.x（没拿到 DHCP —— 这个机型要手工配一次）".to_string()
            } else {
                "只有 169.254.x.x（没拿到 DHCP）".to_string()
            };
        }
    }
    out.sort_by(|a, b| {
        b.strong
            .cmp(&a.strong)
            .then(b.up.cmp(&a.up))
            .then(a.name.cmp(&b.name))
    });
    out
}

/// 读本机的网卡名与地址（`ip addr` 解析，失败再退 `/sys/class/net` 只列名字）。
#[cfg(unix)]
fn list_ifaces() -> Vec<HostNicIface> {
    let mut out = parse_ip_addr(&run_text("ip", &["addr", "show"]));
    if out.is_empty() {
        if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let up = std::fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
                    .map(|s| s.trim() == "up")
                    .unwrap_or(false);
                out.push(HostNicIface {
                    name,
                    up,
                    addrs: Vec::new(),
                    strong: false,
                    note: String::new(),
                });
            }
        }
    }
    out
}

#[cfg(not(unix))]
fn list_ifaces() -> Vec<HostNicIface> {
    parse_netsh(&run_text(
        "netsh",
        &["interface", "ip", "show", "addresses"],
    ))
}

/// 执行一条只读命令取文本输出（失败返回空串）。
fn run_text(prog: &str, args: &[&str]) -> String {
    let mut child = match std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                return String::new();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return String::new(),
        }
    }
    child
        .wait_with_output()
        .map(|o| decode_output(&o.stdout))
        .unwrap_or_default()
}

/// 解析 `ip addr show` 的输出（busybox 与 iproute2 都是这个形）。
fn parse_ip_addr(text: &str) -> Vec<HostNicIface> {
    let mut out: Vec<HostNicIface> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        // "2: eth0: <...UP...> mtu 1500 ..."
        if let Some(rest) = strip_index_line(t) {
            let name = rest.split(':').next().unwrap_or("").trim().to_string();
            if !name.is_empty() {
                out.push(HostNicIface {
                    up: t.contains("UP"),
                    name,
                    addrs: Vec::new(),
                    strong: false,
                    note: String::new(),
                });
            }
            continue;
        }
        if let Some(addr) = t.strip_prefix("inet ") {
            if let Some(last) = out.last_mut() {
                let v = addr.split_whitespace().next().unwrap_or("").to_string();
                if !v.is_empty() {
                    last.addrs.push(v);
                }
            }
        }
    }
    out
}

/// "2: eth0: <...>" → "eth0: <...>"（不是接口头则返回 None）。
fn strip_index_line(line: &str) -> Option<&str> {
    let colon = line.find(':')?;
    let idx = &line[..colon];
    if idx.is_empty() || !idx.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = line[colon + 1..].trim_start();
    rest.contains(':').then_some(rest)
}

/// 解析 Windows `netsh interface ip show addresses` 的输出。
#[cfg(windows)]
fn parse_netsh(text: &str) -> Vec<HostNicIface> {
    let mut out: Vec<HostNicIface> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("Configuration for interface") {
            let name = l
                .trim_start_matches("Configuration for interface")
                .trim()
                .trim_matches('"')
                .to_string();
            if !name.is_empty() {
                out.push(HostNicIface {
                    name,
                    up: true,
                    addrs: Vec::new(),
                    strong: false,
                    note: String::new(),
                });
            }
            continue;
        }
        if let Some(v) = l.strip_prefix("IP Address:") {
            if let Some(last) = out.last_mut() {
                let a = v.trim().to_string();
                if !a.is_empty() {
                    last.addrs.push(a);
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ 台账 ---

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostNicFile {
    version: i64,
    #[serde(default)]
    saved_at: String,
    #[serde(default)]
    plans: HashMap<String, HostNicPlan>,
}

/// 已保存的主机网卡配置（`hostnic.json`，与配置文件同目录）。
pub struct HostNicStore {
    path: String,
    inner: std::sync::Mutex<HashMap<String, HostNicPlan>>,
}

impl HostNicStore {
    pub fn new(path: String) -> Self {
        let mut inner: HashMap<String, HostNicPlan> = HashMap::new();
        if let Some(f) = super::store::load_json::<HostNicFile>(&path) {
            for (k, v) in f.plans {
                let p = normalize_plan(&v);
                if !p.iface.trim().is_empty() {
                    inner.insert(k.trim().to_string(), p);
                }
            }
        }
        Self {
            path,
            inner: std::sync::Mutex::new(inner),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// 已保存过配置的网卡名（按字母序）。
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.inner.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }

    /// 全部已保存的配置（页面切换下拉时把对应参数带出来）。
    pub fn saved_plans(&self) -> HashMap<String, HostNicPlan> {
        self.inner.lock().unwrap().clone()
    }

    /// 取某张网卡保存过的配置。
    pub fn get(&self, iface: &str) -> Option<HostNicPlan> {
        self.inner.lock().unwrap().get(iface.trim()).cloned()
    }

    /// 保存一张网卡的配置（校验不通过直接报错，不落盘）。
    pub fn save(&self, plan: &HostNicPlan) -> Result<HostNicPlan> {
        let p = normalize_plan(plan);
        validate_plan(&p)?;
        {
            let mut guard = self.inner.lock().unwrap();
            guard.insert(p.iface.clone(), p.clone());
        }
        self.flush();
        log::info!(
            "主机网卡配置已保存：{} {} mask={} gw={}",
            p.iface,
            p.ip,
            p.mask,
            p.gateway
        );
        Ok(p)
    }

    /// 原子写盘（.tmp + rename）。
    fn flush(&self) {
        if self.path.trim().is_empty() {
            return;
        }
        let plans = self.inner.lock().unwrap().clone();
        let payload = HostNicFile {
            version: 1,
            saved_at: timestamp(),
            plans,
        };
        let Ok(text) = serde_json::to_string_pretty(&payload) else {
            log::warn!("序列化主机网卡台账失败");
            return;
        };
        super::store::write_json_atomic(&self.path, &text);
    }
}

/// 台账文件路径（与配置文件同目录）。
pub fn hostnic_file(config_path: &str) -> String {
    let p = std::path::Path::new(config_path);
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.join("hostnic.json").to_string_lossy().to_string(),
        None => "hostnic.json".to_string(),
    }
}

#[cfg(test)]
mod tests;
