//! `netprov::v6` 的纯逻辑单测（sysctl / RS / 路由这些要真机，见 PROGRESS §30）。

use std::time::Duration;

use super::*;

#[test]
fn mode_parse_matches_reference_semantics() {
    assert_eq!(V6Mode::parse("dhcpv6"), V6Mode::Dhcpv6);
    assert_eq!(V6Mode::parse("DHCPv6"), V6Mode::Dhcpv6);
    assert_eq!(V6Mode::parse("ra"), V6Mode::Ra);
    assert_eq!(V6Mode::parse("SLAAC"), V6Mode::Ra);
    assert_eq!(V6Mode::parse("static"), V6Mode::Static);
    assert_eq!(V6Mode::parse("off"), V6Mode::Off);
    assert_eq!(V6Mode::parse("none"), V6Mode::Off);
    // ★ 关键：缺省 / 拼错绝不掉进 RA（那正是「默认 IPv6 不对」的根因）
    for raw in ["", "  ", "bogus", "5g"] {
        assert_eq!(V6Mode::parse(raw), V6Mode::Dhcpv6, "raw={raw:?}");
    }
    assert_eq!(V6Mode::default(), V6Mode::Dhcpv6);
}

#[test]
fn mode_shape_and_sibling_policy() {
    assert_eq!(V6Mode::Off.sibling_proto(), "none");
    assert!(!V6Mode::Off.wants_sibling());
    assert_eq!(V6Mode::Dhcpv6.sibling_proto(), "dhcpv6");
    assert_eq!(V6Mode::Ra.sibling_proto(), "none");
    assert_eq!(V6Mode::Static.sibling_proto(), "static");
    for m in [V6Mode::Dhcpv6, V6Mode::Ra, V6Mode::Static] {
        assert!(m.wants_sibling(), "{m:?} 该建兄弟接口");
    }
    // 模式清单给页面用：default 恰好一个，且是 dhcpv6
    let payload = v6_modes_payload();
    assert_eq!(payload.len(), 4);
    assert_eq!(
        payload.iter().filter(|v| v["default"] == true).count(),
        1,
        "只能有一个默认模式"
    );
}

#[test]
fn link_local_and_placeholder_detection() {
    assert!(is_link_local_v6("fe80::1"));
    assert!(is_link_local_v6("FE80::200:11ff:fe12:1314"));
    assert!(is_link_local_v6("febf::1"));
    assert!(!is_link_local_v6("2001:db8:1:2::1"));
    assert!(!is_link_local_v6("::1"));
}

#[test]
fn placeholder_and_non_global_v6_is_rejected() {
    // 实机 `AT+CGPADDR` 在只有 IPv4 时的 v6 位（解码后就是这个）
    assert!(!is_usable_global_v6("::18df:9a01:f982:815a"));
    assert!(!is_usable_global_v6("::"));
    assert!(!is_usable_global_v6("::1"));
    assert!(!is_usable_global_v6("fe80::200:11ff:fe12:1314"));
    assert!(!is_usable_global_v6("ff02::1"));
    assert!(!is_usable_global_v6(""));
    // 可路由的全局地址（此处用 RFC 3849 文档段，形态同实机 24xx 前缀）
    assert!(is_usable_global_v6("2001:db8::1"));
    assert!(is_usable_global_v6("2001:db8:1:2::8"));
    assert!(is_usable_global_v6("2001:DB8:1:2::8"));
    // 带前缀长度也要认（调用方可能直接给 `<addr>/64`）
    assert!(is_usable_global_v6("2001:db8:1:2::8/64"));
}

#[test]
fn static_v6_picks_only_usable_address() {
    assert_eq!(
        usable_static_v6("2001:db8:1:2::8/64").as_deref(),
        Some("2001:db8:1:2::8")
    );
    assert_eq!(
        usable_static_v6(" 2001:DB8::1 ").as_deref(),
        Some("2001:db8::1")
    );
    // 残值 / 链路本地 / 空 ⇒ 不能当静态地址
    assert_eq!(usable_static_v6("::18df:9a01:f982:815a"), None);
    assert_eq!(usable_static_v6("fe80::1"), None);
    assert_eq!(usable_static_v6(""), None);
}

