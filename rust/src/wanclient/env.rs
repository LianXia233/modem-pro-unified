//! `modemd wan-client` 的入参：全部来自 netifd 经 `proto_export` 注入的环境变量。
//!
//! 变量名与含义对旧 `mp-client.sh` 保持不变（`INTERFACE` / `MP_IFACE` / `MP_UOPTS` /
//! `MP_OOPTS` / `MP_NO_V6`），只新增 `MP_FIX_ACCEPT_RA`：把「是否要把网卡 accept_ra
//! 置为 2」这个已由 proto handler 判定好的结论传进来，避免在 Rust 里重新解读 uci。

/// 旧脚本里的初始值：`IFACE="${MP_IFACE:-${1:-}}"`。
pub const ARG_IFACE_INDEX: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    /// netifd 逻辑接口名（`$INTERFACE`），上报脚本名里带的也是它。
    pub interface: String,
    /// 实际网卡名（`$MP_IFACE`），如 `wwan0`。
    pub iface: String,
    /// udhcpc 的附加参数（已按 shell 空白切分）。
    pub uopts: Vec<String>,
    /// odhcp6c 的附加参数（已按 shell 空白切分）。
    pub oopts: Vec<String>,
    /// `mp_no_v6=1`：本接口只跑 udhcpc，IPv6 由独立的兄弟接口负责。
    pub no_v6: bool,
    /// proto handler 判定「需要把 accept_ra 置 2」。
    pub fix_accept_ra: bool,
}

impl Params {
    /// 从当前进程环境 + 命令行读取。
    pub fn from_process() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().collect();
        let arg_iface = args.get(ARG_IFACE_INDEX).cloned();
        Self::from_pairs(|k| std::env::var(k).ok(), arg_iface.as_deref())
    }

    /// 纯函数版本（便于单测，不碰进程环境）。
    pub fn from_pairs<F>(get: F, arg_iface: Option<&str>) -> Result<Self, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let interface = get("INTERFACE").unwrap_or_default();
        if interface.is_empty() {
            return Err("缺少 INTERFACE 环境变量".to_string());
        }

        let iface = match get("MP_IFACE") {
            Some(v) if !v.is_empty() => v,
            _ => arg_iface.unwrap_or_default().to_string(),
        };
        if iface.is_empty() {
            return Err("缺少接口名".to_string());
        }

        Ok(Self {
            interface,
            iface,
            uopts: split_opts(get("MP_UOPTS").unwrap_or_default().as_str()),
            oopts: split_opts(get("MP_OOPTS").unwrap_or_default().as_str()),
            no_v6: get("MP_NO_V6").as_deref() == Some("1"),
            fix_accept_ra: get("MP_FIX_ACCEPT_RA").as_deref() == Some("1"),
        })
    }

    /// `/tmp/mp-dhcp-<INTERFACE>.script`（v4 上报脚本，路径与旧 shim 一致）。
    pub fn v4_script_path(&self) -> String {
        format!("/tmp/mp-dhcp-{}.script", self.interface)
    }

    /// `/tmp/mp-dhcpv6-<INTERFACE>.script`（v6 上报脚本，路径与旧 shim 一致）。
    pub fn v6_script_path(&self) -> String {
        format!("/tmp/mp-dhcpv6-{}.script", self.interface)
    }

    /// `/var/run/mp-udhcpc-<INTERFACE>.pid`（与旧 shim 一致）。
    pub fn udhcpc_pidfile(&self) -> String {
        format!("/var/run/mp-udhcpc-{}.pid", self.interface)
    }
}

/// 按 shell 的「无引号变量展开」语义切分空白：连续空白不产生空参数。
pub fn split_opts(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn params(pairs: &[(&str, &str)], arg: Option<&str>) -> Result<Params, String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Params::from_pairs(|k| map.get(k).cloned(), arg)
    }

    #[test]
    fn missing_interface_rejected() {
        assert!(params(&[("MP_IFACE", "wwan0")], None).is_err());
    }

    #[test]
    fn missing_iface_rejected() {
        assert!(params(&[("INTERFACE", "modem1")], None).is_err());
    }

    #[test]
    fn iface_falls_back_to_arg() {
        let p = params(&[("INTERFACE", "modem1")], Some("wwan0")).unwrap();
        assert_eq!(p.iface, "wwan0");
    }

    #[test]
    fn env_iface_wins_over_arg() {
        let p = params(
            &[("INTERFACE", "modem1"), ("MP_IFACE", "wwan0")],
            Some("wwan1"),
        )
        .unwrap();
        assert_eq!(p.iface, "wwan0");
    }

    #[test]
    fn opts_are_split_like_shell() {
        assert_eq!(
            split_opts(" -B -x hostname:router -O 121 "),
            vec!["-B", "-x", "hostname:router", "-O", "121"]
        );
        assert_eq!(split_opts(""), Vec::<String>::new());
        assert!(split_opts("   ").is_empty());
        assert_eq!(split_opts("-Nforce -t120"), vec!["-Nforce", "-t120"]);
    }

    #[test]
    fn flags_and_paths() {
        let p = params(
            &[
                ("INTERFACE", "modem1"),
                ("MP_IFACE", "wwan0"),
                ("MP_NO_V6", "1"),
                ("MP_FIX_ACCEPT_RA", "1"),
            ],
            None,
        )
        .unwrap();
        assert!(p.no_v6);
        assert!(p.fix_accept_ra);
        assert_eq!(p.v4_script_path(), "/tmp/mp-dhcp-modem1.script");
        assert_eq!(p.v6_script_path(), "/tmp/mp-dhcpv6-modem1.script");
        assert_eq!(p.udhcpc_pidfile(), "/var/run/mp-udhcpc-modem1.pid");
    }

    #[test]
    fn flags_default_off() {
        let p = params(&[("INTERFACE", "modem1"), ("MP_IFACE", "wwan0")], None).unwrap();
        assert!(!p.no_v6);
        assert!(!p.fix_accept_ra);
        assert!(p.uopts.is_empty());
        assert!(p.oopts.is_empty());
    }
}
