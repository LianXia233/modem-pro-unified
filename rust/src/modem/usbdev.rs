//! USB 模组设备枚举 / 热插拔事件 / 串口驱动补绑（`/api/usb`、`/api/usb/clear`、`/api/usb/bind`）。
//!
//! 只认「命中已知模组身份表」的 USB 设备，避免去动无关设备的驱动；
//! 枚举包含**还没交出串口**的设备（插上了但 ttyUSB 没出来，正是要补驱动的场景）。
//!
//! 简化模型说明：
//!   · 本进程一个 AT 口 = 一个模组，这里只做「按 VID:PID 认出 USB 模组」与设备清单；
//!   · 事件环形缓冲、补驱动（`new_id`）逻辑保留；自动插拔巡检与 `auto_bind` 触发在后台循环阶段接入。

use std::sync::Mutex;

use serde::Serialize;

use super::vendor::Vendor;

/// sysfs 里 USB 设备根目录（Linux/OpenWrt）。
#[cfg(unix)]
const USB_DEVICES_ROOT: &str = "/sys/bus/usb/devices";
/// 事件环形缓冲上限（最近 N 条）。
const EVENT_KEEP: usize = 60;

/// 一个厂商在 USB 上的身份（VID + 常见 PID）。
///
/// 用途是「快速筛掉无关设备」，不是安全边界：命中后还要看类码 / 能不能答 AT。
/// 宁可少写也不要乱写 —— 误判的代价是去动别人设备的驱动绑定。
pub struct ModemUsbFamily {
    pub vendor: Vendor,
    /// 四位十六进制小写
    pub vid: &'static str,
    /// 已知 PID（小写十六进制）；空 = 看 [`Self::vendor_only_ok`] / 类码
    pub pids: &'static [&'static str],
    pub note: &'static str,
    /// 这个 VID 是该厂商独占的吗（如鼎桥 3466、芯讯通 1e0e）
    pub vendor_only_ok: bool,
}

/// 已知模组的 USB 身份表（VID:PID → 厂商/系列）。
pub const MODEM_USB_FAMILIES: &[ModemUsbFamily] = &[
    // 移远 2c7c 是自有 VID，但底下挂高通 / 展锐 / MTK 三种平台的模组，PID 段各不一样 ⇒ 必须锁 PID
    ModemUsbFamily {
        vendor: Vendor::Quectel,
        vid: "2c7c",
        pids: &[
            // 高通平台
            "0121", "0122", "0125", "0127", "0133", "0191", "0195", "0296", "0306", "030b", "0435",
            "0452", "0455", "0512", "0620", "0800", "0801", "0802", // 展锐平台
            "0900", "0901", "0902", "0903", "0904", "6001", "6002", "6004", "6005", "6006", "6007",
            "6026", "8101", // MTK 平台
            "0700", "7001", "7003",
        ],
        note: "高通（EC20/EC25/RG500…）+ 展锐（RM500U/RG500U/EC200U…）+ MTK（0700/7001/7003）",
        vendor_only_ok: false,
    },
    ModemUsbFamily {
        vendor: Vendor::Fibocom,
        vid: "0e8d",
        pids: &["7003", "7005", "7101", "7126", "7127"],
        note: "MTK 平台（FM150/FM350 等）",
        vendor_only_ok: false,
    },
    ModemUsbFamily {
        vendor: Vendor::Fibocom,
        vid: "2cb7",
        pids: &["0a06", "0104", "0109", "0113"],
        note: "FM650-CN（展锐平台）/ FG150 / FM150 / FG101 等",
        vendor_only_ok: false,
    },
    ModemUsbFamily {
        vendor: Vendor::Fibocom,
        vid: "1508",
        pids: &["1000", "1001"],
        note: "NL668",
        vendor_only_ok: false,
    },
    ModemUsbFamily {
        vendor: Vendor::Fibocom,
        vid: "05c6",
        pids: &["9025"],
        note: "NL668（高通 VID）",
        vendor_only_ok: false,
    },
    ModemUsbFamily {
        vendor: Vendor::TDTech,
        vid: "3466",
        pids: &["3301"],
        note: "鼎桥 TD Tech / TDTECH Connect",
        vendor_only_ok: false,
    },
    // 芯讯通 VID 是厂商独占的：拿不到类码时（Windows 上常见）也得能认出来
    ModemUsbFamily {
        vendor: Vendor::Simcom,
        vid: "1e0e",
        pids: &[],
        note: "SIM8200/A7900 等",
        vendor_only_ok: true,
    },
    // SIM8200 在「QMI/Modem 模式」下会枚举成高通的 05c6:90db（05c6 是高通通用 VID，只能靠 PID）
    ModemUsbFamily {
        vendor: Vendor::Simcom,
        vid: "05c6",
        pids: &["90db"],
        note: "SIM8200（QMI 模式枚举）",
        vendor_only_ok: false,
    },
    ModemUsbFamily {
        vendor: Vendor::Meig,
        vid: "2dee",
        pids: &["4d22"],
        note: "SRM815/SRM825 等 5G 模组",
        vendor_only_ok: false,
    },
    ModemUsbFamily {
        vendor: Vendor::Meig,
        vid: "05c6",
        pids: &["f601"],
        note: "SLM750/SLM730（高通 VID）",
        vendor_only_ok: false,
    },
];

