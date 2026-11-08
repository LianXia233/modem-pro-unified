//! `accept_ra` 前置设置（原 `modem_pro.sh` 里的 `proto_modem_pro_fix_accept_ra`）。
//!
//! 本机是转发设备（`net.ipv6.conf.*.forwarding=1`），内核在这种情况**忽略** `accept_ra=1`，
//! 结果永远只有 link-local。odhcp6c 会主动发 RS，所以必须在它启动之前把值改成 2。
//! proto handler 只负责判定「要不要设」（`mp_no_v6=1` 与 `accept_ra=0` 时都不设），
//! 真正的写入放在本子命令启动时，时序仍是「第一个 RS 之前」。

use super::log;

/// `/proc/sys/net/ipv6/conf/<dev>/accept_ra`。
pub fn accept_ra_path(dev: &str) -> String {
    format!("/proc/sys/net/ipv6/conf/{dev}/accept_ra")
}

/// 当前值是否已经是 2（`cat` 的输出带尾随换行，比较时忽略）。
pub fn already_two(current: &str) -> bool {
    current.trim() == "2"
}

/// 把设备的 `accept_ra` 置为 2；任何失败只记日志、不阻断接口拉起（与旧 shell 一致）。
pub fn fix_accept_ra(dev: &str) {
    let path = accept_ra_path(dev);
    let p = std::path::Path::new(&path);

    if !p.exists() {
        log::line(&format!(
            "警告：{path} 不存在（设备 {dev} 还没起来？），跳过 accept_ra 设置"
        ));
        return;
    }

    match std::fs::read_to_string(p) {
        Ok(v) if already_two(&v) => return,
        Ok(_) => {}
        Err(_) => {
            log::line(&format!(
                "警告：{dev} 的 accept_ra 读取失败，IPv6 可能只有 link-local"
            ));
            return;
        }
    }

    if std::fs::write(p, b"2\n").is_ok() {
        log::line(&format!("已把 {dev} 的 accept_ra 设为 2"));
    } else {
        log::line(&format!(
            "警告：{dev} 的 accept_ra 设置失败，IPv6 可能只有 link-local"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_follows_kernel_layout() {
        assert_eq!(
            accept_ra_path("wwan0"),
            "/proc/sys/net/ipv6/conf/wwan0/accept_ra"
        );
    }

    #[test]
    fn detects_already_set() {
        assert!(already_two("2\n"));
        assert!(already_two("2"));
        assert!(!already_two("1\n"));
        assert!(!already_two("0\n"));
        assert!(!already_two(""));
    }
}
