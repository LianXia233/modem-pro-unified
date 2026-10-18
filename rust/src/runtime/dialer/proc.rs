//! 拨号器托管的薄 IO：拉起、按进程树收起、探活。
//!
//! ## 为什么按「进程树」而不是「进程组」
//!
//! 配置注释的原话是「进程退出前**按进程组一起收**（连它拉起的 busybox udhcpc
//! 一起），不留占着控制口的尾巴」。进程组是对的，但纯 `std` 拿不到
//! `setsid()` —— 那需要 `libc`，而 `Cargo.toml` 的「依赖刻意保持精简」不想
//! 为它破例（这个仓库现在连 `libc` 都没直接依赖）。
//!
//! 所以这里换个等价且**不依赖任何假设**的做法：**扫 `/proc` 收集进程树**，
//! 一个一个按**正 pid** 发信号。
//!
//! 三个好处：
//!
//! 1. 不依赖 `setsid` 到底 fork 不 fork（fork 了 pid 就对不上，组也更难找）；
//! 2. 不依赖 BusyBox `kill` 收不收负 pid（进程组写法要 `kill -TERM -- -PGID`）；
//! 3. 覆盖的是**真实的那棵树**：拨号器 → busybox udhcpc → 它再起的脚本，
//!    而不是「碰巧同组的所有进程」。
//!
//! 代价是每次 `stop` 要扫一遍 `/proc` —— 路由器上进程数几十个，可忽略。

use super::DialerPlan;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::Duration;

/// 拉起一个拨号器，返回子进程句柄。
///
/// **必须把 `Child` 交回去**（调用方存进 `Slot`）：Unix 上父进程不 `wait` 的话，
/// 子进程退出后会变成僵尸，`/proc/<pid>` 依旧存在 —— 那时「探活」会永远认为
/// 它活着，拨号器退一次就再也不重拉（配置注释的「退出自动重试」失效）。
///
/// 二进制不存在 / 不可执行时**直接报错并把路径带出来**（配置注释：指向不存在
/// 的路径直接报错，不静默退回别的）。
#[cfg(unix)]
pub fn spawn(plan: &DialerPlan) -> Result<std::process::Child, String> {
    if !Path::new(&plan.bin).is_file() {
        return Err(format!("拨号器可执行文件不存在: {}", plan.bin));
    }
    // 拨号器自己的 stdout/stderr 进 null：它每轮都刷一大片，而 modemd 的
    // 日志通道是「日志文件或 stderr」两选一（见 logging/logger.rs），inherit
    // 不一定落得了盘。排障靠 modemd 这边打的完整命令行（可复制手跑）+ 退出原因。
    std::process::Command::new(&plan.bin)
        .args(&plan.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("拉起 {} 失败: {e}", plan.bin))
}

/// Windows / 其它平台：这些拨号器压根不存在，明确说清楚。
#[cfg(not(unix))]
pub fn spawn(_plan: &DialerPlan) -> Result<std::process::Child, String> {
    Err("拨号器托管只在 Linux/OpenWrt 上有效（当前平台没有这些厂商拨号器）".into())
}

/// 停掉一个进程树（含它拉起的 udhcpc）：先 TERM 给个机会自己收，
/// 还在就 KILL。udhcpc 是拨号器的直接子进程，TERM 它也会乖乖退。
#[cfg(unix)]
pub fn stop(pid: i32) {
    if pid <= 0 {
        return;
    }
    let tree = process_tree(pid);
    if tree.is_empty() {
        return;
    }
    for p in &tree {
        signal(*p, "TERM");
    }
    // 等它自己收（udhcpc 收到 TERM 会删租约再退，值得等这一下）
    if wait_gone(&tree, Duration::from_millis(500)) {
        return;
    }
    for p in &tree {
        if alive(*p) {
            log::warn!("拨号器托管：pid {p} 收到 TERM 后仍在，补 KILL");
            signal(*p, "KILL");
        }
    }
    // KILL 之后也要等：不等的话调用方紧接着 `reap` 时进程可能还在死，
    // `try_wait` 返回「还活着」就不收了，僵尸会一直留着。
    wait_gone(&tree, Duration::from_millis(500));
}

