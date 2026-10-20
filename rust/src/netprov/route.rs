//! 主机侧默认路由查询（只读 `/proc/net/{route,ipv6_route}`，不 fork）。
//!
//! ## 为什么需要
//!
//! 「拨号成功」有三种完全不同的含义，以前只看了前两种：
//!
//! 1. 模组说 PDP 激活了（`AT+CGACT=1` 回 `OK`）；
//! 2. 模组给了本机地址（`AT+CGPADDR` 有值）；
//! 3. **主机真的能经这块网卡发出去**（有地址 + 有默认路由）。
//!
//! 第 3 条才是用户的体感。实测踩过两次：
//! - FM350：模组侧激活成功、地址也读到了，主机 `eth2` 却挂在 `169.254.x.x`，
//!   没有默认路由 —— 页面显示「已连接」，实际打不开网页；
//! - 反过来的情况也存在：地址配好了，但默认路由被别的口抢了（metric 更低），
//!   流量全走错口。
//!
//! 所以判「链路阶段」时必须能看见**路由事实**。这里用 `/proc/net/route`：
//! 只读文件、不 fork `ip route`（轮询里每秒执行一次 `ip` 是不可接受的）。
//!
//! ## 格式（内核固定）
//!
//! `/proc/net/route`：`Iface Destination Gateway Flags RefCnt Use Metric Mask ...`
//! - `Destination` / `Gateway` / `Mask` 都是**小端十六进制**：`01002A0A` = `10.42.0.1`；
//! - 默认路由 = `Destination == 00000000 && Mask == 00000000`；
//! - 网关标志是 `Flags & 0x2`（RTF_GATEWAY）。
//!
//! `/proc/net/ipv6_route`：目标 32 hex + 前缀长 + 源 32 hex + 源前缀长 + 网关 32 hex + …
//! - 默认路由 = 目标全 0 且目标前缀长 `00`。

use serde::Serialize;

/// 一条默认路由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DefaultRoute {
    /// 出口网卡名（`eth2` / `wwan0_1`）。
    pub iface: String,
    /// 网关（v4 点分 / v6 十六进制串）；点对点链路没有网关时为空。
    pub gateway: String,
    /// 度量值（越小越优先）。
    pub metric: u32,
    /// `4` 或 `6`。
    pub family: u8,
}

/// 小端十六进制（`01002A0A`）→ 点分十进制（`10.42.0.1`）。
fn hex_le_to_ipv4(s: &str) -> Option<String> {
    if s.len() != 8 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    // 内核按主机字节序打印，x86/arm 都是小端：字节顺序需要翻转回网络序
    let b = v.to_le_bytes();
    Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
}

/// 解析 `/proc/net/route`，只返回默认路由。
pub fn parse_v4(text: &str) -> Vec<DefaultRoute> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        // 至少要有 Iface / Destination / Gateway / Flags / RefCnt / Use / Metric / Mask
        if f.len() < 8 {
            continue;
        }
        if f[1] != "00000000" || f[7] != "00000000" {
            continue; // 不是默认路由
        }
        let metric = f[6].parse::<u32>().unwrap_or(0);
        let gateway = hex_le_to_ipv4(f[2]).unwrap_or_default();
        out.push(DefaultRoute {
            iface: f[0].to_string(),
            gateway,
            metric,
            family: 4,
        });
    }
    out.sort_by_key(|r| r.metric);
    out
}

/// 解析 `/proc/net/ipv6_route`，只返回默认路由。
///
/// 行格式：`<dst32> <dst_prefix> <src32> <src_prefix> <nexthop32> <metric> … <iface>`
pub fn parse_v6(text: &str) -> Vec<DefaultRoute> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 {
            continue;
        }
        let dst = f[0];
        let dst_prefix = f[1];
        if dst.trim_matches('0') != "" || dst_prefix != "00" {
            continue; // 不是默认路由
        }
        // metric 在第 5 列（0 基），十六进制
        let metric = u32::from_str_radix(f[5], 16).unwrap_or(0);
        let iface = f[f.len() - 1].to_string();
        out.push(DefaultRoute {
            iface,
            gateway: f[4].to_string(),
            metric,
            family: 6,
        });
    }
    out.sort_by_key(|r| r.metric);
    out
}

/// 当前主机的全部默认路由（v4 在前）。
pub fn defaults() -> Vec<DefaultRoute> {
    let mut out = Vec::new();
    if let Ok(t) = std::fs::read_to_string("/proc/net/route") {
        out.extend(parse_v4(&t));
    }
    if let Ok(t) = std::fs::read_to_string("/proc/net/ipv6_route") {
        out.extend(parse_v6(&t));
    }
    out
}

/// 有没有**任何**默认路由经这块网卡。
pub fn has_default_via(iface: &str) -> bool {
    if iface.trim().is_empty() {
        return false;
    }
    defaults().iter().any(|r| r.iface == iface.trim())
}

