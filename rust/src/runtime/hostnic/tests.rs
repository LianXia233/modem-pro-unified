//! hostnic 纯逻辑单测（枚举与执行要真机 / 真命令，不在覆盖范围）。

use super::*;

fn plan() -> HostNicPlan {
    HostNicPlan {
        iface: "wwan0".into(),
        ip: "10.178.25.2".into(),
        mask: "255.255.255.0".into(),
        gateway: "10.178.25.161".into(),
        dns: vec!["114.114.114.114".into()],
    }
}

#[test]
fn only_fm350_needs_manual_nic() {
    let (need, why) = quirk("FM350-GL");
    assert!(need);
    assert!(why.contains("169.254"));
    // 判据用 contains：调用方可能直接给 ATI 原文
    assert!(quirk("ATI|Manufacturer: Fibocom|Model: FM350-GL|Revision: x").0);
    // 350 系列的另一个型号：RW350 与 FM350 同构，必须同样命中
    assert!(quirk("RW350-CN").0);
    assert!(quirk("ATI|Manufacturer: Fibocom|Model: RW350|Revision: x").0);
    // 正常机型不该被要求手工配（ECM/NCM/MBIM/QMI 的地址有别的来源）
    assert!(!quirk("RM520N-GL").0);
    assert!(!quirk("FM160-CN").0);
    assert!(!quirk("").0);
}

#[test]
fn mask_and_prefix_roundtrip() {
    assert_eq!(prefix_from_mask("255.255.255.0"), Some(24));
    assert_eq!(prefix_from_mask("24"), Some(24));
    assert_eq!(prefix_from_mask("0"), Some(0));
    assert_eq!(prefix_from_mask("255.255.0.0"), Some(16));
    assert_eq!(mask_from_prefix(24).as_deref(), Some("255.255.255.0"));
    assert_eq!(mask_from_prefix(0).as_deref(), Some("0.0.0.0"));
    assert_eq!(mask_from_prefix(32).as_deref(), Some("255.255.255.255"));
    // 非法：空 / 越界 / 非连续掩码 / 不是 IPv4
    assert_eq!(prefix_from_mask(""), None);
    assert_eq!(prefix_from_mask("33"), None);
    assert_eq!(prefix_from_mask("255.0.255.0"), None);
    assert_eq!(prefix_from_mask("abc"), None);
    assert_eq!(mask_from_prefix(33), None);
}

#[test]
fn validate_rejects_broken_plans() {
    assert!(validate_plan(&plan()).is_ok());
    let mut p = plan();
    p.iface = " ".into();
    assert_eq!(validate_plan(&p).unwrap_err().to_string(), "先选一张网卡");
    let mut p = plan();
    p.ip = "10.0.0.256".into();
    assert!(validate_plan(&p)
        .unwrap_err()
        .to_string()
        .contains("IP 不是合法"));
    let mut p = plan();
    p.mask = "24 或 255.255.255.0 之外的写法".into();
    assert!(validate_plan(&p).unwrap_err().to_string().contains("掩码"));
    let mut p = plan();
    p.gateway = "not-an-ip".into();
    assert!(validate_plan(&p).unwrap_err().to_string().contains("网关"));
    let mut p = plan();
    p.dns = vec!["dns.example".into()];
    assert!(validate_plan(&p).unwrap_err().to_string().contains("DNS"));
    // 网关留空是允许的（netsh 允许不带网关）
    let mut p = plan();
    p.gateway.clear();
    assert!(validate_plan(&p).is_ok());
}

#[test]
fn normalize_plan_cleans_up() {
    let mut dirty = plan();
    dirty.iface = "  wwan0  ".into();
    dirty.mask = "24".into();
    dirty.dns = vec![
        " 114.114.114.114 ".into(),
        "".into(),
        "114.114.114.114".into(),
    ];
    let p = normalize_plan(&dirty);
    assert_eq!(p.iface, "wwan0");
    assert_eq!(p.mask, "255.255.255.0");
    assert_eq!(p.dns, vec!["114.114.114.114".to_string()]);
}

#[test]
fn suggest_rides_alongside_the_modem() {
    let s = suggest_plan("10.178.25.161").unwrap();
    assert_eq!(s.ip, "10.178.25.2");
    assert_eq!(s.gateway, "10.178.25.161");
    assert_eq!(s.mask, "255.255.255.0");
    // 模组自己就是 .2 时往后挪一个，别和自己撞
    let s = suggest_plan("10.178.25.2").unwrap();
    assert_eq!(s.ip, "10.178.25.3");
    assert!(suggest_plan("").is_none());
    assert!(suggest_plan("not-an-ip").is_none());
}