/// 归一 VID/PID 写法（大小写 / `VID_` 前缀 / 冒号 / 短横线都认，返回小写十六进制）。
pub fn split_vid_pid(s: &str) -> (String, String) {
    let up = s
        .trim()
        .to_uppercase()
        .replace("VID_", "")
        .replace("PID_", "");
    let up = up.replace(':', "&");
    let parts: Vec<&str> = up.split('&').collect();
    let get = |i: usize| -> String {
        parts
            .get(i)
            .map(|p| p.trim().to_lowercase())
            .filter(|p| is_hex_digits(p))
            .unwrap_or_default()
    };
    (get(0), get(1))
}

/// 是不是「全是十六进制数字」（空串不算）。
pub fn is_hex_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 判断某个 USB 设备是不是「已知模组」（保守优先）。
///
///   · VID 不在表里 ⇒ 不是；
///   · VID 命中且表里给了 PID ⇒ 必须 PID 也命中（如 0e8d 是联发科通用 VID，不能放太宽）；
///   · VID 命中、表里没给 PID 但是**厂商独占 VID**（1e0e）⇒ 算；
///   · 否则看接口类码里有没有模组常见的（厂商自定义 ff、MBIM/NCM/RNDIS/ECM）。
pub fn is_known_modem_usb(
    vid: &str,
    pid: &str,
    classes: &[String],
) -> Option<&'static ModemUsbFamily> {
    let (vid, pid) = split_vid_pid(&format!("{}:{}", vid.trim(), pid.trim()));
    for f in MODEM_USB_FAMILIES {
        if f.vid != vid {
            continue;
        }
        if !f.pids.is_empty() {
            if f.pids.iter().any(|p| *p == pid) {
                return Some(f);
            }
            continue;
        }
        if f.vendor_only_ok || has_modem_like_class(classes) {
            return Some(f);
        }
    }
    None
}

/// 接口类码里有没有「像模组」的那种（厂商自定义 ff / ECM / MBIM / RNDIS / NCM）。
fn has_modem_like_class(classes: &[String]) -> bool {
    use super::model::dial::classify_usb_classes;
    for c in classes {
        let cc = c.trim().to_lowercase();
        if cc.starts_with("ff") {
            return true;
        }
        if let Some(kind) = classify_usb_classes(std::slice::from_ref(&cc)) {
            if matches!(kind, "1" | "2" | "3" | "5") {
                return true;
            }
        }
    }
    false
}

/// 系统里的一个「像模组」的 USB 设备（可能还没交出串口）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct ModemUsbDevice {
    pub vid: String,
    pub pid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub family: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vendor_cn: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub product: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub serial: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bus_path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ttys: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drivers: Vec<String>,
    pub has_serial_driver: bool,
    pub known: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bind_note: String,
}

/// 事件类型。
pub const USB_EVENT_ADDED: &str = "added";
pub const USB_EVENT_REMOVED: &str = "removed";
pub const USB_EVENT_BOUND: &str = "bound";
pub const USB_EVENT_FAILED: &str = "failed";
pub const USB_EVENT_IGNORED: &str = "ignored";

