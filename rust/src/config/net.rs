//! `[net]` 段：WAN 拨号与 IPv6 开关。

use serde::Deserialize;

/// 强制下发的默认 DNS（**顺序即优先级**：IPv4 首选 / IPv4 备选 / IPv6 首选 / IPv6 备选）。
///
/// | 族 | 首选 | 备选 |
/// | --- | --- | --- |
/// | IPv4 | `119.29.29.29`（DNSPod） | `223.5.5.5`（阿里） |
/// | IPv6 | `2402:4e00::`（DNSPod） | `2400:3200::1`（阿里） |
///
/// 选这两家而不是海外公共 DNS：蜂窝链路的出口在国内，解析延迟与劫持风险都更低；
/// 两家都提供 Anycast 的 v4 + v6 双栈，任何一族不可用都有同族的备选顶上。
///
/// ⚠️ 改这里要同步三处：本常量、`files/etc/modemd/config.toml` 的 `[net] wan_dns`
/// 注释、以及 `docs/DIAL_STATE_MACHINE.md` 里提到默认值的地方。
/// 老机器升级后配置文件里没有这一行也走本常量（`config.toml` 是 conffile，升级不覆盖）。
pub const DEFAULT_WAN_DNS: &str = "119.29.29.29 223.5.5.5 2402:4e00:: 2400:3200::1";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NetConfig {
    pub auto_wan: bool,
    pub auto_dial: bool,
    pub wan_mode: String,
    pub host_dial: bool,
    pub host_dial_bin: String,
    pub host_dial_pdn: i64,
    pub wan_ipv6: bool,
    /// 模组侧 IPv6 的取址方式（`dhcpv6` / `ra` / `static` / `off`）。
    ///
    /// 默认 `dhcpv6`：交给 netifd 的 odhcp6c（用户态收 RA，不吃 `accept_ra` 的坑）
    /// 并把 `/64` 前缀委派给 LAN。口径见 `netprov/v6.rs`，对齐 `luci-app-fm350`。
    /// 认不出的值按 `dhcpv6` 处理（配置写错不该把用户带进一条不通的路径）。
    pub wan_ipv6_mode: String,
    pub lan_ipv6: bool,
    pub wan_metric: i64,
    pub wan_zone: String,
    /// 强制下发到 WAN 接口的 DNS（空格分隔；默认见 [`DEFAULT_WAN_DNS`]）。
    ///
    /// 蜂窝链路 DHCP 常拿不到 DNS（FM350 系列 RNDIS 尤其如此），导致
    /// `resolv.conf.auto` 为空、域名解析全 REFUSED。插件组网时把它写进
    /// `network.modem<id>.dns`，netifd 拉起接口即经 `interface_add_dns_server`
    /// 下发到 resolv.conf.auto（netifd 这一个入口同时认 v4 与 v6 地址）。
    /// 取值 `off` / `none` / `carrier` = 不强制，用运营商/DHCP DNS。
    ///
    /// ⚠️ IPv6 那两条只在 `[net] wan_ipv6` 打开时才下发 —— 链路没有 IPv6
    /// 时写进 resolv.conf 只会多出两个永远不回应的 nameserver。
    pub wan_dns: String,
    pub dial_enabled: bool,
    pub allow_roaming: bool,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            auto_wan: true,
            auto_dial: true,
            wan_mode: "modem_pro".to_string(),
            host_dial: true,
            host_dial_bin: "native".to_string(),
            host_dial_pdn: 0,
            wan_ipv6: true,
            wan_ipv6_mode: "dhcpv6".to_string(),
            lan_ipv6: true,
            wan_metric: 20,
            wan_zone: "wan".to_string(),
            wan_dns: DEFAULT_WAN_DNS.to_string(),
            dial_enabled: true,
            allow_roaming: true,
        }
    }
}

impl NetConfig {
    pub(super) fn normalize(&mut self) {
        if self.wan_metric <= 0 {
            self.wan_metric = 20;
        }
        if self.wan_zone.trim().is_empty() {
            self.wan_zone = "wan".to_string();
        }
        if self.wan_mode.trim().is_empty() {
            self.wan_mode = "modem_pro".to_string();
        }
        // 取址方式：空值/拼错统一归一到 dhcpv6（与 netprov::v6::V6Mode::parse 同口径，
        // 这里先落成规范字符串，配置读回时一眼能看出实际生效的是哪个）。
        self.wan_ipv6_mode = crate::netprov::v6::V6Mode::parse(&self.wan_ipv6_mode)
            .as_str()
            .to_string();
        // 拨号后端只剩内置一种：空值（旧配置的默认）统一落成 "native"，
        // 这样 `/api/config` 与日志里看到的就是实际生效值。
        if self.host_dial_bin.trim().is_empty() {
            self.host_dial_bin = "native".to_string();
        }
        // DNS 强制下发：空值回落默认（v4 首选/备选 + v6 首选/备选，见 DEFAULT_WAN_DNS）；
        // off / none / carrier = 不强制，用运营商或 DHCP 给的 DNS。
        match self.wan_dns.trim().to_ascii_lowercase().as_str() {
            "" => self.wan_dns = DEFAULT_WAN_DNS.to_string(),
            "off" | "none" | "carrier" => self.wan_dns = String::new(),
            _ => self.wan_dns = self.wan_dns.trim().to_string(),
        }
    }
}