#[test]
fn addr_line_filters_lifetime_and_link_local() {
    let ok = "15: eth2    inet6 2001:db8:1:2::8/64 scope global dynamic \\ valid_lft 3588sec preferred_lft 3588sec";
    let deprecated = "15: eth2    inet6 2001:db8:1:2::9/64 scope global dynamic \\ valid_lft 120sec preferred_lft 0sec";
    let expired = "15: eth2    inet6 2001:db8:1:2::a/64 scope global dynamic \\ valid_lft 0sec preferred_lft 0sec";
    let forever = "15: eth2    inet6 2001:db8:1:2::b/64 scope global \\ valid_lft forever preferred_lft forever";
    let link_local = "15: eth2    inet6 fe80::200:11ff:fe12:1314/64 scope link \\ valid_lft forever preferred_lft forever";
    let placeholder = "15: eth2    inet6 ::18df:9a01:f982:815a/128 scope global \\ valid_lft forever preferred_lft forever";

    assert_eq!(
        usable_v6_from_addr_line(ok).as_deref(),
        Some("2001:db8:1:2::8")
    );
    assert_eq!(
        usable_v6_from_addr_line(deprecated).as_deref(),
        Some("2001:db8:1:2::9"),
        "deprecated 但 valid 未归零，仍算可用"
    );
    assert_eq!(usable_v6_from_addr_line(expired), None);
    assert_eq!(
        usable_v6_from_addr_line(forever).as_deref(),
        Some("2001:db8:1:2::b")
    );
    assert_eq!(usable_v6_from_addr_line(link_local), None);
    assert_eq!(usable_v6_from_addr_line(placeholder), None);
}

#[test]
fn resolicit_gate() {
    // 刚启动不抢（netifd 还在拉接口）
    assert!(!should_resolicit(
        V6Mode::Ra,
        "eth2",
        Duration::from_secs(5),
        false
    ));
    // 起来一会儿、还没地址 ⇒ 该补
    assert!(should_resolicit(
        V6Mode::Ra,
        "eth2",
        Duration::from_secs(31),
        false
    ));
    // 已经有 IPv6 / 设备名空 / 不是 RA 模式 ⇒ 不动
    assert!(!should_resolicit(
        V6Mode::Ra,
        "eth2",
        Duration::from_secs(31),
        true
    ));
    assert!(!should_resolicit(
        V6Mode::Ra,
        "",
        Duration::from_secs(31),
        false
    ));
    assert!(!should_resolicit(
        V6Mode::Dhcpv6,
        "eth2",
        Duration::from_secs(31),
        false
    ));
}

#[test]
fn recovery_gap_tiers() {
    // 15 秒 ×10
    for n in 0..KICK_FAST_TIMES {
        assert_eq!(recovery_gap(n), KICK_FAST_INTERVAL, "attempt {n}");
    }
    // 61 秒 ×10
    for n in KICK_FAST_TIMES..KICK_FAST_TIMES + KICK_SLOW_TIMES {
        assert_eq!(recovery_gap(n), KICK_SLOW_INTERVAL, "attempt {n}");
    }
    // 之后 3 分钟兜底，**不封顶**（纯 IPv4 环境也不能停止巡检）
    for n in [20u32, 21, 50, 500, 100_000] {
        assert_eq!(recovery_gap(n), KICK_FALLBACK_INTERVAL, "attempt {n}");
    }
    assert_eq!(KICK_FAST_INTERVAL, Duration::from_secs(15));
    assert_eq!(KICK_SLOW_INTERVAL, Duration::from_secs(61));
    assert_eq!(KICK_FALLBACK_INTERVAL, Duration::from_secs(180));
}

