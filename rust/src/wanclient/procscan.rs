//! 孤儿清理与进程信号。
//!
//! 对应旧 shim 的 `cleanup_stale`：扫描 `/proc/*/cmdline`，凡是命令行里带着**本接口专属**
//! 上报脚本路径的进程一律 SIGKILL。netifd 收尾用 SIGKILL 时 shim 的 trap 跑不到，
//! 这些残留客户端会一直占着网卡，导致新实例收不到 RA（2026-10-13 真机事故）。
//!
//! 判据里带 `INTERFACE`，所以同一台设备上别的 modem_pro 接口不受影响。

#[cfg(unix)]
pub fn cleanup_stale(interface: &str) -> usize {
    if !std::path::Path::new("/proc").is_dir() {
        return 0;
    }

    let self_pid = std::process::id();
    let patterns = [
        format!("mp-dhcp-{interface}.script"),
        format!("mp-dhcpv6-{interface}.script"),
    ];

    let mut killed = 0usize;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        // 进程可能刚好退出 —— 读不到就跳过，不刷错误日志
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        let cmd: String = raw
            .into_iter()
            .map(|b| if b == 0 { ' ' } else { b as char })
            .collect();
        if patterns.iter().any(|p| cmd.contains(p)) && kill9(pid) {
            killed += 1;
        }
    }
    killed
}

#[cfg(not(unix))]
pub fn cleanup_stale(_interface: &str) -> usize {
    0
}

/// 发 SIGTERM（旧 shim 的 `kill`）。
#[cfg(unix)]
pub fn terminate(pid: u32) -> bool {
    kill(pid, libc::SIGTERM)
}

/// 发 SIGKILL（旧 shim 的 `kill -9`）。
#[cfg(unix)]
pub fn kill9(pid: u32) -> bool {
    kill(pid, libc::SIGKILL)
}

#[cfg(unix)]
fn kill(pid: u32, sig: libc::c_int) -> bool {
    // SAFETY: 标准 POSIX 调用，只传进程号与信号常量，无内存/所有权问题。
    unsafe { libc::kill(pid as libc::pid_t, sig) == 0 }
}

#[cfg(not(unix))]
#[allow(dead_code)] // Windows 上保留同名接口：supervisor 的闭包按 cfg 走 start_kill
pub fn terminate(_pid: u32) -> bool {
    false
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn kill9(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_interface_matches_nothing() {
        assert_eq!(super::cleanup_stale("modem-does-not-exist-9x"), 0);
    }
}