/// 一条 USB 热插拔事件（给页面看「刚刚发生了什么」）。
#[derive(Debug, Clone, Serialize)]
pub struct UsbEvent {
    pub at: String,
    pub kind: String,
    /// `"0e8d:7126"`，未知时尽量留 VID/PID
    pub device: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modem_id: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
}

static USB_EVENTS: Mutex<Vec<UsbEvent>> = Mutex::new(Vec::new());

/// 记一条热插拔事件（内存环形缓冲，只留最近 [`EVENT_KEEP`] 条）。
pub fn note_usb_event(mut e: UsbEvent) {
    if e.at.is_empty() {
        e.at = crate::logging::timestamp();
    }
    let mut list = USB_EVENTS.lock().unwrap();
    list.push(e);
    if list.len() > EVENT_KEEP {
        let cut = list.len() - EVENT_KEEP;
        list.drain(0..cut);
    }
}

/// 最近的事件（新的在前）。
pub fn usb_events() -> Vec<UsbEvent> {
    let list = USB_EVENTS.lock().unwrap();
    list.iter().rev().cloned().collect()
}

/// 清空事件（页面上的「清空」）。
pub fn clear_usb_events() {
    USB_EVENTS.lock().unwrap().clear();
}

/// 页面用的总视图。
#[derive(Debug, Clone, Serialize)]
pub struct UsbHotplugView {
    /// windows / linux / openwrt
    pub platform: String,
    /// 是否会自动补驱动（Linux 才有效）；来自配置
    pub auto_bind: bool,
    /// 当前平台能不能补驱动
    pub can_bind: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bind_note: String,
    pub devices: Vec<ModemUsbDevice>,
    pub events: Vec<UsbEvent>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub advice: String,
}

/// 当前平台（openwrt 与 linux 分开：给的提示不一样）。
pub fn platform() -> &'static str {
    #[cfg(unix)]
    {
        if std::path::Path::new("/etc/openwrt_release").exists() {
            "openwrt"
        } else {
            "linux"
        }
    }
    #[cfg(not(unix))]
    {
        "windows"
    }
}

/// 本平台能不能自动补串口驱动（Linux/OpenWrt 可以）。
pub fn can_bind() -> bool {
    cfg!(unix)
}

/// 给页面的一句话说明。
pub fn bind_note() -> &'static str {
    if !can_bind() {
        return "Windows 上请到「AT 指令」页的串口下拉里手动指定命令口";
    }
    if platform() == "openwrt" {
        "OpenWrt：会自动 Modprobe option 并登记 VID/PID（new_id）；没装驱动时先 opkg install kmod-usb-serial-option"
    } else {
        "Linux：会自动 Modprobe option/usbserial 并登记 VID/PID（new_id），把 ttyUSB 变出来"
    }
}

/// 没驱动时该装什么（OpenWrt 与桌面 Linux 说法不同）。
pub fn serial_package_hint() -> &'static str {
    if platform() == "openwrt" {
        "OpenWrt 上先装驱动：opkg update && opkg install kmod-usb-serial-option"
    } else {
        "把内核的 usb-serial 模块装上（kmod-usb-serial-option / usbserial）"
    }
}

/// 组装页面视图（设备枚举失败也要能返回，页面得有东西看）。
pub fn usb_hotplug_info(auto_bind: bool) -> UsbHotplugView {
    let devices = list_modem_usb_devices();
    let mut advice = String::new();
    // 只在**能自动补驱动**的平台上提醒「插上了却没串口」（Windows 注册表会留已拔掉的记录）
    if can_bind()
        && devices
            .iter()
            .any(|d| d.known && d.ttys.is_empty() && !d.has_serial_driver)
    {
        advice = format!("有模组插上了但还没交出串口：{}", serial_package_hint());
    }
    UsbHotplugView {
        platform: platform().to_string(),
        auto_bind,
        can_bind: can_bind(),
        bind_note: bind_note().to_string(),
        devices,
        events: usb_events(),
        advice,
    }
}

