//! UCI 读写：`uci show` 读出现状、把 [`UciOp`] 落盘。
//!
//! **薄 IO 层**：只负责「跑命令 + 解析文本」，不判断该做什么（那是 `mod.rs` 的
//! 纯策略）。解析部分不依赖平台，可单测；执行部分只在 unix 上有意义
//! （Windows / 桌面 Linux 上没有 `uci`，见 [`available`]）。
//!
//! ## 为什么用 `uci show` 文本而不是 libuci
//!
//! OpenWrt 上没有稳定的 libuci 开发包可链；而 `uci show` 的输出是稳定的
//! `pkg.section.key='value'` 行格式，解析它只要几十行纯函数。
//! 代价是每次组网要 fork 两个 `uci` —— 但组网循环 ≤15 秒一轮，可接受。

use std::process::Command;

use crate::error::{msg, Result};

use super::{ExistingIface, UciOp};

/// `uci` 可执行文件是否可用（顺带测一下 `--version` 能不能跑）。
///
/// 非 Linux 平台恒为 false —— 组网整段空操作（红线 2）。
pub fn available() -> bool {
    if !cfg!(unix) {
        return false;
    }
    Command::new("uci")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 跑一条 `uci`，返回 stdout（trim 后）。
fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("uci").args(args).output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if o.status.success() {
                Ok(text)
            } else {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                Err(msg(format!(
                    "uci {} 失败: {}",
                    args.join(" "),
                    if err.is_empty() { "无 stderr" } else { &err }
                )))
            }
        }
        Err(e) => Err(msg(format!("跑不了 uci {}: {e}", args.join(" ")))),
    }
}

/// 读 `network` + `firewall` 的现状。
///
/// 第二个返回值是**zone 名 → section 路径**（如 `wan` → `firewall.@zone[1]`）。
/// ⚠️ 不能假设 section 名就等于 zone 名：实机（ImmortalWrt 默认配置）的 wan zone 是
/// **匿名 section** `firewall.@zone[1]`，`name='wan'` 只是它的一个选项 ——
/// 拿 `firewall.wan.network` 去 `add_list` 会报 `Invalid argument`（实测踩过）。
pub fn read_existing() -> Result<(
    Vec<ExistingIface>,
    std::collections::HashMap<String, String>,
)> {
    let net = run(&["show", "network"])?;
    let fw = run(&["show", "firewall"])?;
    Ok(parse_existing(&net, &fw))
}

