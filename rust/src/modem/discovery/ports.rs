//! 串口枚举：找出「可能是 AT 口」的设备节点。

/// 列出候选串口路径。
///
/// unix：serialport 会返回 /dev/ttyUSB*、/dev/ttyACM* 等；windows：返回 COMx。
pub fn list_candidates() -> Vec<String> {
    let mut out: Vec<String> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.port_name)
        .filter(|name| is_at_candidate(name))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 是否是「值得拿 AT 去探」的串口名。
///
/// 只认 USB/ACM 串口（模组的 AT 口几乎都在这两类里）；板载 UART（ttyS*）默认不探，
/// 避免误伤控制台。
pub fn is_at_candidate(name: &str) -> bool {
    name.contains("ttyUSB")
        || name.contains("ttyACM")
        || name.starts_with("COM")
        || name.contains("ttyAP")
        || name.contains("ttyGS")
}

/// 本机是否存在这个串口设备（系统串口列表里有这个名字）。
///
/// 用 sysfs glob（`ttyUSB*` / `ttyACM*` / `mhi_*DUN*` / `wwan*at*`…）列名单；
/// 这里放宽成「路径存在且是字符设备」—— 语义一致（防手滑打错端口名存下来），
/// 且对 MHI / wwan / 虚拟串口一律适用。
pub fn port_exists(port: &str) -> bool {
    let p = port.trim();
    if p.is_empty() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        match std::fs::metadata(p) {
            Ok(md) => md.file_type().is_char_device(),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        std::path::Path::new(p).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::{is_at_candidate, port_exists};

    #[test]
    fn filters_ports() {
        assert!(is_at_candidate("/dev/ttyUSB2"));
        assert!(is_at_candidate("/dev/ttyACM0"));
        assert!(is_at_candidate("COM5"));
        assert!(!is_at_candidate("/dev/ttyS0"));
    }

    #[test]
    fn existence_check() {
        // 空 / 不存在的路径
        assert!(!port_exists(""));
        assert!(!port_exists("  "));
        assert!(!port_exists("/dev/definitely-not-a-port-xyz"));
        // unix：/dev/null 是字符设备（pty 从设备也是，端到端测试靠这条）
        #[cfg(unix)]
        assert!(port_exists("/dev/null"));
        // windows：普通文件路径也算存在（没有字符设备概念）
        #[cfg(not(unix))]
        {
            let f = std::env::temp_dir().join(format!("modemd-port-{}", std::process::id()));
            std::fs::write(&f, b"x").unwrap();
            assert!(port_exists(&f.to_string_lossy()));
            let _ = std::fs::remove_file(&f);
        }
    }
}