/// 枚举系统里「像模组」的 USB 设备（**包含还没有串口的**）。
///
/// 只看命中身份表的设备：别的 USB 设备不进这个列表，免得页面上的「补驱动」按钮误伤它们。
#[cfg(unix)]
pub fn list_modem_usb_devices() -> Vec<ModemUsbDevice> {
    use std::collections::HashMap;
    use std::path::Path;

    // 串口 → USB 设备目录名（如 1-1），用于把 /dev/ttyUSB3 归到设备下
    let mut tty_by_dev: HashMap<String, Vec<String>> = HashMap::new();
    for port in super::ports_list() {
        if let Some(dir) = usb_device_dir(&port) {
            if let Some(base) = dir.file_name().map(|s| s.to_string_lossy().to_string()) {
                tty_by_dev.entry(base).or_default().push(port);
            }
        }
    }

    let Ok(entries) = std::fs::read_dir(USB_DEVICES_ROOT) else {
        return Vec::new();
    };
    let mut out: Vec<ModemUsbDevice> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains(':') {
            continue; // 接口目录（1-1:1.0）/ hub
        }
        let dir = Path::new(USB_DEVICES_ROOT).join(&name);
        // ⚠️ 判据是「能不能读到 idVendor/idProduct」，**不要**先用 is_dir() 过滤：
        // /sys/bus/usb/devices 下的条目全是符号链接，read_dir 的 is_dir() 对它们返回 false。
        let vid = read_sysfs_id(&dir.join("idVendor"));
        let pid = read_sysfs_id(&dir.join("idProduct"));
        if vid.is_empty() || pid.is_empty() {
            continue;
        }
        let mut dev = ModemUsbDevice {
            vid: vid.clone(),
            pid: pid.clone(),
            bus_path: name.clone(),
            product: read_sysfs_line(&dir.join("product")),
            serial: read_sysfs_line(&dir.join("serial")),
            ttys: tty_by_dev.get(&name).cloned().unwrap_or_default(),
            ..ModemUsbDevice::default()
        };
        let mut drivers: Vec<String> = Vec::new();
        if let Ok(ifaces) = std::fs::read_dir(&dir) {
            for ie in ifaces.flatten() {
                let iname = ie.file_name().to_string_lossy().to_string();
                if !iname.contains(':') {
                    continue;
                }
                let idir = dir.join(&iname);
                if !is_dir_or_symlink(&idir) {
                    continue;
                }
                if let Some(code) = usb_interface_code(&idir) {
                    dev.classes.push(code);
                }
                if let Some(drv) = interface_driver(&idir) {
                    drivers.push(drv);
                }
            }
        }
        dev.classes = super::model::dial::reduce_usb_codes(&dev.classes);
        dev.drivers = dedup_strings(drivers);
        dev.has_serial_driver = has_serial_driver_name(&dev.drivers) || !dev.ttys.is_empty();

        // ① 厂商身份表命中；② 否则看端口规则（QModem 登记过的 VID:PID 也算模组）——
        //    否则「按 VID:PID 绑定命令口」的页面上根本选不到这台设备。
        match is_known_modem_usb(&vid, &pid, &dev.classes) {
            Some(fam) => {
                dev.known = true;
                dev.family = fam.vendor.as_str().to_string();
                dev.vendor_cn = fam.vendor.label().to_string();
            }
            None => match crate::modem::ports::lookup(&vid, &pid) {
                Some(rule) => {
                    dev.known = true;
                    dev.family = rule.model.clone();
                    dev.vendor_cn = "端口规则登记".to_string();
                }
                None => continue,
            },
        }
        out.push(dev);
    }
    out.sort_by(|a, b| a.bus_path.cmp(&b.bus_path));
    out
}

/// Windows 拿不到 sysfs ⇒ 设备清单为空（接口照常返回，`can_bind=false`）。
#[cfg(not(unix))]
pub fn list_modem_usb_devices() -> Vec<ModemUsbDevice> {
    Vec::new()
}