/// 等到整棵树都不在（或超时）。僵尸算不在 —— 见 [`alive`]。
#[cfg(unix)]
fn wait_gone(tree: &[i32], budget: Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if tree.iter().all(|p| !alive(*p)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    tree.iter().all(|p| !alive(*p))
}

#[cfg(not(unix))]
pub fn stop(_pid: i32) {}

/// 进程还在吗。
///
/// ⚠️ **僵尸算不在**：父进程不 `wait` 的话子进程退出后会留 `/proc/<pid>`
/// （状态 `Z`），只判「目录在不在」会永远认为它活着。这里连状态一起看。
#[cfg(unix)]
pub fn alive(pid: i32) -> bool {
    pid > 0 && state_of(pid).map(|s| s != 'Z').unwrap_or(false)
}

#[cfg(not(unix))]
pub fn alive(_pid: i32) -> bool {
    false
}

/// 读 `/proc/<pid>/stat` 的进程状态（第 3 个字段：`R`/`S`/`D`/`Z`/`T`…）。
///
/// 与 `ppid_of` 一样从最后一个 `)` 之后切 —— 第 2 个字段的 comm 可含空格括号。
#[cfg(unix)]
pub fn state_of(pid: i32) -> Option<char> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = s.rsplit_once(')')?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // rest 形如 " S 1234 1235 0 ..."：第 1 个是 state，第 2 个是 ppid
    f.first().and_then(|v| v.chars().next())
}

/// 收尸：`wait` 掉一个已退出的子进程，把僵尸清成干净退出。
///
/// 不收的后果不是「难看」：僵尸占着 pid，且让「探活」必须靠状态字段兜底
/// （见 [`alive`]）。路由器上跑几个月，反复重启拨号器会攒出一串僵尸。
#[cfg(unix)]
pub fn reap(child: &mut std::process::Child) {
    // 已经死了才 wait；还活着的 wait 会**阻塞**到它退出，那是 stop 之后的事
    if let Ok(None) = child.try_wait() {
        return;
    }
    let _ = child.wait();
}

#[cfg(not(unix))]
pub fn reap(_child: &mut std::process::Child) {}