/// netifd 里到底有没有这个接口（`ifstatus <name>` 退出码为 0）。
///
/// 只写 UCI 不够：UCI 是**落盘配置**，netifd 的运行时状态要 `network reload` 才会跟上。
/// 早先 reload 调用写错成 uci 子命令、一直失败 —— 结果配置看着全对，`ifstatus` 却说
/// "Interface not found"，接口永远 DOWN。这个函数用来判断"要不要补一次 reload"。
pub fn interface_loaded(name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    Command::new("ifstatus")
        .arg(name.trim())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 把 `uci show network` / `uci show firewall` 的文本解析成现状清单。
///
/// 关心的三类行（其它一概忽略）：
///
/// ```text
/// network.modem1=interface          ← section 诞生
/// network.modem1.proto='modem_pro'  ← 普通选项
/// firewall.wan.network='modem1'     ← 这个接口在哪个 zone 里
/// ```
///
/// `firewall.@defaults[0]=defaults` 这种带 `@` 的**匿名 section** 一律跳过：
/// 它们没有名字，也不可能叫 `modem<id>`。
pub fn parse_existing(
    net: &str,
    fw: &str,
) -> (
    Vec<ExistingIface>,
    std::collections::HashMap<String, String>,
) {
    let mut out: Vec<ExistingIface> = Vec::new();
    // 接口名 → 在 out 里的下标（zone 反查用）
    let mut idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for line in net.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let (lhs, rhs) = (&line[..eq], &line[eq + 1..]);
        // 形如 `network.<name>=<type>`：section 诞生（只收 interface 类型，
        // route/rule/switch 等与本程序无关）
        if let Some(name) = section_name(lhs, "network") {
            if unquote(rhs) == "interface" && !idx.contains_key(&name) {
                idx.insert(name.clone(), out.len());
                out.push(ExistingIface {
                    name,
                    ..Default::default()
                });
            }
            continue;
        }
        // 形如 `network.<name>.<key>='value'`：给已存在的 section 补一个选项
        let Some((lhs, value)) = split_option(line, "network") else {
            continue;
        };
        let mut parts = lhs.splitn(2, '.');
        let (Some(name), Some(key)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(&i) = idx.get(name) else { continue };
        match key {
            "proto" => out[i].proto = value,
            "device" => out[i].device = value,
            "metric" => out[i].metric = value,
            "auto" => out[i].auto = value,
            "ipaddr" => out[i].ipaddr = value,
            "netmask" => out[i].netmask = value,
            "gateway" => out[i].gateway = value,
            "extendprefix" => out[i].extendprefix = value,
            "ip6addr" => out[i].ip6addr = value,
            _ => {}
        }
    }

    // 防火墙 zone。三种行都要认（顺序无关，先收 section 再按行补选项）：
    //   firewall.@zone[1]=zone              ← section 诞生（**可能是匿名的**）
    //   firewall.@zone[1].name='wan'        ← zone 名只是它的一个选项
    //   firewall.@zone[1].network='wan' 'wan6'  ← **列表值，空格分隔**
    let mut zones: std::collections::HashMap<String, (String, Vec<String>)> =
        std::collections::HashMap::new(); // section 路径 → (zone 名, 成员)
    for line in fw.lines() {
        let line = line.trim();
        let Some((section, key, value)) = fw_entry(line) else {
            continue;
        };
        if key.is_empty() {
            // section 诞生行：只收 zone 类型
            if value == "zone" {
                zones
                    .entry(section)
                    .or_insert_with(|| (String::new(), Vec::new()));
            }
            continue;
        }
        let Some(entry) = zones.get_mut(&section) else {
            continue; // 不是 zone（defaults / redirect / rule…）
        };
        match key.as_str() {
            "name" => entry.0 = value,
            "network" => entry.1.extend(split_uci_list(&value)),
            _ => {}
        }
    }
    // zone 名 → section 路径（调用方生成 add_list/del_list 时用真路径）
    let mut zone_sections: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for (section, (name, members)) in &zones {
        // 具名 section（`firewall.wan`）没有 name 选项时，section 名就是 zone 名；
        // 匿名 section（`firewall.@zone[1]`）没写 name 才是配置残缺，跳过不猜。
        let name = if !name.is_empty() {
            name.clone()
        } else if let Some(sec) = section.strip_prefix("firewall.") {
            if sec.starts_with('@') {
                continue;
            }
            sec.to_string()
        } else {
            continue;
        };
        zone_sections.insert(name.clone(), section.clone());
        for m in members {
            if let Some(&i) = idx.get(m) {
                if !out[i].zones.iter().any(|z| z == &name) {
                    out[i].zones.push(name.clone());
                }
            }
        }
    }
    (out, zone_sections)
}

/// 解析 `firewall` 的一行：`firewall.<section>[.<key>]='<value>'`。
///
/// 返回 `(section 路径, key, 去引号值)`；section 诞生行的 key 为空串。
/// section 里可能带 `@zone[1]` 这种匿名下标 —— 在**最后一个点**处切，
/// 不能按第一个点切（`@zone[1].name` 会切出 `@zone[1]` 才对）。
fn fw_entry(line: &str) -> Option<(String, String, String)> {
    let eq = line.find('=')?;
    let lhs = line[..eq].strip_prefix("firewall.")?;
    let raw = unquote(&line[eq + 1..]);
    match lhs.rfind('.') {
        None => Some((format!("firewall.{lhs}"), String::new(), raw)),
        Some(dot) => {
            let section = &lhs[..dot];
            let key = &lhs[dot + 1..];
            if section.is_empty() || key.is_empty() || key.contains('.') {
                return None;
            }
            Some((format!("firewall.{section}"), key.to_string(), raw))
        }
    }
}

/// UCI 列表值拆分：`'wan' 'wan6'` → ["wan", "wan6"]（也容忍无引号/逗号分隔）。
fn split_uci_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || c == ',')
        .map(|s| s.trim().trim_matches('\'').trim_matches('"').trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 从 `pkg.<name>` 里取 section 名（匿名 section 返回 None）。
///
/// 带更多点号的（`pkg.<name>.<key>`）也返回 None —— 那是**选项行**不是
/// section 定义行，交给 [`split_option`] 处理。少了这个判断，选项行会先被
/// 当成「新建了一个叫 `modem1.proto` 的 section」，proto 永远读不出来。
fn section_name(lhs: &str, pkg: &str) -> Option<String> {
    let rest = lhs.strip_prefix(pkg)?.strip_prefix('.')?;
    if rest.starts_with('@') || rest.contains('.') {
        return None; // 匿名 section / 选项行
    }
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

/// 从 `pkg.<name>.<key>='value'` 里取 `(name.key, 去引号值)`。
fn split_option(line: &str, pkg: &str) -> Option<(String, String)> {
    let eq = line.find('=')?;
    let lhs = line[..eq].strip_prefix(pkg)?.strip_prefix('.')?;
    // 至少要有 `name.key` 两段
    if !lhs.contains('.') {
        return None;
    }
    Some((lhs.to_string(), unquote(&line[eq + 1..])))
}

/// 去掉 UCI 值外面的单/双引号（`'modem_pro'` → `modem_pro`）。
pub fn unquote(s: &str) -> String {
    let t = s.trim();
    for q in ['\'', '"'] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return t[1..t.len() - 1].to_string();
        }
    }
    t.to_string()
}

