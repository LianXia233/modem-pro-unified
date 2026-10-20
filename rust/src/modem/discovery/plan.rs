//! 端口probe计划：**按物理设备分组**、按 QModem 端口规则筛选、每台设备只选一个命令口。
//!
//! ## 解决的问题
//!
//! 早先的发现流程是「枚举所有 ttyUSB/ttyACM → 逐个发 AT → 谁回 OK 就登记成一台模组」，
//! 在真实设备上会出三类错误：
//!
//! 1. **同一台模组被登记多次**：RM520N-GL 有 4 个 tty（DM/NMEA/AT/AT），
//!    AT 探活对其中两个都能回 OK，列表里就冒出两台「模组」，抢同一块网卡；
//! 2. **探到 diag / NMEA 口**：这些口可能对 `AT` 回 OK（或回 URC），但真指令全超时；
//! 3. **探到语音 PCM 口**：`2c7c:0801` 的 `1.1` 是 PCM，打开后既不是 AT 口，
//!    还会把语音通道占住。
//!
//! QModem 的 `modem_scand.c:563-690` 用「USB 设备 → 接口号白名单 → 精确匹配
//! 完整接口串（`1.2`）」的方式解决；上游 Go 版则用 `GroupByUSBDevice`
//! （`module/netdev/ports_unix.go:112-148`）+ 命令口四级优先级
//! （`module/kernel/modem_discover.go:140-169`）。
//!
//! 本模块把两者的判定抽成**纯函数**：输入是「端口快照」，输出是「每组选谁、
//! 跳过谁、为什么」。这样不碰 sysfs 也能把规则测透，真机侧只负责填快照。
//!
//! ## 判定顺序（与上游一致）
//!
//! 1. 按 USB 设备（`1-1`）分组；拿不到 USB 拓扑的（MHI / 纯串口）各自成组；
//! 2. 命中端口规则时：接口号必须在 `include` 里，语音 PCM 接口排除；
//! 3. 组内排序：`include` 里的顺序 → 驱动更像 AT 口 → 端口号（自然序）；
//! 4. 首选第一个，其余留作**回退**（首选探不通时才试），不并排登记。

use crate::modem::ports;

/// 一个候选串口的快照（真机数据来自 sysfs，测试里直接构造）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortInfo {
    /// `/dev/ttyUSB2`。
    pub path: String,
    /// USB 设备目录名（`1-1`）；MHI / 纯串口为空。
    pub usb_dev: String,
    /// 配置.接口号（`1.2`）；非 USB 为空。
    pub iface: String,
    /// 绑定的驱动名（`option` / `qcserial` / `cdc_acm` / `diag`…）。
    pub driver: String,
    /// 4 位小写十六进制（USB 设备身份）。
    pub vid: String,
    /// 4 位小写十六进制。
    pub pid: String,
}

impl PortInfo {
    fn group_key(&self) -> String {
        if self.usb_dev.trim().is_empty() {
            format!("port:{}", self.path)
        } else {
            format!("usb:{}", self.usb_dev)
        }
    }
}

/// 一组（一台物理设备）的挑选结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// USB 设备名，或 `port:<路径>`。
    pub key: String,
    pub vid: String,
    pub pid: String,
    /// 首选命令口。
    pub primary: String,
    /// 首选失败时按顺序再试的口。
    pub fallbacks: Vec<String>,
    /// 被规则排除的口（路径 + 原因），用于日志/诊断。
    pub skipped: Vec<(String, String)>,
    /// 命中的规则说明（没命中规则时为空）。
    pub rule_note: String,
}

/// 驱动是否「像 AT 口」。
///
/// 只看名字排序，**不做硬排除**：硬排除会误杀（有厂商用私有驱动名），
/// 而排序 + 探活已经足够把 diag 口排到最后。
fn driver_rank(driver: &str) -> u8 {
    let d = driver.to_ascii_lowercase();
    if d.contains("option") || d.contains("qcserial") || d.contains("cdc_acm") {
        return 0;
    }
    if d.contains("usbserial") || d.contains("usb_serial") || d.contains("acm") {
        return 1;
    }
    if d.contains("diag") || d.contains("mdm") || d.contains("qcdm") {
        return 9;
    }
    5
}

