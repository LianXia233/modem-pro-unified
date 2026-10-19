//! WAN 接口形态（`[net] wan_mode`）：三种模式（纯策略，不碰 UCI / 不发 AT / 不读环境）。
//!
//! WAN 模式的取值与落盘口径：
//!
//!   · `modem_pro` —— 只用一张接口（proto=modem_pro），IPv4+IPv6 都在上面。**默认**。
//!   · `dhcp`      —— 传统两张：`modem<id>`(dhcp) + `modem<id>_6`(dhcpv6)。老行为。
//!   · `both`      —— 两种都要：`modem<id>`(modem_pro) + `modem<id>_6`(dhcpv6)。
//!
//! ★ 「不分协议」：模式 1/3 下**不再只认 QMI** —— qmi / ecm / ncm / rndis / mbim
//! 以及认不出的类型一律用 modem_pro。MBIM 也**不再例外**（2026-10-13 起）：
//! 它的会话建立由 modemd 内置的 MBIM 拨号做，接口跟其它类型一样是一张
//! `modem<id>`；只有 `host_dial` 关掉时才退回 `proto=mbim`
//! （见 `netprov::main_proto`）。
//!
//! ⚠️ 本文件只放**纯策略**：真正的建接口 / 改接口在 `netprov/`（纯策略 + UCI 薄 IO
//!    + 编排），由 `http/loops.rs::spawn_netprov` 每 15 秒驱动一轮。

use serde::Serialize;

/// 自动组网生成的 WAN 接口形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WanMode {
    /// 只用一张接口（proto=modem_pro），v4+v6 都在它上面。**默认**。
    #[default]
    ModemPro,
    /// 传统两张：modem<id>(dhcp) + modem<id>_6(dhcpv6)。
    Dhcp,
    /// 两种都要：modem<id>(modem_pro) + modem<id>_6(dhcpv6)。
    Both,
}

impl WanMode {
    /// 三种模式，顺序就是界面上的显示顺序（默认的排第一）。
    pub const ALL: [WanMode; 3] = [WanMode::ModemPro, WanMode::Dhcp, WanMode::Both];

    /// 把配置里的字符串解析成模式。
    ///
    /// 认不出的值（含空串、写错）一律回落到**默认 ModemPro** ——
    /// 配置写错不该让用户的网络变成另一种形态。顺带接受几个好记的别名。
    pub fn parse(s: &str) -> WanMode {
        match s.trim().to_lowercase().as_str() {
            "dhcp" | "dhcp_v4v6" | "dhcpv4v6" | "dhcpv6" | "v4v6" | "v6" | "2" | "two" => {
                WanMode::Dhcp
            }
            "both" | "all" | "both_modes" | "modem_pro_dhcp" | "modem_pro+dhcp" | "3" | "three" => {
                WanMode::Both
            }
            "modem_pro" | "modempro" | "modem-pro" | "pro" | "1" | "one" => WanMode::ModemPro,
            _ => WanMode::ModemPro,
        }
    }

    /// 归一化后的配置值（写回界面 / 日志用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            WanMode::ModemPro => "modem_pro",
            WanMode::Dhcp => "dhcp",
            WanMode::Both => "both",
        }
    }

    /// 主接口（modem<id>）是不是用 modem_pro 协议 —— 模式 1 / 3。
    pub fn uses_modem_pro(&self) -> bool {
        matches!(self, WanMode::ModemPro | WanMode::Both)
    }

    /// 要不要另建 modem<id>_6（proto=dhcpv6）兄弟接口 —— 模式 2 / 3。
    pub fn wants_dhcpv6_sibling(&self) -> bool {
        matches!(self, WanMode::Dhcp | WanMode::Both)
    }

    /// 中文说明（日志、界面提示用）。
    pub fn label(&self) -> &'static str {
        match self {
            WanMode::Dhcp => "DHCP V4 和 V6（两张接口）",
            WanMode::Both => "Modem-Pro + DHCP V4 和 V6（两种都要）",
            WanMode::ModemPro => "Modem-Pro（单接口，推荐）",
        }
    }

    /// 三种模式的取值与中文说明（给页面渲染下拉框用）。
    pub fn list_payload() -> Vec<serde_json::Value> {
        Self::ALL
            .iter()
            .map(|m| {
                serde_json::json!({
                    "value": m.as_str(),
                    "label": m.label(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::WanMode;

    #[test]
    fn parse_known_and_aliases() {
        assert_eq!(WanMode::parse("modem_pro"), WanMode::ModemPro);
        assert_eq!(WanMode::parse("dhcp"), WanMode::Dhcp);
        assert_eq!(WanMode::parse("both"), WanMode::Both);
        assert_eq!(WanMode::parse(" MODEM_PRO "), WanMode::ModemPro);
        assert_eq!(WanMode::parse("dhcpv6"), WanMode::Dhcp);
        assert_eq!(WanMode::parse("modem-pro"), WanMode::ModemPro);
        assert_eq!(WanMode::parse("2"), WanMode::Dhcp);
        assert_eq!(WanMode::parse("3"), WanMode::Both);
    }

    #[test]
    fn unknown_falls_back_to_default() {
        assert_eq!(WanMode::parse(""), WanMode::ModemPro);
        assert_eq!(WanMode::parse("garbage"), WanMode::ModemPro);
    }

    #[test]
    fn mode_flags() {
        assert!(WanMode::ModemPro.uses_modem_pro());
        assert!(WanMode::Both.uses_modem_pro());
        assert!(!WanMode::Dhcp.uses_modem_pro());
        assert!(WanMode::Dhcp.wants_dhcpv6_sibling());
        assert!(WanMode::Both.wants_dhcpv6_sibling());
        assert!(!WanMode::ModemPro.wants_dhcpv6_sibling());
        assert_eq!(WanMode::default(), WanMode::ModemPro);
    }

    #[test]
    fn list_payload_has_three_in_order() {
        let list = WanMode::list_payload();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0]["value"], "modem_pro");
        assert_eq!(list[1]["value"], "dhcp");
        assert_eq!(list[2]["value"], "both");
        assert_eq!(list[0]["label"], "Modem-Pro（单接口，推荐）");
    }
}