/// 把一组操作落盘（`uci set` × N + 一次 `commit`）。
///
/// **一次 commit**：逐条 set 之后统一提交，中间失败也不会留下半套配置 ——
/// 但注意 UCI 的 set 是立即进 staging 的，所以「全部 set 成功才 commit」
/// 只能保证 commit 这一步不分裂，不能回滚已经 set 的部分。真要原子性得先
/// 备份整段，那对组网这种低频操作太重，这里用「逐条记日志 + 失败即停」兜底。
pub fn apply(ops: &[UciOp]) -> Result<usize> {
    if ops.is_empty() {
        return Ok(0);
    }
    if !available() {
        return Err(msg("uci 不可用（非 OpenWrt？）"));
    }
    let mut done = 0usize;
    for op in ops {
        let args = op_args(op);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run(&refs)?;
        done += 1;
    }
    run(&["commit", "network"])?;
    run(&["commit", "firewall"])?;
    Ok(done)
}

/// 一条操作对应的 `uci` 参数。
fn op_args(op: &UciOp) -> Vec<String> {
    match op {
        UciOp::Create { section } => vec!["set".into(), format!("network.{section}=interface")],
        UciOp::Set {
            section,
            key,
            value,
        } => vec!["set".into(), format!("network.{section}.{key}={value}")],
        UciOp::AddList {
            section,
            key,
            value,
        } => vec!["add_list".into(), format!("{section}.{key}={value}")],
        UciOp::DelList {
            section,
            key,
            value,
        } => vec!["del_list".into(), format!("{section}.{key}={value}")],
        UciOp::Remove { section } => vec!["delete".into(), format!("network.{section}")],
        UciOp::Unset { section, key } => {
            vec!["delete".into(), format!("network.{section}.{key}")]
        }
    }
}