/// 收集一个进程的**整棵树**（含自身，父在前）。
///
/// 扫 `/proc/<数字>/stat` 的 ppid 建邻接表再 DFS。为什么不用
/// `/proc/<pid>/task/*/children`：那个文件要 `CONFIG_PROC_CHILDREN`，
/// 且只给直接子进程，孙子得递归读，反而更麻烦。
#[cfg(unix)]
fn process_tree(root: i32) -> Vec<i32> {
    // 根都不在 /proc 里（已退出且没留僵尸）就没有树可收：
    // 否则 stop(死 pid) 会拿着 [dead] 去发信号、白等两轮 wait_gone。
    if state_of(root).is_none() {
        return Vec::new();
    }
    // pid -> ppid
    let mut parent: Vec<(i32, i32)> = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return vec![root];
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        if let Some(pp) = ppid_of(pid) {
            parent.push((pid, pp));
        }
    }
    let mut out = vec![root];
    // 多轮展开到没有新增为止（进程数几十个，O(n²) 也无感）
    loop {
        let mut added = false;
        for (pid, pp) in &parent {
            if out.contains(pid) {
                continue;
            }
            if out.contains(pp) {
                out.push(*pid);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    out
}

/// 读 `/proc/<pid>/stat` 的 ppid（第 4 个字段）。
///
/// ⚠️ 第 2 个字段是**可含空格与括号的 comm**，不能按空格切 —— 从最后一个 `)`
/// 之后开始切才对（`/proc/pid/stat` 的经典坑）。
#[cfg(unix)]
fn ppid_of(pid: i32) -> Option<i32> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = s.rsplit_once(')')?;
    // rest 形如 " S 1234 1235 0 ..."：第 1 个是 state，第 2 个是 ppid
    let f: Vec<&str> = rest.split_whitespace().collect();
    f.get(1).and_then(|v| v.parse().ok())
}

/// 给一个 pid 发信号（正 pid；`kill` 在所有 BusyBox 上都收正 pid）。
#[cfg(unix)]
fn signal(pid: i32, sig: &str) {
    let _ = Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::DialerPlan;

    /// 平台无关的冒烟：拨号器托管在非 Unix 上必须是「明确说清楚」而不是 panic。
    #[test]
    fn non_unix_says_so_instead_of_panicking() {
        let plan = DialerPlan {
            bin: "/usr/lib/modemd/quectel-CM".into(),
            args: vec!["-s".into(), "cmnet".into()],
            modem_id: 0,
        };
        #[cfg(not(unix))]
        {
            assert!(proc::spawn(&plan).is_err());
            assert!(!proc::alive(1));
            proc::stop(1); // 不能挂、不能抛
        }
        #[cfg(unix)]
        {
            // Unix 上真正干活的路径在下面那个 mod 里测；这里只确认计划能构造
            assert_eq!(plan.command_line(), "/usr/lib/modemd/quectel-CM -s cmnet");
        }
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;

    #[test]
    fn ppid_parsing_survives_spaces_in_comm() {
        // 用本进程测：comm 可能带空格/括号，从最后一个 ) 之后切才对
        let me = std::process::id() as i32;
        assert!(alive(me), "本进程应该在");
        // ppid 一定存在且小于我们（不验具体值，容器/CI 上可能是 0）
        let pp = ppid_of(me);
        assert!(pp.is_some(), "/proc/self 的 stat 应该能解出 ppid");
    }

    #[test]
    fn process_tree_includes_self() {
        let me = std::process::id() as i32;
        let t = process_tree(me);
        assert!(t.contains(&me), "树至少要含根自己");
        // 第一个就是根（父在前，停的时候先停父再停子）
        assert_eq!(t[0], me);
    }

    #[test]
    fn dead_pid_is_not_alive_and_tree_is_empty() {
        // 找一个肯定不存在的 pid：扫 /proc 取 max+1 太麻烦，用一个极大的值
        let dead = i32::MAX - 1;
        assert!(!alive(dead));
        assert!(process_tree(dead).is_empty());
    }

    #[test]
    fn stop_on_dead_pid_is_a_noop() {
        // 不能抛、不能挂
        stop(0);
        stop(-1);
        stop(i32::MAX - 1);
    }

    #[test]
    fn zombie_is_not_alive() {
        // 起一个立刻退出的子进程，**不 wait** —— Unix 上它会变成僵尸，
        // /proc/<pid> 依旧存在。只判「目录在不在」会永远认为它活着，
        // 「退出自动重试」就失效了。这是 proc::spawn 必须把 Child 交回去的原因。
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("起一个 sh 应该没问题");
        let pid = child.id() as i32;
        // 等它退出（最多 2 秒）
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if state_of(pid) == Some('Z') {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(state_of(pid), Some('Z'), "不退的进程不该留在这儿等");
        assert!(!alive(pid), "僵尸必须算不在，否则拨号器永远不重拉");
        // reap 之后 /proc 条目消失
        reap(&mut child);
        assert_eq!(state_of(pid), None);
        assert!(!alive(pid));
    }

    #[test]
    fn state_of_reads_the_third_field() {
        // 本进程是活的，状态只可能是 R/S/D 之一，绝不可能是 Z
        let me = std::process::id() as i32;
        let st = state_of(me).expect("本进程的 stat 应该读得到");
        assert!(matches!(st, 'R' | 'S' | 'D'), "活着的进程不该是 {st:?}");
    }
}
