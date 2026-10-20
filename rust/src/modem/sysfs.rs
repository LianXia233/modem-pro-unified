//! sysfs 小工具：读 USB / PCI 设备的属性文件。
//!
//! 这些 helper 原本在 `usbdev.rs`、`pcidev.rs`、`model/dial.rs` 里各抄了一份
//! （`read_sysfs_line` / `read_sysfs_hex` / `usb_ancestor_dir` …），
//! 抄的过程中已经开始漂移（返回值 `String` vs `Option<String>`、
//! `format!("{s:0>2}")` vs `format!("{:0>2}", s)`），
//! 判定口径一旦分叉，同一条错误结论会散在三个模块里各自成立 —— 故收拢到此处。
//!
//! 行为约定（与上游 Go 版 `module/sysfs`、QModem `modem_scand.c` 对齐）：
//!   · 读不到 / 读空一律返回空串或 `None`，**不 panic、不报错**：
//!     枚举设备时「这个文件不存在」是常态（比如没绑定的接口没有 `driver`）。
//!   · 只取首行：sysfs 的属性文件是「一行一个值」。
//!   · 所有函数都刻意**不带** `#[cfg(unix)]`：`std::fs` 在宿主 Windows 上
//!     一样能编译，只是运行时读不到内容（返回空），宿主开发自测不受影响。

use std::path::{Path, PathBuf};

/// 读 sysfs 文本文件的首行（sysfs 的「一行一个值」）。
pub fn read_line(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(s) => s.lines().next().unwrap_or("").trim().to_string(),
        Err(_) => String::new(),
    }
}

/// 读 sysfs 里的一个十六进制**字节**值（归一成两位小写）。
///
/// ⚠️ 只用于 bInterfaceClass 这类单字节属性 —— VID/PID 是 **4 位**，
/// 必须用 [`read_hex_id`]（实测踩过：用错函数 VID/PID 永远是空）。
pub fn read_hex_byte(path: &Path) -> String {
    let s = read_line(path).to_lowercase();
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::new();
    }
    format!("{s:0>2}")
}

/// 读 sysfs 里的 USB ID（idVendor / idProduct = 4 位十六进制，小写归一）。
pub fn read_hex_id(path: &Path) -> String {
    let s = read_line(path).to_lowercase();
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::new();
    }
    s
}

/// 写 sysfs 属性文件（`authorized` / `bind` / `remove` 这类）。
///
/// 调用方负责错误呈现；这里不做任何兜底。
pub fn write(path: &Path, val: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.write_all(val.as_bytes())
}

/// 目录（或指向目录的符号链接）就算 true（sysfs 里两者混着来）。
pub fn is_dir_or_symlink(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// 读一个接口目录的类码，拼成 `02/06/00`（读不到返回 `None`）。
///
/// 接口目录优先用 bInterface*；设备目录只有 bDevice*（非复合设备）。
pub fn interface_code(dir: &Path) -> Option<String> {
    let mut class = read_hex_byte(&dir.join("bInterfaceClass"));
    let mut sub = read_hex_byte(&dir.join("bInterfaceSubClass"));
    let mut proto = read_hex_byte(&dir.join("bInterfaceProtocol"));
    if class.is_empty() {
        class = read_hex_byte(&dir.join("bDeviceClass"));
        sub = read_hex_byte(&dir.join("bDeviceSubClass"));
        proto = read_hex_byte(&dir.join("bDeviceProtocol"));
    }
    if class.is_empty() {
        return None;
    }
    Some(format!("{class}/{sub}/{proto}"))
}

/// 接口绑定的驱动名（没绑定返回 `None`）。
pub fn interface_driver(iface_dir: &Path) -> Option<String> {
    let real = std::fs::canonicalize(iface_dir.join("driver")).ok()?;
    real.file_name().map(|s| s.to_string_lossy().to_string())
}

/// USB 接口目录名（`1-1:1.0`；测试/Windows 退化写法 `1-1.0` 也认）。
pub fn is_interface_dir_name(name: &str) -> bool {
    name.contains(':') || (name.contains('-') && name.contains('.'))
}

/// 设备目录下各接口的类码（`02/06/00`），顺序即 sysfs 枚举序。
///
/// 没有接口级类码时（非复合设备 / 老内核不放）退回设备级类码。
/// 去重归并交给调用方（`model::dial::reduce_usb_codes` 有项目特定的合并口径）。
pub fn usb_class_codes(dev_dir: &Path) -> Vec<String> {
    let mut codes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dev_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() || !is_interface_dir_name(&name) {
                continue;
            }
            if let Some(code) = interface_code(&e.path()) {
                codes.push(code);
            }
        }
    }
    if codes.is_empty() {
        if let Some(code) = interface_code(dev_dir) {
            codes.push(code);
        }
    }
    codes
}

/// 从一个 sysfs 路径往上找含 `marker` 的目录，最多 8 层。
///
/// `marker = "idVendor"` 即「USB 设备目录」（依据 Go `module/netdev/ports_unix.go`：
/// 那边同样是从 tty 的 `device` 软链逐级上溯找 `idVendor`）。
pub fn ancestor_with(path: &Path, marker: &str) -> Option<PathBuf> {
    let mut real = std::fs::canonicalize(path).ok()?;
    for _ in 0..8 {
        if real.join(marker).exists() {
            return Some(real);
        }
        let parent = real.parent()?;
        if parent == real {
            break;
        }
        real = parent.to_path_buf();
    }
    None
}

/// AT 口 → USB 设备目录（往上逐层找到含 idVendor 的那层）。
///
/// 返回 `None` 表示这个口不是 USB 串口（主板物理口 / 蓝牙虚拟口 / 名字不是 tty*）。
pub fn tty_usb_device_dir(port: &str) -> Option<PathBuf> {
    let name = Path::new(port.trim())
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.is_empty() || name == "." {
        return None;
    }
    let link = Path::new("/sys/class/tty").join(&name).join("device");
    ancestor_with(&link, "idVendor")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_hex_byte_rejects_wrong_width() {
        let dir = std::env::temp_dir().join(format!("mp-sysfs-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("bInterfaceClass");
        std::fs::write(&p, "06\n").unwrap();
        assert_eq!(read_hex_byte(&p), "06");
        std::fs::write(&p, "0e8d").unwrap();
        assert_eq!(read_hex_byte(&p), ""); // 4 位是 ID，不是字节
        std::fs::write(&p, "zz").unwrap();
        assert_eq!(read_hex_byte(&p), "");
        std::fs::write(&p, "").unwrap();
        assert_eq!(read_hex_byte(&p), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_hex_id_needs_exactly_four() {
        let dir = std::env::temp_dir().join(format!("mp-sysfs-id-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("idVendor");
        std::fs::write(&p, "0E8D\n").unwrap();
        assert_eq!(read_hex_id(&p), "0e8d");
        std::fs::write(&p, "e8d").unwrap();
        assert_eq!(read_hex_id(&p), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_line_handles_missing_file() {
        assert_eq!(read_line(Path::new("/nonexistent/mp-no-such")), "");
    }
}
