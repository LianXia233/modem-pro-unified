//! PCIe（MHI / wwan）模组设备：清单 / 软复位 / 绑驱动（`/api/pcie*`）。
//!
//! PCIe 模组没有「USB 插拔」这件事，设备清单与复位走单独一组接口。
//!
//! 简化模型说明：`catalog_models`（按 PCI VID 猜「可能是哪几个支持 PCIe 的机型」）暂缺——
//! Rust 侧暂无 PCIe 型号清单，该字段为空时直接省略。
//! 其余字段与判定（驱动名、网卡 / wwan 端口 / AT & 控制口归属、复位与绑驱动）对齐。

use serde::Serialize;

use super::sysfs;
use super::vendor::Vendor;

/// 已知的 5G 模组 PCIe 驱动：绑上这些驱动的设备**就是**模组。
///
///   · `mhi_pci_generic` / `mhi_pci`  高通 MHI（SDX55/65/72/75 全系）
///   · `mtk_t7xx` / `t7xx_pci`        联发科 T7xx（FM350-GL PCIe 版 / T700 系）
///   · `pcie_mhi`                     某些 BSP 里的老名字
///   · `fibo_mhi`                     广和通厂外树 MHI（Siriling/5G-Modem-Support
///     `fibocom_MHI`，`obj-m := fibo_mhi.o`，PKG_NAME `pcie_mhi_fb`）
///
/// ⚠️ 判据必须是「驱动」而不是「VID」（17cb 同时也是高通无线网卡 ath11k/ath12k 的 VID）。
pub const PCIE_MODEM_DRIVERS: &[&str] = &[
    "mhi_pci_generic",
    "mhi_pci",
    "pcie_mhi",
    "fibo_mhi",
    "mtk_t7xx",
    "t7xx_pci",
];

/// 芯片/模组厂的 PCI VID → 显示名（只在「没绑驱动」时用来提示）。
pub const PCIE_VENDOR_NAMES: &[(&str, &str)] = &[
    ("17cb", "高通 Qualcomm（MHI）"),
    ("14c3", "联发科 MediaTek（T7xx）"),
    ("105b", "富士康 Foxconn（T99W 系）"),
    ("2c7c", "移远 Quectel"),
    ("2cb7", "广和通 Fibocom"),
    ("1e0e", "芯讯通 SIMCom"),
    ("2dee", "美格 MeiG"),
    ("05c6", "高通（模组模式枚举）"),
    ("0e8d", "联发科（模组模式枚举）"),
];

/// 由 PCI VID 推厂商（认不出返回 `(Unknown, "")`，不猜）。
pub fn pcie_vendor_of(vid: &str) -> (Vendor, &'static str) {
    let v = vid.trim().to_lowercase();
    match v.as_str() {
        "2c7c" => (Vendor::Quectel, Vendor::Quectel.label()),
        "2cb7" | "1508" => (Vendor::Fibocom, Vendor::Fibocom.label()),
        "1e0e" => (Vendor::Simcom, Vendor::Simcom.label()),
        "2dee" => (Vendor::Meig, Vendor::Meig.label()),
        // 富士康没有独立的厂商标识（只给显示名，标识保持 unknown）
        "105b" => (Vendor::Unknown, "富士康 Foxconn（T99W 系）"),
        _ => {
            let name = PCIE_VENDOR_NAMES
                .iter()
                .find(|(k, _)| *k == v)
                .map(|(_, n)| *n)
                .unwrap_or("");
            (Vendor::Unknown, name)
        }
    }
}

/// 一个 PCIe 设备（只列「像 5G 模组」的）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct PciDevice {
    /// PCI 地址，如 `0000:01:00.0`
    pub bdf: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub driver: String,
    pub bound: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub class: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vendor_name: String,
    pub modem: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mhi: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub netdevs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub wwan_ports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub at_ports: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub qmi_ctrl: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mbim_dev: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 本平台支不支持 PCIe 复位（Windows 上不支持）。
pub fn reset_supported() -> bool {
    cfg!(unix)
}