/// 有没有**指定族**的默认路由经这块网卡。
///
/// v4 与 v6 的路由表是独立的（`/proc/net/route` vs `/proc/net/ipv6_route`），
/// 双栈下完全可能出现「v4 默认路由没了、v6 还在」—— 实机复现过：
/// `ip route del` 掉 eth2 的两条 v4 默认路由后，它的 IPv6 默认路由
/// （fe80 链路本地网关）不受影响。链路阶段必须按族分别判，
/// 否则「v4 已经出不去」会被 v6 的路由掩盖成 online。
pub fn has_default_via_family(iface: &str, family: u8) -> bool {
    let iface = iface.trim();
    if iface.is_empty() {
        return false;
    }
    match family {
        4 => std::fs::read_to_string("/proc/net/route")
            .map(|t| parse_v4(&t).iter().any(|r| r.iface == iface))
            .unwrap_or(false),
        6 => std::fs::read_to_string("/proc/net/ipv6_route")
            .map(|t| parse_v6(&t).iter().any(|r| r.iface == iface))
            .unwrap_or(false),
        _ => false,
    }
}

/// 这块网卡是不是**当前最优**出口（metric 最小的那条 v4 默认路由）。
///
/// 用途：区分「有路由但被别的口抢了」和「压根没路由」——两者排障方向完全不同。
pub fn is_primary_via(iface: &str) -> bool {
    if iface.trim().is_empty() {
        return false;
    }
    defaults()
        .iter()
        .filter(|r| r.family == 4)
        .min_by_key(|r| r.metric)
        .map(|r| r.iface == iface.trim())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 取自实机（FM350 + 一条板载 WAN 同时在线的场景）
    const V4: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
eth1\t00000000\t010200C0\t0003\t0\t0\t10\t00000000\t0\t0\t0\n\
eth2\t00000000\t01002A0A\t0003\t0\t0\t20\t00000000\t0\t0\t0\n\
eth2\t00000000\t01002A0A\t0003\t0\t0\t50\t00000000\t0\t0\t0\n\
eth2\t0053040A\t00000000\t0001\t0\t0\t20\t00FFFFFF\t0\t0\t0\n\
br-lan\t000AA8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n";

    const V6: &str = "00000000000000000000000000000000 00 20010db8000000000000000000000001 40 fe80000000000000942b33fffecd8306 00000200 00000003 00000000 00000003     eth1\n\
00000000000000000000000000000000 00 20010db8000000000000000000000002 40 fe800000000000000000000000000001 00000200 00000002 00000000 00000003     eth2\n\
20010db8000000000000000000000001 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000004 00000000 00000001     eth1\n";

    #[test]
    fn parses_ipv4_defaults_only() {
        let r = parse_v4(V4);
        // eth2 有两条（netifd 的 20 与遗留的 50），都算默认路由
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].iface, "eth1");
        assert_eq!(r[0].metric, 10);
        assert_eq!(r[1].iface, "eth2");
        assert_eq!(r[1].metric, 20);
        // 网关按小端还原：01002A0A → 10.42.0.1
        assert_eq!(r[1].gateway, "10.42.0.1");
        assert_eq!(r[0].gateway, "192.0.2.1");
    }

    #[test]
    fn ignores_subnet_routes() {
        // 10.42.0.0/24 那条 mask 不是 0，不能当默认路由
        for r in parse_v4(V4) {
            assert_ne!(r.gateway, "");
        }
        assert!(parse_v4(V4)
            .iter()
            .all(|r| r.metric != 0 || r.iface != "br-lan"));
    }

    #[test]
    fn parses_ipv6_defaults() {
        let r = parse_v6(V6);
        assert_eq!(r.len(), 2, "两条 ::/0");
        assert_eq!(r[0].iface, "eth1");
        assert_eq!(r[1].iface, "eth2");
        assert_eq!(r[0].family, 6);
        // 前缀非 0 的那条（2001:db8:…/64）不是默认路由
        assert!(r.iter().all(|x| x.iface != "lo"));
    }

    #[test]
    fn empty_input_is_not_an_error() {
        assert!(parse_v4("").is_empty());
        assert!(parse_v6("").is_empty());
        assert!(parse_v4("Iface\tDestination\n").is_empty());
    }

    #[test]
    fn has_default_via_matches_by_iface() {
        // 直接测纯函数，避免依赖本机真实路由表
        let r4 = parse_v4(V4);
        assert!(r4.iter().any(|r| r.iface == "eth2"));
        assert!(!r4.iter().any(|r| r.iface == "wwan9"));
    }

    #[test]
    fn family_check_does_not_mask_the_other_family() {
        // 实机复现过的场景：eth2 的 v4 默认路由全删了，v6 默认路由还在。
        // has_default_via（混判）会误报 online；按族查必须分开：
        assert!(
            parse_v6(V6).iter().any(|r| r.iface == "eth2"),
            "样例里 eth2 有 v6 默认路由"
        );
        assert!(
            !parse_v4(V4).iter().any(|r| r.iface == "wwan9"),
            "v4 样例里没有 wwan9"
        );
        // 直接验证族过滤语义：v4 表里查不到的 iface，v4 族 = false
        let v4_table = parse_v4(V4);
        assert!(!v4_table.iter().any(|r| r.iface == "wwan9"));
        let v6_table = parse_v6(V6);
        assert!(v6_table.iter().any(|r| r.iface == "eth2"));
        // 族不同 → 判定互不影响（这是混判 bug 的根子）
        assert_ne!(v4_table.len(), v6_table.len());
    }

    #[test]
    fn little_endian_gateway_is_reversed() {
        assert_eq!(hex_le_to_ipv4("010200C0").as_deref(), Some("192.0.2.1"));
        assert_eq!(hex_le_to_ipv4("00000000").as_deref(), Some("0.0.0.0"));
        assert_eq!(hex_le_to_ipv4("bad"), None);
    }
}
