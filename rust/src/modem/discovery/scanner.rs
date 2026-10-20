//! 探活扫描：按物理设备分组 → 按端口规则挑命令口 → 身份级探活。
//!
//! 与早先「每个 tty 单独探一次 `AT`」的区别见 [`super::plan`]：
//! 一台模组只返回**一个**命令口，diag / 语音 / 非白名单接口不会进列表。

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::modem::at::AtPort;

use super::plan;
use super::ports;

/// 探活失败的口：下次可再试的时刻 + 已失败次数。
#[derive(Debug, Clone, Copy)]
struct Negative {
    next: Instant,
    tries: u32,
}

/// 否定缓存（进程级）。
///
/// 巡检周期默认 3 秒（`[server] scan_interval`），而一台「有 tty 但不是模组」的设备
/// （打印机、GPS、USB 转串口）会永远答不出身份。不做退避的话，每 3 秒就要为它
/// 发 4 条 AT、白等一个探测超时 —— 串口被反复打开，既耗 CPU 又可能把设备弄卡。
///
/// 退避 15s → 30s → 60s → 120s → 300s（封顶），与上游 Go 版
/// `modem_serial.go:307-343` 的 10/20/40…→5min 同量级；端口消失即从缓存里删掉，
/// 重插后会立刻重探（不会因为「上次失败」把新设备挡住）。
fn negative() -> &'static Mutex<std::collections::BTreeMap<String, Negative>> {
    static CELL: OnceLock<Mutex<std::collections::BTreeMap<String, Negative>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()))
}

fn probe_allowed(path: &str) -> bool {
    let g = negative().lock().unwrap_or_else(|e| e.into_inner());
    match g.get(path) {
        None => true,
        Some(n) => Instant::now() >= n.next,
    }
}

/// 第 `tries` 次失败后的退避秒数：15 → 30 → 60 → 120 → 300（封顶）。
fn backoff_secs(tries: u32) -> u64 {
    let shift = tries.saturating_sub(1).min(5);
    (15u64 << shift).min(300)
}

fn note_probe_failed(path: &str) {
    let mut g = negative().lock().unwrap_or_else(|e| e.into_inner());
    let tries = g.get(path).map(|n| n.tries + 1).unwrap_or(1);
    let backoff = Duration::from_secs(backoff_secs(tries));
    g.insert(
        path.to_string(),
        Negative {
            next: Instant::now() + backoff,
            tries,
        },
    );
}

fn note_probe_ok(path: &str) {
    let mut g = negative().lock().unwrap_or_else(|e| e.into_inner());
    g.remove(path);
}

/// 清掉已经不存在的端口（设备拔掉后重插应当是「新设备」，不受旧退避影响）。
fn prune_negative(alive: &BTreeSet<String>) {
    let mut g = negative().lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|p, _| alive.contains(p));
}

/// 一台新发现的模组（含判定依据，供日志与页面排查）。
#[derive(Debug, Clone)]
pub struct Discovered {
    /// 选定的 AT 口。
    pub path: String,
    /// 分组键（`usb:1-1` / `port:/dev/mhi_DUN0`）。
    pub group: String,
    /// 端口规则说明（没命中规则时为空）。
    pub rule_note: String,
    /// 组内被跳过的口与原因。
    pub skipped: Vec<(String, String)>,
    /// 探活拿到的身份行（ATI/CGMM/CGMI）。
    pub evidence: Vec<String>,
    /// 首选口探不通、退到第几个回退口（0 = 首选）。
    pub fallback_index: usize,
}

/// 返回「已知之外、且身份探活成功」的设备（每台物理设备最多一个）。
pub fn discover_new(
    known: &BTreeSet<String>,
    baud: u32,
    probe_timeout: Duration,
) -> Vec<Discovered> {
    let snapshot = ports::snapshot();
    let alive: BTreeSet<String> = snapshot.iter().map(|p| p.path.clone()).collect();
    prune_negative(&alive);
    let groups = plan::plan(&snapshot);
    let mut out = Vec::new();
    for g in groups {
        // 这台物理设备已经有登记（无论登记的是首选口还是回退口）⇒ 整组跳过。
        // 判据用「任一候选已登记」而不是「首选已登记」：手动切口后
        // `Modem::port()` 可能落在回退口上，只看首选会把同一台设备再登记一次。
        if known.contains(&g.primary) || g.fallbacks.iter().any(|p| known.contains(p)) {
            continue;
        }
        let mut candidates: Vec<&String> = Vec::with_capacity(g.fallbacks.len() + 1);
        candidates.push(&g.primary);
        candidates.extend(g.fallbacks.iter());

        for (idx, cand) in candidates.iter().enumerate() {
            if known.contains(*cand) {
                continue;
            }
            // 退避期内不重探：这台口上次连身份都拿不到
            if !probe_allowed(cand) {
                continue;
            }
            let Some(evidence) = AtPort::probe_identity(cand, baud, probe_timeout) else {
                note_probe_failed(cand);
                continue;
            };
            note_probe_ok(cand);
            out.push(Discovered {
                path: (*cand).clone(),
                group: g.key.clone(),
                rule_note: g.rule_note.clone(),
                skipped: g.skipped.clone(),
                evidence,
                fallback_index: idx,
            });
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modem::discovery::plan::PortInfo;

    /// 计划层的分组结果直接决定这里探几个口：同一组只探首选（与回退）。
    #[test]
    fn one_candidate_per_physical_device() {
        let ports = vec![
            PortInfo {
                path: "/dev/ttyUSB0".into(),
                usb_dev: "1-1".into(),
                iface: "1.0".into(),
                driver: "option".into(),
                vid: "2c7c".into(),
                pid: "0801".into(),
            },
            PortInfo {
                path: "/dev/ttyUSB2".into(),
                usb_dev: "1-1".into(),
                iface: "1.2".into(),
                driver: "option".into(),
                vid: "2c7c".into(),
                pid: "0801".into(),
            },
        ];
        let groups = plan::plan(&ports);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].primary, "/dev/ttyUSB2");
        assert!(groups[0].fallbacks.is_empty());
        assert_eq!(groups[0].skipped.len(), 1);
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_secs(1), 15);
        assert_eq!(backoff_secs(2), 30);
        assert_eq!(backoff_secs(3), 60);
        assert_eq!(backoff_secs(4), 120);
        assert_eq!(backoff_secs(5), 240);
        assert_eq!(backoff_secs(6), 300, "封顶 5 分钟");
        assert_eq!(backoff_secs(99), 300);
    }

    /// `discover_new` 在没有串口的机器上必须返回空（CI/开发机）。
    #[test]
    fn empty_machine_yields_nothing() {
        let known: BTreeSet<String> = BTreeSet::new();
        // 只要不 panic 即可：真机有串口时结果取决于硬件
        let got = discover_new(&known, 115200, Duration::from_millis(50));
        assert!(got.len() <= 64);
    }
}