/// 列出「像 5G 模组」的 PCIe 设备（含还没绑驱动的）。
#[cfg(unix)]
pub fn list_pci_modem_devices() -> Vec<PciDevice> {
    use std::path::Path;

    let root = Path::new("/sys/bus/pci/devices");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let netdevs = netdev_device_paths();
    let wwan_ports = wwan_class_ports();
    let at_cand = pcie_at_port_candidates();
    let qmi_cand = pcie_qmi_control_candidates();
    let mbim_cand = pcie_mbim_candidates();

    let mut out: Vec<PciDevice> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !e.path().is_dir() || !looks_like_bdf(&name) {
            continue;
        }
        let dir = root.join(&name);
        let vid = normalize_pci_id(&sysfs::read_line(&dir.join("vendor")));
        let pid = normalize_pci_id(&sysfs::read_line(&dir.join("device")));
        let driver = pci_driver_of(&dir);
        let cls = normalize_pci_id(&sysfs::read_line(&dir.join("class")));
        if !pcie_looks_like_modem(&driver, &vid, &cls) {
            continue;
        }
        let (vendor, vendor_name) = pcie_vendor_of(&vid);
        let mut dev = PciDevice {
            bdf: name.clone(),
            vid,
            pid,
            driver: driver.clone(),
            bound: !driver.is_empty(),
            class: cls,
            vendor: if vendor == Vendor::Unknown {
                String::new()
            } else {
                vendor.as_str().to_string()
            },
            vendor_name: vendor_name.to_string(),
            modem: true,
            mhi: pci_driver_is_mhi(&driver),
            ..PciDevice::default()
        };
        if !dev.bound {
            dev.note = "这块 PCIe 设备是 5G 模组，但**还没绑定驱动** —— 请查内核模块\
                （kmod-mhi-pci-generic / kmod-mtk-t7xx）是否装上；页面上的「绑定驱动」按钮也会帮它绑定。"
                .to_string();
        }
        out.push(dev);
    }
    out.sort_by(|a, b| a.bdf.cmp(&b.bdf));

    // 把网卡 / wwan 端口 / AT & 控制口分派到各自的 PCIe 设备
    let single = out.len() == 1;
    for dev in out.iter_mut() {
        dev.netdevs = assign_by_bdf(&dev.bdf, &netdevs, single, is_likely_mhi_face);
        dev.wwan_ports = assign_names_by_bdf(&dev.bdf, &wwan_ports, single);
        dev.at_ports = assign_names_by_bdf(&dev.bdf, &at_cand, single);
        if let Some(c) = assign_names_by_bdf(&dev.bdf, &qmi_cand, single).first() {
            dev.qmi_ctrl = format!("/dev/{c}");
        }
        if let Some(c) = assign_names_by_bdf(&dev.bdf, &mbim_cand, single).first() {
            dev.mbim_dev = format!("/dev/{c}");
        }
    }
    out
}

/// Windows 没有 sysfs ⇒ 空清单（接口照常返回 `supported=false`）。
#[cfg(not(unix))]
pub fn list_pci_modem_devices() -> Vec<PciDevice> {
    Vec::new()
}

/// PCIe 软复位：`remove` → 等一会儿 → `rescan`。失败如实返回错误。
#[cfg(unix)]
pub fn reset_pci_device(bdf: &str) -> Result<(), String> {
    let bdf = bdf.trim();
    if !looks_like_bdf(bdf) {
        return Err(format!("PCI 地址不合法：{bdf:?}"));
    }
    let dev_dir = std::path::Path::new("/sys/bus/pci/devices").join(bdf);
    if !dev_dir.exists() {
        return Err(format!("没有这块 PCIe 设备（{bdf}）"));
    }
    sysfs::write(&dev_dir.join("remove"), "1").map_err(|e| format!("摘除 {bdf} 失败：{e}"))?;
    log::info!("PCIe 设备 {bdf} 已摘除，等待后重新扫描");
    std::thread::sleep(std::time::Duration::from_secs(2));
    sysfs::write(std::path::Path::new("/sys/bus/pci/rescan"), "1")
        .map_err(|e| format!("重新扫描 PCIe 总线失败（设备可能没回来，必要时重启路由器）：{e}"))?;
    log::info!("PCIe 总线重新扫描完成（{bdf}）");
    Ok(())
}

#[cfg(not(unix))]
pub fn reset_pci_device(_bdf: &str) -> Result<(), String> {
    Err("本平台不支持 PCIe 软复位".to_string())
}