/// 让 netifd 重新加载网络配置（改完 UCI 不 reload 不生效）。
///
/// 失败只返回错误、不中断组网：接口已经写进 UCI，下次 netifd 自己 reload
/// 也会用上；而组网循环的职责是「把配置写对」。
pub fn reload_network() -> Result<()> {
    // ⚠️ 这里要跑的是 init 脚本，**不是** uci 子命令：早先写成
    // `uci -c /etc/init.d/network reload`，实机直接回 "Usage: uci …"，
    // 结果 UCI 写进去了却没生效（接口一直是 DOWN）。
    let out = Command::new("/etc/init.d/network")
        .arg("reload")
        .output()
        .map_err(|e| msg(format!("跑不了 /etc/init.d/network reload: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(msg(format!(
        "/etc/init.d/network reload 失败: {}",
        if err.is_empty() { "无 stderr" } else { &err }
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_interface_sections() {
        let net = "\
network.loopback=interface
network.loopback.proto='static'
network.loopback.device='lo'
network.globals=globals
network.modem1=interface
network.modem1.proto='modem_pro'
network.modem1.device='wwan0'
network.modem1.metric='20'
network.modem1.auto='1'
network.@route[0]=route
";
        let (out, _) = parse_existing(net, "");
        assert_eq!(out.len(), 2, "只收 interface 类型的 section");
        let m = out.iter().find(|e| e.name == "modem1").unwrap();
        assert_eq!(m.proto, "modem_pro");
        assert_eq!(m.device, "wwan0");
        assert_eq!(m.metric, "20");
        assert_eq!(m.auto, "1");
    }

    #[test]
    fn parses_zone_membership() {
        let net = "network.modem1=interface\nnetwork.modem1.proto='modem_pro'\n";
        let fw = "\
firewall.@defaults[0]=defaults
firewall.@defaults[0].input='ACCEPT'
firewall.wan=zone
firewall.wan.input='REJECT'
firewall.wan.network='wan'
firewall.wan.network='modem1'
firewall.lan=zone
firewall.lan.network='lan'
";
        let (out, _) = parse_existing(net, fw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].zones, vec!["wan".to_string()]);
    }

    #[test]
    fn parses_anonymous_zone_with_list_value() {
        // 实机 ImmortalWrt 默认配置：wan zone 是**匿名 section**，network 是列表值
        let net = "network.modem0=interface\nnetwork.modem0.proto='dhcp'\n";
        let fw = "\
firewall.@zone[0]=zone
firewall.@zone[0].name='lan'
firewall.@zone[0].network='lan'
firewall.@zone[1]=zone
firewall.@zone[1].name='wan'
firewall.@zone[1].input='ACCEPT'
firewall.@zone[1].masq='1'
firewall.@zone[1].network='wan' 'wan6' 'modem0'
";
        let (out, zone_sections) = parse_existing(net, fw);
        assert_eq!(out[0].zones, vec!["wan".to_string()]);
        // 关键：zone 名 → 真实 section 路径（不能拿 `firewall.wan` 去 add_list）
        assert_eq!(
            zone_sections.get("wan").map(String::as_str),
            Some("firewall.@zone[1]")
        );
        assert_eq!(
            zone_sections.get("lan").map(String::as_str),
            Some("firewall.@zone[0]")
        );
    }

    #[test]
    fn ignores_unknown_keys_and_zones() {
        let net = "network.modem1=interface\nnetwork.modem1.proto='modem_pro'\nnetwork.modem1.ip6ifaceid='random'\n";
        let fw = "firewall.wan=zone\nfirewall.wan.network='modem1'\nfirewall.wan.mtu_fix='1'\n";
        let (out, _) = parse_existing(net, fw);
        assert_eq!(out[0].zones, vec!["wan".to_string()]);
    }

    #[test]
    fn handles_missing_values() {
        // 接口建了但还没写 device / metric（本程序 device 为空时就是这样）
        let net = "network.modem1=interface\nnetwork.modem1.proto='modem_pro'\n";
        let (out, _) = parse_existing(net, "");
        assert_eq!(out[0].proto, "modem_pro");
        assert_eq!(out[0].device, "");
        assert_eq!(out[0].metric, "");
        assert!(out[0].zones.is_empty());
    }

    #[test]
    fn unquote_strips_both_quote_styles() {
        assert_eq!(unquote("'modem_pro'"), "modem_pro");
        assert_eq!(unquote("\"modem_pro\""), "modem_pro");
        assert_eq!(unquote("modem_pro"), "modem_pro");
        assert_eq!(unquote("  20  "), "20");
        assert_eq!(unquote("''"), "");
    }

    #[test]
    fn op_args_match_uci_syntax() {
        assert_eq!(
            op_args(&UciOp::Create {
                section: "modem1".into()
            }),
            vec!["set".to_string(), "network.modem1=interface".to_string()]
        );
        assert_eq!(
            op_args(&UciOp::Set {
                section: "modem1".into(),
                key: "proto".into(),
                value: "modem_pro".into()
            }),
            vec![
                "set".to_string(),
                "network.modem1.proto=modem_pro".to_string()
            ]
        );
        assert_eq!(
            op_args(&UciOp::AddList {
                section: "firewall.wan".into(),
                key: "network".into(),
                value: "modem1".into()
            }),
            vec![
                "add_list".to_string(),
                "firewall.wan.network=modem1".to_string()
            ]
        );
        assert_eq!(
            op_args(&UciOp::Remove {
                section: "modem1_6".into()
            }),
            vec!["delete".to_string(), "network.modem1_6".to_string()]
        );
    }

    #[test]
    fn apply_noop_on_empty() {
        assert_eq!(apply(&[]).unwrap(), 0);
    }
}