/// 从 AT 口反查它所属 USB 设备的身份（识别引擎的输入之一）。
///
/// 走的是 sysfs 同一套链路：`/sys/class/tty/<tty>/device` 一路向上找到
/// USB 设备目录，读 `idVendor` / `idProduct` / `product`，再把该目录下的
/// 接口号（`1-1:1.2` → `1.2`）收集起来 —— 与 QModem 端口规则的 `include`
/// 同一口径，后续判断「这个口能不能当 AT 口」直接用得上。
///
/// 取不到（PCIe 模组、Windows、纯串口设备）返回 `None`，识别引擎会退回
/// 只靠 AT 应答判断，不影响功能。
#[cfg(unix)]
pub fn identity_for_tty(tty: &str) -> Option<crate::modem::detect::UsbIdentity> {
    let dir = usb_device_dir(tty)?;
    let vid = read_sysfs_id(&dir.join("idVendor"));
    let pid = read_sysfs_id(&dir.join("idProduct"));
    if vid.is_empty() || pid.is_empty() {
        return None;
    }
    let mut interfaces: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            // 接口目录形如 `1-1:1.2`；设备目录本身（`1-1`）不含冒号
            if let Some(idx) = name.rfind(':') {
                let tail = &name[idx + 1..];
                if tail.contains('.') {
                    interfaces.push(tail.to_string());
                }
            }
        }
    }
    interfaces.sort();
    interfaces.dedup();
    Some(crate::modem::detect::UsbIdentity {
        vid,
        pid,
        product: read_sysfs_line(&dir.join("product")),
        bus_path: dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        interfaces,
    })
}

/// Windows：没有 sysfs，识别只靠 AT 应答。
#[cfg(not(unix))]
pub fn identity_for_tty(_tty: &str) -> Option<crate::modem::detect::UsbIdentity> {
    None
}

/// 列出「端口规则要求补 option 驱动、但还没出串口」的 USB 设备。
///
/// 与 [`list_modem_usb_devices`] 的区别：这里**不要求**设备在本模块的厂商身份表里，
/// 只看 QModem 端口规则（`option_driver=1`）。用途是发现流程开始前先把 tty 建出来 ——
/// QModem 的 `modem_scand.c:570` 也是在枚举接口**之前**写 `option1/new_id`。
#[cfg(unix)]
pub fn option_bind_candidates() -> Vec<ModemUsbDevice> {
    use std::path::Path;

    let Ok(entries) = std::fs::read_dir(USB_DEVICES_ROOT) else {
        return Vec::new();
    };
    // 串口枚举只做一次：每个设备的 tty 归属都在同一份名单里查
    let ports = super::ports_list();
    let mut tty_owner: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for p in &ports {
        if let Some(base) =
            usb_device_dir(p).and_then(|d| d.file_name().map(|s| s.to_string_lossy().to_string()))
        {
            tty_owner.insert(base, p.clone());
        }
    }
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains(':') {
            continue;
        }
        let dir = Path::new(USB_DEVICES_ROOT).join(&name);
        let vid = read_sysfs_id(&dir.join("idVendor"));
        let pid = read_sysfs_id(&dir.join("idProduct"));
        if vid.is_empty() || pid.is_empty() {
            continue;
        }
        let Some(rule) = crate::modem::ports::lookup(&vid, &pid) else {
            continue;
        };
        if !rule.option_driver {
            continue;
        }
        // 已经有 tty 就不用补
        if tty_owner.contains_key(&name) {
            continue;
        }
        out.push(ModemUsbDevice {
            vid,
            pid,
            bus_path: name.clone(),
            product: read_sysfs_line(&dir.join("product")),
            serial: read_sysfs_line(&dir.join("serial")),
            known: true,
            family: rule.model.clone(),
            vendor_cn: "端口规则登记（需补 option 驱动）".to_string(),
            ..ModemUsbDevice::default()
        });
    }
    out
}

/// 非 unix：没有 sysfs，不补驱动。
#[cfg(not(unix))]
pub fn option_bind_candidates() -> Vec<ModemUsbDevice> {
    Vec::new()
}

/// 软重插：把 USB 设备的 `authorized` 置 0 再置 1，让内核重新枚举。
///
/// 用途只有一个：设备明明在、却一个 AT 口都认不出（固件跑飞 / 枚举卡死）。
/// 这是**破坏性**操作（数据面会短暂断开），调用方必须自带开关与冷却，
/// 不能放进常规巡检路径。
#[cfg(unix)]
pub fn reauthorize(bus_path: &str) -> Result<(), String> {
    let name = bus_path.trim();
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return Err(format!("USB 设备名不合法：{bus_path:?}"));
    }
    let path = std::path::Path::new(USB_DEVICES_ROOT)
        .join(name)
        .join("authorized");
    if !path.exists() {
        return Err(format!(
            "{} 不存在（内核不支持 authorized？）",
            path.display()
        ));
    }
    std::fs::write(&path, b"0").map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::write(&path, b"1").map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn reauthorize(_bus_path: &str) -> Result<(), String> {
    Err("本平台不支持 USB 软重插".to_string())
}