/// 把某个 PCIe 设备绑到指定驱动（驱动目录里得有 `bind` 文件）。
#[cfg(unix)]
pub fn bind_pci_driver(bdf: &str, driver: &str) -> Result<(), String> {
    let bdf = bdf.trim();
    let driver = driver.trim();
    if !looks_like_bdf(bdf) || driver.is_empty() {
        return Err(format!("参数不合法：{bdf:?} / {driver:?}"));
    }
    let bind = std::path::Path::new("/sys/bus/pci/drivers")
        .join(driver)
        .join("bind");
    if !bind.exists() {
        return Err(format!("驱动 {driver} 没装（找不到 {}）", bind.display()));
    }
    sysfs::write(&bind, bdf).map_err(|e| format!("绑定 {bdf} 到 {driver} 失败：{e}"))?;
    log::info!("PCIe 驱动已绑定：{bdf} → {driver}");
    Ok(())
}

#[cfg(not(unix))]
pub fn bind_pci_driver(_bdf: &str, _driver: &str) -> Result<(), String> {
    Err("本平台不支持绑定 PCIe 驱动".to_string())
}

/// PCIe 模组的 AT 候选口名（**不带 `/dev/`**，与 wwan 类目录写法一致）。
#[cfg(unix)]
pub fn pcie_at_port_candidates() -> Vec<String> {
    let mut out = Vec::new();
    out.extend(glob_dev("mhi_*DUN*"));
    out.extend(glob_dev("mhi_*AT*"));
    out.extend(glob_dev("wwan*at*"));
    dedup_sorted(out)
}

#[cfg(not(unix))]
pub fn pcie_at_port_candidates() -> Vec<String> {
    Vec::new()
}

/// QMI 控制口候选名（主机侧拨号器 / qmicli 用；不带 `/dev/`）。
#[cfg(unix)]
pub fn pcie_qmi_control_candidates() -> Vec<String> {
    let mut out = Vec::new();
    out.extend(glob_dev("mhi_*QMI*"));
    out.extend(glob_dev("wwan*qmi*"));
    out.extend(glob_dev("qcqmi*"));
    out.extend(glob_dev("mhi_uci_q*"));
    dedup_sorted(out)
}

#[cfg(not(unix))]
pub fn pcie_qmi_control_candidates() -> Vec<String> {
    Vec::new()
}

/// MBIM 控制口候选名（netifd 的 mbim 协议 / umbim 用；不带 `/dev/`）。
#[cfg(unix)]
pub fn pcie_mbim_candidates() -> Vec<String> {
    let mut out = Vec::new();
    out.extend(glob_dev("wwan*mbim*"));
    out.extend(glob_dev("mhi_*MBIM*"));
    dedup_sorted(out)
}

#[cfg(not(unix))]
pub fn pcie_mbim_candidates() -> Vec<String> {
    Vec::new()
}

/// PCIe 总线重新扫描路径（页面展示用）。
pub const PCI_RESCAN_PATH: &str = "/sys/bus/pci/rescan";

// ---------- 判定小工具 ----------

/// 目录名是不是 PCI 地址（`0000:01:00.0`；测试树里的 `0000-01-00.0` 也认）。
pub fn looks_like_bdf(name: &str) -> bool {
    let s = name.replace('-', ":");
    if s.len() < 12 || s.matches(':').count() != 2 || !s.contains('.') {
        return false;
    }
    s.chars().all(|c| {
        c.is_ascii_digit()
            || ('a'..='f').contains(&c)
            || ('A'..='F').contains(&c)
            || c == ':'
            || c == '.'
    })
}

/// 归一 sysfs 里的 `0x17cb` → `17cb`。
pub fn normalize_pci_id(s: &str) -> String {
    let s = s.trim().to_lowercase();
    s.strip_prefix("0x").unwrap_or(&s).to_string()
}

/// 当前绑的驱动名（没绑返回空）。
#[cfg(unix)]
fn pci_driver_of(dir: &std::path::Path) -> String {
    std::fs::canonicalize(dir.join("driver"))
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default()
}

/// 高通 / 广和通 MHI 系驱动（含厂外树 `fibo_mhi`）。
// 仅 unix 路径与单测使用：非 unix 平台别报「未用」。
#[cfg_attr(not(unix), allow(dead_code))]
fn pci_driver_is_mhi(driver: &str) -> bool {
    let d = driver.trim().to_lowercase();
    d.starts_with("mhi") || d == "pcie_mhi" || d == "fibo_mhi"
}

/// 是不是「模组专用」的 PCIe 驱动。
fn pci_driver_is_modem_driver(driver: &str) -> bool {
    let d = driver.trim().to_lowercase();
    PCIE_MODEM_DRIVERS.iter().any(|m| *m == d)
}

