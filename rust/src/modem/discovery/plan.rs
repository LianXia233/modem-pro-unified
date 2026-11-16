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
    /// USB 接口描述串（sysfs `interface`，如 `TDTECH Connect - PC UI Interface`）。
    pub iface_name: String,
    /// `bInterfaceProtocol`（小写十六进制、去 `0x`，如 `12` = PCUI/AT、`13` = DIAG）。
    pub protocol: String,
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

/// 把接口描述串切成大写整词（非字母数字一律作分隔）。
///
/// 必须整词匹配而不是 `contains`：实机 `TDTECH Connect - Application Interface`
/// 里含子串 `at`（Applic**at**ion），按子串会把它误判成 AT 口，
/// 而真正的 `PC UI Interface` 反被排到后面（A-P2-16）。
fn tokenize(iface: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in iface.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_uppercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn has_token(tokens: &[String], words: &[&str]) -> bool {
    tokens.iter().any(|t| words.iter().any(|w| t == w))
}

/// 接口名是否给出了**强** AT 口信号（PCUI / PC UI / ATPORT）。
///
/// 强信号优先于协议号：这些是 MT5700 系列对 AT 口的专有叫法，
/// 误写概率极低；而 `AT` / `Modem` 这类通用词可能被 OEM 误用在数据口上。
fn strong_at_name(iface: &str) -> bool {
    let t = tokenize(iface);
    has_token(&t, &["PCUI"])
        || (has_token(&t, &["PC"]) && has_token(&t, &["UI"]))
        || has_token(&t, &["ATPORT"])
}

/// 接口名打分：0=AT 口，1=模组/命令口，2=未知，3=明确不是 AT 口。
fn iface_score(iface: &str) -> Option<u8> {
    if iface.trim().is_empty() {
        return None;
    }
    let t = tokenize(iface);
    let at_port = has_token(&t, &["PCUI"])
        || (has_token(&t, &["PC"]) && has_token(&t, &["UI"]))
        || (has_token(&t, &["AT"]) && has_token(&t, &["PORT"]))
        || has_token(&t, &["ATPORT"])
        || (has_token(&t, &["AT"]) && has_token(&t, &["INTERFACE", "CMD"]));
    if at_port {
        return Some(0);
    }
    // GPS/NMEA 只吐定位语句，DIAG 是调试口：先于弱 AT 词判定
    if has_token(&t, &["GPS", "GNSS", "NMEA", "DIAG", "DIAGNOSTIC"]) {
        return Some(3);
    }
    if has_token(&t, &["MODEM", "COMMAND", "AT", "ATCMD", "DUN"]) {
        return Some(1);
    }
    Some(2)
}

/// `bInterfaceProtocol` 打分，只认「明确是 / 明确不是」：
/// 0x12=PCUI(AT)；0x14=GPS、0x13/0x1b/0x1c=DIAG/Serial_B/C 均为明确非 AT 口。
///
/// 与上游（`luci-app-mt5700` serialdetect 给 0x13 打 2=未知）相比，这里把
/// DIAG/Serial 一并降到 3：它们**不是**命令口，排到未知口之后仍会作为最后
/// 兜底被尝试（不丢弃），但不会因为驱动名/编号序抢在真实候选前面。
fn protocol_score(proto: &str) -> Option<u8> {
    match proto.trim().trim_start_matches("0x") {
        "12" => Some(0),
        "13" | "14" | "1b" | "1c" => Some(3),
        _ => None,
    }
}

/// 端口角色分（0 最好）：接口名强信号 → 名字/协议交叉校验 → 协议兜底 → 未知。
fn port_score(p: &PortInfo) -> u8 {
    if strong_at_name(&p.iface_name) {
        return 0;
    }
    let name = iface_score(&p.iface_name);
    let proto = protocol_score(&p.protocol);
    match (name, proto) {
        // 名字只是弱 AT 信号，但协议号明确说「不是 AT 口」→ 信协议号
        (Some(0 | 1), Some(s @ 2..=3)) => s,
        (Some(s), _) if s != 2 => s,
        // 名字缺失/不认识 → 协议号兜底
        (_, Some(s)) => s,
        _ => 2,
    }
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
                // 接口角色分（接口名 + bInterfaceProtocol）：0x12/PCUI 优先，
                // DIAG/GPS 沉底（A-P2-16）
                .then(port_score(a).cmp(&port_score(b)))
                .then(driver_rank(&a.driver).cmp(&driver_rank(&b.driver)))
                .then(natural_key(&a.path).cmp(&natural_key(&b.path)))
        });
        // 同路径去重（serialport 偶尔把同一节点列两次）
        allowed.dedup_by(|a, b| a.path == b.path);
        members.clear();

        // 明确不是 AT 口的口（GPS/DIAG/Serial_B/C，分数 3）不参与首选；
        // 只有组内全是这类口时才退而求其次（保留为最后兜底，而不是直接丢弃）。
        let primary_idx = allowed
            .iter()
            .position(|p| port_score(p) < 3)
            .unwrap_or(0);
        let primary = allowed[primary_idx].path.clone();
        // 回退序里把「明确不是 AT 口」的口排到最后：规则白名单可能把 GPS/DIAG
        // 也列进来且接口顺序靠前，不能让它抢在普通候选之前被尝试。
        let mut good_fb: Vec<String> = Vec::new();
        let mut bad_fb: Vec<String> = Vec::new();
        for (i, p) in allowed.iter().enumerate() {
            if i == primary_idx {
                continue;
            }
            if port_score(p) >= 3 {
                bad_fb.push(p.path.clone());
            } else {
                good_fb.push(p.path.clone());
            }
        }
        good_fb.extend(bad_fb);
        let fallbacks = good_fb;
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
            iface_name: String::new(),
            protocol: String::new(),
        }
    }

    /// 带接口名 / 协议号的端口（A-P2-16 的判据输入）。
    fn pn(
        path: &str,
        dev: &str,
        iface: &str,
        driver: &str,
        vid: &str,
        pid: &str,
        name: &str,
        proto: &str,
    ) -> PortInfo {
        PortInfo {
            iface_name: name.into(),
            protocol: proto.into(),
            ..p(path, dev, iface, driver, vid, pid)
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

    /// 实机 MT5700：`Application` 含子串 at，但真正的 AT 口是 `PC UI`
    /// （A-P2-16：整词匹配，不能按子串）。
    #[test]
    fn application_interface_never_beats_pcui() {
        let ports = vec![
            pn(
                "/dev/ttyUSB0",
                "1-1",
                "1.2",
                "option",
                "3466",
                "0001",
                "TDTECH Connect - Application Interface",
                "13",
            ),
            pn(
                "/dev/ttyUSB1",
                "1-1",
                "1.3",
                "option",
                "3466",
                "0001",
                "TDTECH Connect - PC UI Interface",
                "12",
            ),
        ];
        assert_eq!(
            port_score(&ports[0]),
            3,
            "Application 里的 at 子串不算 AT 信号，协议 0x13 又明确是 DIAG ⇒ 末位"
        );
        // 单看接口名（无协议号）也必须是「未知」，不能因 Application 里的 at 子串升格
        let bare = pn(
            "/dev/ttyUSB9",
            "1-1",
            "1.2",
            "option",
            "3466",
            "0001",
            "TDTECH Connect - Application Interface",
            "",
        );
        assert_eq!(port_score(&bare), 2);
        assert_eq!(port_score(&ports[1]), 0, "PC UI 是强 AT 信号");
        let g = plan(&ports);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].primary, "/dev/ttyUSB1");
        assert_eq!(g[0].fallbacks, vec!["/dev/ttyUSB0"]);
    }

    /// 接口名缺失时靠 bInterfaceProtocol：0x12 = PCUI/AT。
    #[test]
    fn protocol_number_breaks_ties_without_names() {
        let ports = vec![
            pn("/dev/ttyUSB0", "1-1", "1.2", "option", "", "", "", "13"),
            pn("/dev/ttyUSB1", "1-1", "1.3", "option", "", "", "", "12"),
            pn("/dev/ttyUSB2", "1-1", "1.4", "option", "", "", "", ""),
        ];
        let g = plan(&ports);
        assert_eq!(g[0].primary, "/dev/ttyUSB1");
        // 0x13 明确不是 AT 口，排到末尾；未知协议的普通口优先于它
        assert_eq!(g[0].fallbacks, vec!["/dev/ttyUSB2", "/dev/ttyUSB0"]);
    }

    /// 名字弱信号被协议号否决：OEM 把数据口也写成 "AT Port"，但协议号是 DIAG。
    #[test]
    fn protocol_overrides_weak_name_signal() {
        let ports = vec![
            pn("/dev/ttyUSB0", "1-1", "1.2", "option", "", "", "AT Port", "13"),
            pn("/dev/ttyUSB1", "1-1", "1.3", "option", "", "", "Modem", "00"),
        ];
        assert_eq!(port_score(&ports[0]), 3, "弱 AT 名 + DIAG 协议 → 不可信");
        assert_eq!(port_score(&ports[1]), 1);
        let g = plan(&ports);
        assert_eq!(g[0].primary, "/dev/ttyUSB1");
    }

    /// GPS/DIAG/Serial 口只作最后兜底：有正常候选时绝不当首选，但也不丢。
    #[test]
    fn non_at_ports_are_kept_only_as_last_resort() {
        let ports = vec![
            pn("/dev/ttyUSB0", "1-1", "1.0", "option", "", "", "GPS NMEA", "14"),
            pn("/dev/ttyUSB1", "1-1", "1.1", "option", "", "", "Serial_B", "1b"),
        ];
        let g = plan(&ports);
        // 全是非 AT 口时仍给一个首选（否则设备彻底不可用）
        assert_eq!(g[0].primary, "/dev/ttyUSB0");
        assert_eq!(g[0].fallbacks, vec!["/dev/ttyUSB1"]);
    }
}