/// 端口号自然序（`ttyUSB2` < `ttyUSB10`）。
fn natural_key(path: &str) -> (String, u64) {
    let idx = path
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(path.len());
    let (head, tail) = path.split_at(idx);
    let num: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    (head.to_string(), num.parse().unwrap_or(0))
}

/// 生成 probe 计划（纯函数）。
pub fn plan(ports: &[PortInfo]) -> Vec<Group> {
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<String, Vec<&PortInfo>> = BTreeMap::new();
    for p in ports {
        if p.path.trim().is_empty() {
            continue;
        }
        groups.entry(p.group_key()).or_default().push(p);
    }

    let mut out = Vec::new();
    for (key, mut members) in groups {
        // 组内型号身份取第一个非空的（同组一定属于同一 USB 设备）
        let first = members
            .iter()
            .find(|p| !p.vid.is_empty() && !p.pid.is_empty())
            .copied();
        let (vid, pid) = first
            .map(|p| (p.vid.clone(), p.pid.clone()))
            .unwrap_or_default();
        let rule = if vid.is_empty() || pid.is_empty() {
            None
        } else {
            ports::lookup(&vid, &pid)
        };

        let mut skipped: Vec<(String, String)> = Vec::new();
        let mut allowed: Vec<&PortInfo> = Vec::new();
        for p in members.iter().copied() {
            if let Some(r) = rule {
                if !r.voice_pcm_interface.is_empty() && p.iface == r.voice_pcm_interface {
                    skipped.push((
                        p.path.clone(),
                        format!("语音 PCM 接口 {}（端口规则剔除）", r.voice_pcm_interface),
                    ));
                    continue;
                }
                if !p.iface.is_empty() && !r.allows_interface(&p.iface) {
                    skipped.push((
                        p.path.clone(),
                        format!(
                            "接口 {} 不在规则白名单 [{}] 内",
                            p.iface,
                            r.at_interfaces.join(",")
                        ),
                    ));
                    continue;
                }
            }
            allowed.push(p);
        }

        if allowed.is_empty() {
            continue;
        }

        // 排序键：规则里的接口顺序 → 驱动像不像 AT 口 → 自然端口号
        let iface_rank = |p: &PortInfo| -> usize {
            match rule {
                Some(r) if !r.at_interfaces.is_empty() => r
                    .at_interfaces
                    .iter()
                    .position(|i| i == &p.iface)
                    .unwrap_or(usize::MAX),
                _ => 0,
            }
        };
        allowed.sort_by(|a, b| {
            iface_rank(a)
                .cmp(&iface_rank(b))
                .then(driver_rank(&a.driver).cmp(&driver_rank(&b.driver)))
                .then(natural_key(&a.path).cmp(&natural_key(&b.path)))
        });
        // 同路径去重（serialport 偶尔把同一节点列两次）
        allowed.dedup_by(|a, b| a.path == b.path);
        members.clear();

        let primary = allowed[0].path.clone();
        let fallbacks = allowed.iter().skip(1).map(|p| p.path.clone()).collect();
        let rule_note = match rule {
            Some(r) => format!(
                "端口规则 {}:{} → {}（AT 接口 {}）",
                vid,
                pid,
                if r.model.is_empty() {
                    "未标型号"
                } else {
                    &r.model
                },
                if r.at_interfaces.is_empty() {
                    "不限".to_string()
                } else {
                    r.at_interfaces.join(",")
                }
            ),
            None => String::new(),
        };
        out.push(Group {
            key,
            vid,
            pid,
            primary,
            fallbacks,
            skipped,
            rule_note,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str, dev: &str, iface: &str, driver: &str, vid: &str, pid: &str) -> PortInfo {
        PortInfo {
            path: path.into(),
            usb_dev: dev.into(),
            iface: iface.into(),
            driver: driver.into(),
            vid: vid.into(),
            pid: pid.into(),
        }
    }

    /// RM520N-GL：4 个 tty，1.1 是 PCM、1.0 不在白名单，只该选 1.2。
    #[test]
    fn rm520n_picks_at_interface_and_skips_pcm() {
        let ports = vec![
            p("/dev/ttyUSB0", "1-1", "1.0", "option", "2c7c", "0801"),
            p("/dev/ttyUSB1", "1-1", "1.1", "option", "2c7c", "0801"),
            p("/dev/ttyUSB2", "1-1", "1.2", "option", "2c7c", "0801"),
            p("/dev/ttyUSB3", "1-1", "1.3", "option", "2c7c", "0801"),
        ];
        let g = plan(&ports);
        assert_eq!(g.len(), 1, "一台设备只能有一组");
        assert_eq!(g[0].primary, "/dev/ttyUSB2");
        assert_eq!(g[0].fallbacks, vec!["/dev/ttyUSB3"]);
        let skipped: Vec<&str> = g[0].skipped.iter().map(|(p, _)| p.as_str()).collect();
        assert!(skipped.contains(&"/dev/ttyUSB0"));
        assert!(skipped.contains(&"/dev/ttyUSB1"));
        assert!(g[0].rule_note.contains("rm520n-gl"));
    }

    /// 两台同型号模组（VID/PID 相同、物理路径不同）必须各自成组。
    #[test]
    fn identical_models_on_different_ports_stay_separate() {
        let ports = vec![
            p("/dev/ttyUSB0", "1-1", "1.2", "option", "2c7c", "0801"),
            p("/dev/ttyUSB2", "1-2", "1.2", "option", "2c7c", "0801"),
        ];
        let g = plan(&ports);
        assert_eq!(g.len(), 2);
        let primaries: Vec<&str> = g.iter().map(|x| x.primary.as_str()).collect();
        assert!(primaries.contains(&"/dev/ttyUSB0"));
        assert!(primaries.contains(&"/dev/ttyUSB2"));
    }

    /// 没有端口规则的设备不能因此被丢弃：按驱动 + 端口号排序后仍然可用。
    #[test]
    fn unknown_device_still_gets_a_primary() {
        let ports = vec![
            p("/dev/ttyUSB10", "2-1", "1.4", "cdc_acm", "1234", "5678"),
            p("/dev/ttyUSB2", "2-1", "1.2", "diag", "1234", "5678"),
        ];
        let g = plan(&ports);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].primary, "/dev/ttyUSB10", "AT 驱动优先于 diag 驱动");
        assert_eq!(g[0].fallbacks, vec!["/dev/ttyUSB2"]);
        assert!(g[0].rule_note.is_empty());
    }

    /// 自然序：ttyUSB2 排在 ttyUSB10 前面（字面序会反过来）。
    #[test]
    fn port_numbers_sort_naturally() {
        let ports = vec![
            p("/dev/ttyUSB10", "3-1", "", "option", "", ""),
            p("/dev/ttyUSB2", "3-1", "", "option", "", ""),
        ];
        let g = plan(&ports);
        assert_eq!(g[0].primary, "/dev/ttyUSB2");
    }

    /// 规则命中但白名单里的接口一个都没出现（tty 没建出来）→ 不选 diag 凑数。
    #[test]
    fn missing_at_interface_yields_no_group() {
        let ports = vec![
            p("/dev/ttyUSB0", "1-1", "1.0", "option", "2c7c", "0801"),
            p("/dev/ttyUSB1", "1-1", "1.1", "option", "2c7c", "0801"),
        ];
        let g = plan(&ports);
        assert!(g.is_empty(), "宁可等 tty 出现，也不能把 diag 口当命令口");
    }

    /// 非 USB 口（MHI / 纯串口）各自成组，不受 USB 规则影响。
    #[test]
    fn non_usb_ports_are_independent_groups() {
        let ports = vec![
            p("/dev/mhi_DUN0", "", "", "mhi", "", ""),
            p("/dev/ttyUSB0", "1-1", "1.2", "option", "2c7c", "0801"),
        ];
        let g = plan(&ports);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].key, "port:/dev/mhi_DUN0");
        assert_eq!(g[1].key, "usb:1-1");
    }

    /// 规则白名单是「只认这些口」，白名单外的 tty 即使能回 AT 也不能当命令口。
    #[test]
    fn rule_whitelist_is_strict() {
        let ports = vec![
            p("/dev/ttyUSB0", "4-1", "1.1", "option", "305a", "1421"),
            p("/dev/ttyUSB1", "4-1", "1.2", "option", "305a", "1421"),
        ];
        let g = plan(&ports);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].primary, "/dev/ttyUSB0", "1.1 是规则登记的 AT 口");
        assert!(g[0].fallbacks.is_empty(), "1.2 不在白名单，不能当回退口");
        assert_eq!(g[0].skipped.len(), 1);
    }
}