/// 判定一个 PCI 设备是不是 5G 模组。
///
///   · 绑着模组驱动（mhi*/t7xx）→ 是（无线网卡用的 ath11k/ath12k/mt76 不会命中）
///   · 没绑驱动、但 VID 在模组芯片厂表里 + class 是无线控制器（0x0b）→ 大概率是
pub fn pcie_looks_like_modem(driver: &str, vid: &str, class: &str) -> bool {
    if pci_driver_is_modem_driver(driver) {
        return true;
    }
    if !driver.is_empty() {
        return false;
    }
    let v = vid.trim().to_lowercase();
    if !PCIE_VENDOR_NAMES.iter().any(|(k, _)| *k == v) {
        return false;
    }
    let cls = normalize_pci_id(class);
    cls.starts_with("0b")
}

/// 是不是「模组交出来的数据网卡」名（MHI / rmnet / wwan）。
///
/// 覆盖三种命名来源：
///   · 树内 / 高通系：`wwan*`、`rmnet*`（移远厂外树 MHI 的数据口就叫
///     `rmnet_mhi0`，`luci-app-pcimodem` 的 init 脚本写死这个名字）；
///   · 树内 MHI：`mhi*`；
///   · 广和通厂外树 MHI（`fibo_mhi`）：数据口叫 `pcie_mhi*` / `pcie_swip*`
///     （`fibocom_MHI/src/devices/mhi_netdev.c` 的 `interface_name`）。
// 仅 unix 路径与单测使用：非 unix 平台别报「未用」。
#[cfg_attr(not(unix), allow(dead_code))]
fn is_likely_mhi_face(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("wwan")
        || n.starts_with("rmnet")
        || n.starts_with("mhi")
        || n.starts_with("pcie_mhi")
        || n.starts_with("pcie_swip")
}

/// 路径里有没有这个 PCI 地址（BDF 在 sysfs 路径里一定是完整的段）。
// 仅 unix 路径与单测使用：非 unix 平台别报「未用」。
#[cfg_attr(not(unix), allow(dead_code))]
fn path_has_bdf(path: &str, bdf: &str) -> bool {
    let bdf = bdf.trim();
    if bdf.is_empty() || path.is_empty() {
        return false;
    }
    path.contains(&format!("/{bdf}/")) || path.ends_with(&format!("/{bdf}"))
}

/// 网卡名优先级（wwan < usb/rndis < eth；桥接/聚合最后）。QMAP mux 排最前。
// 仅 unix 路径与单测使用：非 unix 平台别报「未用」。
#[cfg_attr(not(unix), allow(dead_code))]
fn netdev_rank(name: &str) -> i32 {
    let n = name.to_lowercase();
    if is_qmap_mux(&n) {
        -2
    } else if n.starts_with("wwan") {
        0
    } else if n.starts_with("usb") || n.starts_with("rndis") {
        1
    } else if n.starts_with("eth") {
        2
    } else if n.starts_with("br-") || n.starts_with("bond") {
        8
    } else {
        5
    }
}

/// 是不是 QMAP 的 mux（数据）口（`qmimux0` / 厂家 `wwan0_1` 这种「父名_muxid」）。
// 仅 unix 路径与单测使用：非 unix 平台别报「未用」。
#[cfg_attr(not(unix), allow(dead_code))]
fn is_qmap_mux(name: &str) -> bool {
    let n = name.to_lowercase();
    if n.is_empty() {
        return false;
    }
    if n.starts_with("qmimux") {
        return true;
    }
    match n.rfind('_') {
        Some(i) if i > 0 && i < n.len() - 1 => n[i + 1..].bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
}

/// `/sys/class/net` 里有 `device` 软链（真物理网卡）的：名字 → 解析后的路径。
#[cfg(unix)]
fn netdev_device_paths() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let root = std::path::Path::new("/sys/class/net");
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            if let Ok(real) = std::fs::canonicalize(root.join(e.file_name()).join("device")) {
                out.insert(
                    e.file_name().to_string_lossy().to_string(),
                    real.to_string_lossy().to_string(),
                );
            }
        }
    }
    out
}

/// `/sys/class/wwan` 下的端口名（`wwan0at0` / `wwan0qmi0` …）。
#[cfg(unix)]
fn wwan_class_ports() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/class/wwan") {
        for e in entries.flatten() {
            out.push(e.file_name().to_string_lossy().to_string());
        }
    }
    out.sort();
    out
}

