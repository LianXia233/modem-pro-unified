//! 串口枚举：找出「可能是 AT 口」的设备节点，并带上 sysfs 拓扑（USB 设备 / 接口号 / 驱动）。

use std::path::{Path, PathBuf};

use super::plan::PortInfo;

/// 列出候选串口路径。
///
/// - unix：serialport 返回的 `/dev/ttyUSB*`、`/dev/ttyACM*`；
/// - 再并上 PCIe/MHI 模组的 AT 口（`/dev/mhi_*DUN*`、`/dev/mhi_*AT*`、`/dev/wwan*at*`）——
///   这些不是 tty，serialport 枚举不到，必须单独加（上游 Go 版
///   `module/netdev/ports_unix.go:38-51` 同样用 glob 补这四类）；
/// - windows：COMx。
pub fn list_candidates() -> Vec<String> {
    let mut out: Vec<String> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.port_name)
        .filter(|name| is_at_candidate(name))
        .collect();
    for name in crate::modem::pcidev::pcie_at_port_candidates() {
        let path = format!("/dev/{name}");
        if is_at_candidate(&path) {
            out.push(path);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 是否是「值得拿 AT 去探」的串口名。
///
/// USB/ACM 串口 + PCIe/MHI 的 AT 通道；板载 UART（ttyS*）默认不探，避免误伤控制台。
pub fn is_at_candidate(name: &str) -> bool {
    if name.contains("ttyUSB")
        || name.contains("ttyACM")
        || name.starts_with("COM")
        || name.contains("ttyAP")
        || name.contains("ttyGS")
    {
        return true;
    }
    // MHI / wwan：`mhi_1DUN0`、`mhi_1AT0`、`wwan0at0`、`mhi_uci_q0`
    let base = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
    let mhi_at = base.starts_with("mhi_")
        && (base.contains("dun") || base.contains("at") || base.contains("uci_q"));
    let wwan_at = base.starts_with("wwan") && base.contains("at");
    mhi_at || wwan_at
}

/// 本机候选串口的**拓扑快照**（供 [`super::plan`] 决策）。
///
/// 取不到 sysfs（Windows、纯串口、权限不足）的字段留空，计划层会把它当作
/// 「独立设备、无端口规则」处理 —— 不会因此漏掉设备。
pub fn snapshot() -> Vec<PortInfo> {
    list_candidates()
        .into_iter()
        .map(|path| describe(&path))
        .collect()
}

/// 读一个串口的 sysfs 归属：USB 设备名、接口号、驱动、VID/PID。
#[cfg(unix)]
pub fn describe(path: &str) -> PortInfo {
    let mut info = PortInfo {
        path: path.to_string(),
        ..PortInfo::default()
    };
    let Some(dev_link) = tty_device_dir(path) else {
        // 不是 tty（MHI / wwan AT 口）：按名字前缀归组，避免同一台 PCIe 模组的
        // DUN/AT 两个通道各自变成一台「模组」。
        info.usb_dev = pseudo_group_for(path);
        return info;
    };

    // `device` 的指向随驱动/内核而变：可能直接是接口目录（`2-1:1.2`），
    // 也可能是接口下的端口目录（`2-1:1.2/ttyUSB2`）。两种都要能切出 bus/接口号 ——
    // 切不出来时 `usb_dev` 为空，`plan()` 就会退化成「每个串口一台设备」，
    // 一台多 AT 口的模组被登记成多台「模组」，端口规则也因 vid/pid 为空而匹配不上。
    let Some((iface_dir, bus, iface)) = split_usb_interface(&dev_link) else {
        return info;
    };
    // 驱动绑在接口上，优先取接口目录的（端口目录未必有 driver 链接）
    let drv = driver_of(&iface_dir);
    info.driver = if drv.is_empty() {
        driver_of(&dev_link)
    } else {
        drv
    };
    info.usb_dev = bus;
    info.iface = iface;
    // 接口目录的父目录是 USB 设备目录（`2-1`），vid/pid 在那里
    let usb_dir = iface_dir.parent().unwrap_or(&iface_dir);
    info.vid = read_trim(&usb_dir.join("idVendor"));
    info.pid = read_trim(&usb_dir.join("idProduct"));
    info
}

/// 从 `/sys/class/tty/<tty>/device` 的解析结果里切出 USB 接口目录。
///
/// 返回 `(接口目录, bus, 接口号)`，例如
/// `/sys/.../usb2/2-1/2-1:1.2/ttyUSB2` → (`/sys/.../2-1:1.2`, `2-1`, `1.2`)。
///
/// 为什么不只看 `file_name()`：`device` 的指向随驱动/内核而变 —— 有的直接指向
/// 接口目录 `2-1:1.2`，有的指向接口下的端口目录 `2-1:1.2/ttyUSB2`
/// （内核 6.x 的 usb-serial 以后者居多）。所以从解析路径逐级往上找第一个
/// 长得像「USB 接口」的目录。
pub fn split_usb_interface(dev_link: &Path) -> Option<(PathBuf, String, String)> {
    for dir in dev_link.ancestors() {
        let name = match dir.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };
        // `2-1:1.2`：冒号前是 bus（`2-1`），冒号后是「配置.接口」（`1.2`）
        let Some((bus, iface)) = name.rsplit_once(':') else {
            continue;
        };
        if bus.is_empty() || !bus.contains('-') || !bus.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        if !iface.contains('.') || !iface.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        return Some((dir.to_path_buf(), bus.to_string(), iface.to_string()));
    }
    None
}

#[cfg(not(unix))]
pub fn describe(path: &str) -> PortInfo {
    PortInfo {
        path: path.to_string(),
        ..PortInfo::default()
    }
}

/// 非 tty 的 AT 通道按名字前缀归组（`mhi_1DUN0` / `mhi_1AT0` → `mhi_1`）。
///
/// MHI 的三套命名（上游 Go 版 `ports_unix.go:38-51`）：`mhi_*DUN*`、`mhi_*AT*`、
/// `wwan*at*`。同一台设备的多个通道前缀相同，去掉通道后缀就是稳定的分组键。
pub fn pseudo_group_for(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if let Some(rest) = base.strip_prefix("mhi_") {
        for marker in ["dun", "at", "uci_q", "qmi", "mbim"] {
            if let Some(idx) = rest.find(marker) {
                let head = &rest[..idx];
                if !head.is_empty() {
                    let head = head.trim_end_matches(['_', '-']);
                    return format!("mhi_{head}");
                }
            }
        }
        return format!("mhi_{rest}");
    }
    if base.starts_with("wwan") {
        if let Some(idx) = base.find("at") {
            return base[..idx].trim_end_matches(['_', '-']).to_string();
        }
        return base;
    }
    String::new()
}

/// `/sys/class/tty/<tty>/device` 的真实路径（USB 接口目录）。
#[cfg(unix)]
fn tty_device_dir(path: &str) -> Option<PathBuf> {
    let name = Path::new(path).file_name()?.to_str()?;
    let link = Path::new("/sys/class/tty").join(name).join("device");
    std::fs::canonicalize(link).ok()
}

/// 接口目录上绑的驱动名（`option` / `qcserial` / `cdc_acm` …）。
#[cfg(unix)]
fn driver_of(iface_dir: &Path) -> String {
    std::fs::read_link(iface_dir.join("driver"))
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default()
}

#[cfg(unix)]
fn read_trim(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// 这台设备「出现多久了」（秒）；取不到返回 `None`。
///
/// 用途是探活退避的**预热窗口**：刚插上的 5G 模组要几十秒才把 AT 口拉起来，
/// 这段时间里按指数退避（最长 5 分钟）会把新设备压死。上游 Go 版用 sysfs 设备
/// 目录的 mtime 算年龄（`module/netdev/ports_unix.go:177-190`），
/// 本函数同一口径：mtime 不受进程重启影响，比「进程启动时刻」可靠。
#[cfg(unix)]
pub fn device_age_secs(path: &str) -> Option<u64> {
    let name = Path::new(path).file_name()?.to_str()?;
    let link = Path::new("/sys/class/tty").join(name).join("device");
    let dir = std::fs::canonicalize(link).ok()?;
    // USB 接口目录的父目录是设备目录；非 USB（MHI 等）直接用自己
    let dev_dir = dir.parent().unwrap_or(&dir);
    let modified = std::fs::metadata(dev_dir).ok()?.modified().ok()?;
    modified.elapsed().ok().map(|d| d.as_secs())
}

#[cfg(not(unix))]
pub fn device_age_secs(_path: &str) -> Option<u64> {
    None
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
    use super::{describe, is_at_candidate, port_exists};
    use std::path::{Path, PathBuf};

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

    /// 不存在的设备节点不能 panic，字段留空（计划层按「无规则」处理）。
    #[test]
    fn describe_missing_port_is_empty_but_safe() {
        let info = describe("/dev/definitely-not-a-port-xyz");
        assert_eq!(info.path, "/dev/definitely-not-a-port-xyz");
        assert!(info.usb_dev.is_empty());
        assert!(info.iface.is_empty());
        assert!(info.vid.is_empty());
    }

    /// `device` 指向接口下的端口目录（内核 6.x 的 usb-serial）时必须仍能归到接口，
    /// 否则同一台模组的每个 AT 口会各自成组 —— 一台模组被登记成多台。
    #[test]
    fn split_usb_interface_accepts_port_subdir() {
        let got = super::split_usb_interface(Path::new(
            "/sys/devices/platform/soc/11200000.usb/usb2/2-1/2-1:1.2/ttyUSB2",
        ))
        .expect("端口目录必须能归到接口");
        assert_eq!(got.1, "2-1");
        assert_eq!(got.2, "1.2");
        assert_eq!(
            got.0,
            PathBuf::from("/sys/devices/platform/soc/11200000.usb/usb2/2-1/2-1:1.2"),
            "要返回接口目录本身，不是端口目录"
        );
    }

    /// 老布局：`device` 直接指向接口目录。
    #[test]
    fn split_usb_interface_accepts_iface_dir() {
        let got = super::split_usb_interface(Path::new("/sys/bus/usb/devices/1-1:1.2"))
            .expect("接口目录必须能认出来");
        assert_eq!(got.1, "1-1");
        assert_eq!(got.2, "1.2");
    }

    /// 非 USB（板载串口、根目录）不能误判成 USB 接口。
    #[test]
    fn split_usb_interface_rejects_non_usb() {
        assert!(
            super::split_usb_interface(Path::new("/sys/devices/platform/serial8250/tty/ttyS0"))
                .is_none()
        );
        assert!(super::split_usb_interface(Path::new("/")).is_none());
    }
}