#[test]
fn kick_gate_only_when_v6_missing_and_interface_ready() {
    // 拿到 v6 ⇒ 永不重拉
    assert!(!should_kick(V6Mode::Dhcpv6, true, 120, true, 0, None));
    // 接口没 up：只有第一次（since_last 为空）立刻拉起
    assert!(should_kick(V6Mode::Dhcpv6, false, 0, false, 0, None));
    // 已经拉过就要退避 —— 否则组网循环会把 odhcp6c 反复杀掉，
    // 接口永远停在 pending（实机日志：「已运行 0s …（第 N 次重拉）」每轮 +1）
    assert!(!should_kick(
        V6Mode::Dhcpv6,
        false,
        0,
        false,
        3,
        Some(Duration::from_secs(1))
    ));
    // 快挡：第 1 次后 15 秒才允许再拉
    assert!(!should_kick(
        V6Mode::Dhcpv6,
        false,
        0,
        false,
        1,
        Some(Duration::from_secs(14))
    ));
    assert!(should_kick(
        V6Mode::Dhcpv6,
        false,
        0,
        false,
        1,
        Some(Duration::from_secs(15))
    ));
    // 慢挡：第 12 次后要等 61 秒
    assert!(!should_kick(
        V6Mode::Dhcpv6,
        false,
        0,
        false,
        12,
        Some(Duration::from_secs(60))
    ));
    assert!(should_kick(
        V6Mode::Dhcpv6,
        false,
        0,
        false,
        12,
        Some(Duration::from_secs(61))
    ));
    // 兜底：第 30 次后要等 180 秒
    assert!(!should_kick(
        V6Mode::Dhcpv6,
        false,
        0,
        false,
        30,
        Some(Duration::from_secs(179))
    ));
    assert!(should_kick(
        V6Mode::Dhcpv6,
        false,
        0,
        false,
        30,
        Some(Duration::from_secs(180))
    ));
    // 刚起来不到 10 秒不抢（RA/DHCPv6 还在协商）；下限必须小于快挡 15 秒，
    // 否则快挡会被这条挡掉、实际节奏退化成 30 秒（实机踩过）
    assert!(!should_kick(V6Mode::Dhcpv6, true, 9, false, 0, None));
    assert!(should_kick(V6Mode::Dhcpv6, true, 10, false, 0, None));
    assert!(KICK_MIN_UPTIME < KICK_FAST_INTERVAL.as_secs());
    // static 不归这条路（地址由 UCI 配）
    assert!(!should_kick(V6Mode::Static, true, 600, false, 0, None));
    assert!(!should_kick(V6Mode::Off, true, 600, false, 0, None));
}

#[test]
fn ifstatus_v6_detection_accepts_addr_or_prefix_only() {
    let with_addr = serde_json::json!({
        "up": true,
        "data": {
            "ipv6-address": [
                {"address": "fe80::1", "mask": 64},
                {"address": "2001:db8::1", "mask": 64}
            ]
        }
    });
    assert!(status_has_v6(&with_addr));
    let prefix_only = serde_json::json!({
        "up": true,
        "data": {
            "ipv6-prefix-assignment": [
                {"address": "2001:db8:1:2::", "mask": 64}
            ]
        }
    });
    assert!(status_has_v6(&prefix_only), "只拿到 PD 前缀也算 v6 已就绪");
    let v4_only = serde_json::json!({
        "up": true,
        "data": {"ipv4-address": [{"address": "10.0.0.2", "mask": 24}]}
    });
    assert!(!status_has_v6(&v4_only));
    let placeholder = serde_json::json!({
        "up": true,
        "data": {"ipv6-address": [{"address": "::18df:9a01:f982:815a", "mask": 128}]}
    });
    assert!(!status_has_v6(&placeholder), "无前缀残值不能算拿到了 v6");
}