/// 从「名字 → 真实路径」表里挑属于这个 BDF 的项。
#[cfg(unix)]
fn assign_by_bdf<F: Fn(&str) -> bool>(
    bdf: &str,
    paths: &std::collections::HashMap<String, String>,
    single: bool,
    want: F,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, p) in paths {
        if !want(name) {
            continue;
        }
        if path_has_bdf(p, bdf) || single {
            out.push(name.clone());
        }
    }
    out.sort_by(|a, b| {
        let (ra, rb) = (netdev_rank(a), netdev_rank(b));
        ra.cmp(&rb).then_with(|| a.cmp(b))
    });
    out
}

/// 从名字表里挑属于这个 BDF 的（wwan 端口的 `<name>/device` 解析后判断）。
#[cfg(unix)]
fn assign_names_by_bdf(bdf: &str, names: &[String], single: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if single {
            out.push(n.clone());
            continue;
        }
        let link = std::path::Path::new("/sys/class/wwan")
            .join(n)
            .join("device");
        let real = std::fs::canonicalize(&link)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| link.to_string_lossy().to_string());
        if path_has_bdf(&real, bdf) {
            out.push(n.clone());
        }
    }
    out.sort();
    out
}

/// `/dev` 下按模式找设备名（返回**不带 `/dev/`** 的名字）。
#[cfg(unix)]
fn glob_dev(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/dev") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if glob_match(pattern, &name) {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// 去重 + 排序。
// 仅 unix 路径与单测使用：非 unix 平台别报「未用」。
#[cfg_attr(not(unix), allow(dead_code))]
fn dedup_sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

/// 极简 glob：只支持 `*`（匹配任意字符序列）。
// 仅 unix 路径与单测使用：非 unix 平台别报「未用」。
#[cfg_attr(not(unix), allow(dead_code))]
fn glob_match(pattern: &str, s: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bdf_detection() {
        assert!(looks_like_bdf("0000:01:00.0"));
        assert!(looks_like_bdf("0000-01-00.0")); // 测试树写法
        assert!(!looks_like_bdf("0000:01:00")); // 少一个点
        assert!(!looks_like_bdf("usb1"));
    }

    #[test]
    fn pci_id_and_driver() {
        assert_eq!(normalize_pci_id("0x17CB"), "17cb");
        assert!(pci_driver_is_mhi("mhi_pci_generic"));
        assert!(pci_driver_is_mhi("fibo_mhi")); // 广和通厂外树 MHI
        assert!(pci_driver_is_modem_driver("mtk_t7xx"));
        assert!(pci_driver_is_modem_driver("fibo_mhi"));
        assert!(!pci_driver_is_modem_driver("ath11k"));
    }

    #[test]
    fn looks_like_modem_rules() {
        // 绑了模组驱动 ⇒ 是
        assert!(pcie_looks_like_modem("mhi_pci_generic", "17cb", "0x0b40"));
        // 绑了别的驱动（无线网卡）⇒ 不是
        assert!(!pcie_looks_like_modem("ath11k", "17cb", "0x0b40"));
        // 没绑驱动 + 模组芯片厂 VID + 无线控制器类 ⇒ 是
        assert!(pcie_looks_like_modem("", "17cb", "0x0b40"));
        // 没绑驱动但 VID 不在表里 ⇒ 不是
        assert!(!pcie_looks_like_modem("", "1234", "0x0b40"));
    }

    #[test]
    fn fibocom_mhi_faces_are_recognised() {
        // 广和通厂外树 MHI 的数据网卡名（mhi_netdev.c 的 interface_name）
        assert!(is_likely_mhi_face("pcie_mhi0"));
        assert!(is_likely_mhi_face("pcie_swip0"));
        // 移远厂外树 MHI / 树内命名照旧命中
        assert!(is_likely_mhi_face("rmnet_mhi0"));
        assert!(is_likely_mhi_face("wwan0"));
        assert!(is_likely_mhi_face("mhi_wwan0"));
        // 普通网卡不误判
        assert!(!is_likely_mhi_face("eth0"));
        assert!(!is_likely_mhi_face("pcie0"));
    }

    #[test]
    fn glob_and_mux() {
        assert!(glob_match("mhi_*DUN*", "mhi_DUN0"));
        assert!(glob_match("wwan*at*", "wwan0at0"));
        assert!(!glob_match("wwan*at*", "wwan0qmi0"));
        assert!(is_qmap_mux("qmimux0"));
        assert!(is_qmap_mux("wwan0_1"));
        assert!(!is_qmap_mux("wwan0"));
        assert!(netdev_rank("qmimux0") < netdev_rank("wwan0"));
        assert!(netdev_rank("wwan0") < netdev_rank("eth0"));
    }
}