#[test]
fn static_commands_on_plain_linux() {
    let cs = commands(MODE_STATIC, &plan(), "linux");
    let texts: Vec<&str> = cs.iter().map(|c| c.text.as_str()).collect();
    // flush → add（带前缀）→ up → 默认路由（replace）→ DNS 写 resolv.conf
    assert!(texts.iter().any(|t| t.contains("ip addr flush dev wwan0")));
    assert!(texts
        .iter()
        .any(|t| t.contains("ip addr add 10.178.25.2/24 dev wwan0")));
    assert!(texts.iter().any(|t| t.contains("ip link set wwan0 up")));
    assert!(texts
        .iter()
        .any(|t| t.contains("ip route replace default via 10.178.25.161 dev wwan0")));
    assert!(texts
        .iter()
        .any(|t| t.contains("nameserver 114.114.114.114")));
    assert!(cs.iter().all(|c| c.admin_only));
    assert!(cs.iter().all(|c| !c.manual));
}

#[test]
fn openwrt_does_not_touch_resolv_conf() {
    // OpenWrt 的 DNS 由 dnsmasq 管：运行期不写 /etc/resolv.conf，改由 UCI 块给
    let cs = commands(MODE_STATIC, &plan(), "openwrt");
    assert!(cs.iter().any(|c| c.text.contains("ip addr add")));
    assert!(!cs.iter().any(|c| c.text.contains("resolv.conf")));
}

#[test]
fn renew_and_dhcp_commands() {
    let cs = commands(MODE_RENEW, &plan(), "linux");
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].prog, "udhcpc");
    assert_eq!(cs[0].timeout, Some(RENEW_TIMEOUT));

    let cs = commands(MODE_DHCP, &plan(), "linux");
    assert_eq!(cs.len(), 2);
    assert!(cs[0].text.contains("ip addr flush"));
    assert_eq!(cs[1].prog, "udhcpc");

    // 认不出的平台不乱编命令
    assert!(commands(MODE_STATIC, &plan(), "darwin").is_empty());
    assert!(commands("no-such-mode", &plan(), "linux").is_empty());
}

#[test]
fn windows_commands_use_netsh_syntax() {
    let cs = commands(MODE_STATIC, &plan(), "windows");
    // 展示用 netsh 自己的引号语法，执行走 argv（整个 name=xxx 是一个参数）
    assert!(cs[0].text.contains("netsh interface ip set address name=\"wwan0\" static 10.178.25.2 255.255.255.0 10.178.25.161"));
    assert!(cs[0].args.iter().any(|a| a == "name=wwan0"));
    assert!(cs.iter().any(|c| c
        .text
        .contains("netsh interface ip set dns name=\"wwan0\" static 114.114.114.114")));
}

#[test]
fn persist_only_on_openwrt_and_never_executed() {
    assert!(persist_commands(&plan(), "linux").is_empty());
    assert!(persist_commands(&plan(), "windows").is_empty());
    let cs = persist_commands(&plan(), "openwrt");
    assert!(!cs.is_empty());
    // 全部只显示不执行：一键改 /etc/config/network 风险太大
    assert!(cs.iter().all(|c| c.manual && !c.admin_only));
    let texts: Vec<&str> = cs.iter().map(|c| c.text.as_str()).collect();
    assert!(texts
        .iter()
        .any(|t| t.contains("uci set network.modem=interface")));
    assert!(texts
        .iter()
        .any(|t| t.contains("uci set network.modem.proto=static")));
    assert!(texts
        .iter()
        .any(|t| t.contains("network.modem.device=wwan0")));
    assert!(texts
        .iter()
        .any(|t| t.contains("network.modem.ipaddr=10.178.25.2")));
    assert!(texts.iter().any(|t| t.contains("uci commit network")));
    assert!(texts
        .iter()
        .any(|t| t.contains("/etc/init.d/network reload")));
}

#[test]
fn action_sets_cover_three_modes() {
    let sets = action_sets(&plan(), "openwrt");
    assert_eq!(sets.len(), 3);
    assert_eq!(sets[0].mode, MODE_RENEW);
    assert_eq!(sets[1].mode, MODE_STATIC);
    assert_eq!(sets[2].mode, MODE_DHCP);
    assert!(!sets[0].hint.is_empty());
    assert_eq!(
        sets[1].commands.len(),
        commands(MODE_STATIC, &plan(), "openwrt").len()
    );
}

