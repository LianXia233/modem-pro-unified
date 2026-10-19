//! USB 端口规则（逐 `VID:PID` 的接口号约定）。
//!
//! ## 数据来源
//!
//! 整表取自 QModem 的 `modem_port_rule.json`（github.com/FUjr/QModem，MPL-2.0），
//! 语义与其 `modem_scand.c` 一致：
//!
//! - `at_interfaces`：**允许**当 AT / 数据口的 USB 接口号（`1.2` = 配置 1 的第 2 个接口）。
//!   规则里给了列表就只认这些口，其余接口（diag / adb / 网卡）一律跳过；
//! - `voice_pcm_interface`：语音 PCM 走哪个接口（本项目暂无语音功能，只作为
//!   「这台设备有几个串口、哪个不是 AT」的判据保留）；
//! - `option_driver`：Vendor 专属 PID 需要往 `option1/new_id` 写一次才会出 tty。
//!
//! ## 为什么值得单独一张表
//!
//! `modem_catalog.json` 只有「VID → 厂商」，同一个 VID 底下几十个 PID、型号各不同；
//! 而 AT 口选错（选到 diag 口）的典型症状是「能打开、发 AT 全部超时」。
//! QModem 用这 18 条例外把「某 PID 的哪个接口是 AT」钉死，比通用探测快且准。
//!
//! 表很小（18 条），所以直接内嵌进二进制，运行时不去 `/usr/share` 找文件。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// 内嵌规则表。
const PORTS_JSON: &str = include_str!("modem_ports.json");

/// 一条 `VID:PID` 的端口约定。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PortRule {
    pub vid: String,
    pub pid: String,
    /// 规则里写的型号（可能与清单不同名，如 `TDTECH MH5000-82M` / `gm800`）。
    #[serde(default)]
    pub model: String,
    /// 这个 PID 对应的数据拨号方式（`qmi` / `ecm` / `rndis` / `mbim` / 空）。
    #[serde(default)]
    pub mode: String,
    /// 需要先绑 option 驱动才会出 tty。
    #[serde(default)]
    pub option_driver: bool,
    /// 允许作为 AT / 数据口的接口号（空 = 不限制）。
    #[serde(default)]
    pub at_interfaces: Vec<String>,
    /// 语音 PCM 接口号（空 = 无）。
    #[serde(default)]
    pub voice_pcm_interface: String,
}

impl PortRule {
    /// 这个接口号能不能当 AT 口。
    ///
    /// 没给 `at_interfaces` 时返回 `true`（规则不限制），与 QModem 的
    /// `interface_allowed()` 一致。
    pub fn allows_interface(&self, iface: &str) -> bool {
        if self.at_interfaces.is_empty() {
            return true;
        }
        self.at_interfaces.iter().any(|i| i == iface)
    }

    /// `"1.2"` 这种接口号的配置号部分（`1`）。
    pub fn config_of(iface: &str) -> &str {
        iface.split('.').next().unwrap_or(iface)
    }
}

fn rules() -> &'static Vec<PortRule> {
    static CELL: OnceLock<Vec<PortRule>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(PORTS_JSON).unwrap_or_else(|e| {
            log::error!("端口规则解析失败: {e}");
            Vec::new()
        })
    })
}

/// 全量规则（只读）。
pub fn all() -> &'static [PortRule] {
    rules().as_slice()
}

/// 按 `VID:PID` 查规则；两者都按 4 位小写十六进制比较。
pub fn lookup(vid: &str, pid: &str) -> Option<&'static PortRule> {
    let v = vid.trim().to_ascii_lowercase();
    let p = pid.trim().to_ascii_lowercase();
    if v.is_empty() || p.is_empty() {
        return None;
    }
    rules().iter().find(|r| r.vid == v && r.pid == p)
}

/// 同一 `VID:PID` 可能有多条（不同 `mode`，如 gm800 的 qmi/rndis）。
pub fn lookup_all(vid: &str, pid: &str) -> Vec<&'static PortRule> {
    let v = vid.trim().to_ascii_lowercase();
    let p = pid.trim().to_ascii_lowercase();
    rules()
        .iter()
        .filter(|r| r.vid == v && r.pid == p)
        .collect()
}

/// 规则条数（诊断/统计用）。
pub fn count() -> usize {
    rules().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_the_qmodem_port_rules() {
        assert_eq!(count(), 18, "QModem modem_port_rule.json 共 18 条 USB 规则");
        let all = all();
        assert!(all.iter().all(|r| r.vid.len() == 4 && r.pid.len() == 4));
        // 排序稳定：vid 升序
        let mut sorted = all.to_vec();
        sorted.sort_by(|a, b| a.vid.cmp(&b.vid));
        assert_eq!(all[0].vid, sorted[0].vid);
    }

    #[test]
    fn rm520n_at_interfaces_are_pinned() {
        let r = lookup("2c7c", "0801").expect("rm520n-gl 规则");
        assert_eq!(r.model, "rm520n-gl");
        assert_eq!(r.at_interfaces, vec!["1.2", "1.3", "1.4"]);
        assert_eq!(r.voice_pcm_interface, "1.1", "语音口与 AT 口必须分开");
        assert!(r.allows_interface("1.2"));
        assert!(r.allows_interface("1.3"));
        assert!(!r.allows_interface("1.1"), "1.1 是语音口，不是 AT 口");
        assert!(!r.allows_interface("1.0"));
        assert_eq!(PortRule::config_of("1.2"), "1");
    }

    #[test]
    fn same_pid_can_have_several_modes() {
        let list = lookup_all("305a", "1421");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].mode, "qmi");
        let list = lookup_all("2c7c", "0801");
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn lookup_is_case_insensitive_and_never_guesses() {
        assert!(lookup("2C7C", "0801").is_some());
        assert!(lookup("2c7c", "ffff").is_none());
        assert!(lookup("", "0801").is_none());
        assert!(lookup("2c7c", "").is_none());
    }

    #[test]
    fn empty_interface_list_means_no_restriction() {
        let r = PortRule {
            vid: "0000".into(),
            pid: "0000".into(),
            model: String::new(),
            mode: String::new(),
            option_driver: false,
            at_interfaces: Vec::new(),
            voice_pcm_interface: String::new(),
        };
        assert!(r.allows_interface("1.0"));
        assert!(r.allows_interface("2.9"));
    }
}