/// 给一个「插上了但没有串口」的模组补上 usb-serial 驱动。
///
/// 返回一句给人看的结果说明（成功/失败都会写进 USB 事件）。内核枚举 tty 要几十到几百毫秒。
#[cfg(unix)]
pub fn ensure_serial_tty(dev: &ModemUsbDevice) -> Result<String, String> {
    let (driver, new_id) = match usb_serial_new_id_path() {
        Some(p) => p,
        None => {
            // 没有 usb-serial 驱动 ⇒ 先 Modprobe
            modprobe("option").or_else(|_| modprobe("usbserial"))?;
            usb_serial_new_id_path().ok_or_else(|| {
                format!(
                    "系统里没有 usb-serial 驱动（new_id 路径不存在）。{}",
                    serial_package_hint()
                )
            })?
        }
    };
    let line = format!("{} {}", dev.vid, dev.pid);
    std::fs::write(&new_id, line.as_bytes())
        .map_err(|e| format!("写 {} 失败（需要 root 权限？）: {e}", new_id.display()))?;
    Ok(format!("已让 {driver} 接管 {line}（{}）", dev.bus_path))
}

/// 非 unix：不支持补驱动。
#[cfg(not(unix))]
pub fn ensure_serial_tty(_dev: &ModemUsbDevice) -> Result<String, String> {
    Err(bind_note().to_string())
}

/// usb-serial 驱动目录里的 `new_id` 文件（优先 option）。
#[cfg(unix)]
fn usb_serial_new_id_path() -> Option<(String, std::path::PathBuf)> {
    let root = std::path::Path::new("/sys/bus/usb-serial/drivers");
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    let pick = dirs
        .iter()
        .find(|d| {
            d.file_name()
                .map(|n| n.to_string_lossy().starts_with("option"))
                .unwrap_or(false)
        })
        .or_else(|| dirs.first())?;
    let new_id = pick.join("new_id");
    if !new_id.exists() {
        return None;
    }
    Some((
        pick.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        new_id,
    ))
}

/// 加载内核模块（失败时把 stderr 带出来）。
#[cfg(unix)]
fn modprobe(module: &str) -> Result<(), String> {
    let out = std::process::Command::new("modprobe").arg(module).output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if msg.is_empty() {
                Err(format!("modprobe {module} 失败"))
            } else {
                Err(format!("modprobe {module} 失败: {msg}"))
            }
        }
        Err(e) => Err(format!("modprobe {module} 失败: {e}")),
    }
}

// ---------- sysfs 小工具 ----------

#[cfg(unix)]
fn read_sysfs_line(path: &std::path::Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(s) => s.lines().next().unwrap_or("").trim().to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(unix)]
fn read_sysfs_hex(path: &std::path::Path) -> String {
    let s = read_sysfs_line(path).to_lowercase();
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::new();
    }
    format!("{s:0>2}")
}

#[cfg(unix)]
fn read_sysfs_id(path: &std::path::Path) -> String {
    let s = read_sysfs_line(path).to_lowercase();
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::new();
    }
    s
}