#[test]
fn ip_addr_output_parsing() {
    let sample = "\
1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536
    inet 127.0.0.1/8 scope host lo
2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500
    link/ether 00:11:22:33:44:55 brd ff:ff:ff:ff:ff:ff
    inet 192.168.1.1/24 brd 192.168.1.255 scope global eth0
3: wwan0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500
    inet 169.254.3.7/16 brd 169.254.255.255 scope global wwan0
";
    let got = parse_ip_addr(sample);
    assert_eq!(got.len(), 3);
    assert_eq!(got[1].name, "eth0");
    assert!(got[1].up);
    assert_eq!(got[1].addrs, vec!["192.168.1.1/24".to_string()]);
    assert_eq!(got[2].name, "wwan0");
    assert_eq!(got[2].addrs, vec!["169.254.3.7/16".to_string()]);
}

#[test]
fn iface_listing_filters_and_flags() {
    // 直接跑 interfaces() 依赖本机，改为验证两个判定函数的口径
    assert!(is_virtual_iface("lo"));
    assert!(is_virtual_iface("docker0"));
    assert!(is_virtual_iface("br-lan"));
    // OpenWrt 的无线口（实机上见到的是 phy0-ap0 / phy1-ap0）
    assert!(is_virtual_iface("phy0-ap0"));
    assert!(!is_virtual_iface("wwan0"));
    assert!(!is_virtual_iface("eth1"));
    assert!(is_strong_iface("wwan0"));
    assert!(is_strong_iface("usb0"));
    assert!(!is_strong_iface("eth0"));
}

#[test]
fn hostnic_store_roundtrip() {
    let dir = std::env::temp_dir().join(format!("modemd-hostnic-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("hostnic.json").to_string_lossy().to_string();
    let _ = std::fs::remove_file(&path);

    let store = HostNicStore::new(path.clone());
    assert!(store.names().is_empty());
    // 校验不通过不落盘
    let mut bad = plan();
    bad.ip = "999.1.1.1".into();
    assert!(store.save(&bad).is_err());
    assert!(store.names().is_empty());

    let saved = store.save(&plan()).unwrap();
    assert_eq!(saved.mask, "255.255.255.0");
    assert_eq!(store.names(), vec!["wwan0".to_string()]);
    assert_eq!(store.get("wwan0").unwrap().gateway, "10.178.25.161");

    // 跨"重启"读回（归一化后的值）
    let reloaded = HostNicStore::new(path.clone());
    assert_eq!(reloaded.get("wwan0").unwrap().ip, "10.178.25.2");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostnic_file_sits_next_to_config() {
    assert_eq!(
        hostnic_file("/etc/modemd/config.toml"),
        "/etc/modemd/hostnic.json"
    );
    assert_eq!(hostnic_file(""), "hostnic.json");
}

#[test]
fn data_dev_never_picks_onboard_or_occupied() {
    // 实机（FM350 + MT798x）：eth0=LAN、eth1=WAN 都是板载 mtk_soc_eth，
    // 数据口是 eth2（rndis_host）。这条断言保证排除逻辑不退化。
    assert!(!is_strong_iface("eth0"));
    assert!(!is_strong_iface("eth1"));
    assert!(is_strong_iface("wwan0"));
    assert!(is_strong_iface("usb0"));
    // 板载判定走驱动：本机没网卡时按"驱动为空 ⇒ 不算板载"降级
    assert!(!is_onboard_iface("这个网卡不存在"));

    // 全局兜底：驱动命中优先，且绝不能返回被占用 / 被排除的那张
    let excluded = vec!["eth2".to_string()];
    let got = detect_data_dev(&excluded);
    if let Some(d) = got {
        assert!(!excluded.contains(&d), "不该返回被排除的网卡 {d}");
        assert!(
            !is_onboard_iface(&d) || iface_driver(&d).is_empty(),
            "{d} 是板载口"
        );
    }
}

#[test]
fn driver_and_vid_helpers_degrade_gracefully() {
    // 不存在的网卡：驱动与 VID 都应该是空 / None，不能 panic
    assert_eq!(iface_driver("no-such-iface"), "");
    assert_eq!(iface_usb_vid("no-such-iface"), None);
    // 数据驱动清单与 FM350 参考实现一致（不含 ccmni：主机侧不存在）
    assert!(DATA_DRIVERS.contains(&"rndis_host"));
    assert!(DATA_DRIVERS.contains(&"cdc_ether"));
    assert!(DATA_DRIVERS.contains(&"cdc_ncm"));
    assert!(DATA_DRIVERS.contains(&"qmi_wwan"));
    assert!(!DATA_DRIVERS.contains(&"ccmni"));
}