/// 目录（或指向目录的符号链接）就算 true（sysfs 里两者混着来）。
#[cfg(unix)]
fn is_dir_or_symlink(path: &std::path::Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// 读一个接口目录的类码，拼成 `02/06/00`（读不到返回 None）。
#[cfg(unix)]
fn usb_interface_code(dir: &std::path::Path) -> Option<String> {
    let mut class = read_sysfs_hex(&dir.join("bInterfaceClass"));
    let mut sub = read_sysfs_hex(&dir.join("bInterfaceSubClass"));
    let mut proto = read_sysfs_hex(&dir.join("bInterfaceProtocol"));
    if class.is_empty() {
        class = read_sysfs_hex(&dir.join("bDeviceClass"));
        sub = read_sysfs_hex(&dir.join("bDeviceSubClass"));
        proto = read_sysfs_hex(&dir.join("bDeviceProtocol"));
    }
    if class.is_empty() {
        return None;
    }
    Some(format!("{class}/{sub}/{proto}"))
}

/// 接口绑定的驱动名（没绑定返回 None）。
#[cfg(unix)]
fn interface_driver(iface_dir: &std::path::Path) -> Option<String> {
    let real = std::fs::canonicalize(iface_dir.join("driver")).ok()?;
    real.file_name().map(|s| s.to_string_lossy().to_string())
}

/// 从一个 sysfs 路径往上找含 idVendor 的目录（= USB 设备目录）。
#[cfg(unix)]
fn usb_ancestor_dir(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut real = std::fs::canonicalize(path).ok()?;
    for _ in 0..8 {
        if real.join("idVendor").exists() {
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

/// 串口 → USB 设备目录（往上逐层找到含 idVendor 的那层）。
#[cfg(unix)]
fn usb_device_dir(port: &str) -> Option<std::path::PathBuf> {
    let name = std::path::Path::new(port.trim())
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.is_empty() || name == "." {
        return None;
    }
    let link = std::path::Path::new("/sys/class/tty")
        .join(&name)
        .join("device");
    usb_ancestor_dir(&link)
}

/// 驱动名里有没有 usb-serial 系（option / usbserial / qcserial / cdc_acm）。
#[cfg(unix)]
fn has_serial_driver_name(drivers: &[String]) -> bool {
    drivers
        .iter()
        .any(|d| matches!(d.as_str(), "option" | "usbserial" | "qcserial" | "cdc_acm"))
}

/// 去空 + 去重 + 排序。
#[cfg(unix)]
fn dedup_strings(input: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in input {
        if s.is_empty() || out.contains(&s) {
            continue;
        }
        out.push(s);
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_vid_pid_forms() {
        assert_eq!(split_vid_pid("2c7c:0800"), ("2c7c".into(), "0800".into()));
        assert_eq!(
            split_vid_pid("VID_2C7C&PID_0800"),
            ("2c7c".into(), "0800".into())
        );
        assert_eq!(split_vid_pid("2C7C"), ("2c7c".into(), String::new()));
        // 非十六进制 ⇒ 空
        assert_eq!(split_vid_pid("zzzz:0800"), (String::new(), "0800".into()));
    }

    #[test]
    fn known_modem_by_pid() {
        // 移远展锐平台 PID（曾漏掉的那批）
        let f = is_known_modem_usb("2C7C", "0900", &[]).expect("2c7c:0900 应命中");
        assert_eq!(f.vendor, Vendor::Quectel);
        // 联发科通用 VID 必须靠 PID
        assert!(is_known_modem_usb("0e8d", "7126", &[]).is_some());
        assert!(is_known_modem_usb("0e8d", "9999", &[]).is_none());
        // 广和通自有 VID（FM650-CN）
        assert!(is_known_modem_usb("2cb7", "0a06", &[]).is_some());
    }

    #[test]
    fn vendor_only_ok_needs_class_when_no_pid() {
        // 芯讯通 1e0e 是独占 VID：没类码也认
        assert!(is_known_modem_usb("1e0e", "9011", &[]).is_some());
        // 05c6 这种通用 VID 没给 PID 就不认
        assert!(is_known_modem_usb("05c6", "0000", &[]).is_none());
    }

    #[test]
    fn modem_like_class_detection() {
        assert!(has_modem_like_class(&["ff/00/00".to_string()]));
        assert!(has_modem_like_class(&["02/0e/00".to_string()]));
        assert!(!has_modem_like_class(&["08/06/50".to_string()]));
    }

    #[test]
    fn events_newest_first_and_bounded() {
        clear_usb_events();
        for i in 0..(EVENT_KEEP + 5) {
            note_usb_event(UsbEvent {
                at: String::new(),
                kind: USB_EVENT_ADDED.into(),
                device: format!("2c7c:080{i}"),
                model: String::new(),
                note: String::new(),
                modem_id: None,
                ports: Vec::new(),
            });
        }
        let ev = usb_events();
        assert_eq!(ev.len(), EVENT_KEEP);
        // 新的在前
        assert!(ev[0].device.starts_with("2c7c:080"));
        assert!(ev[0].at.contains('T'), "时间戳应为 RFC3339: {}", ev[0].at);
        clear_usb_events();
        assert!(usb_events().is_empty());
    }
}
