//! 内置拨号器（不依赖任何外部二进制）。
//!
//! ## 结构
//!
//! ```text
//! native/
//!   mod.rs        计划（NativePlan）+ 会话门面（Session）+ 链路选择
//!   qmi_link.rs   QMI 传输与 WDS 数据呼叫
//!   mbim_link.rs  MBIM 传输与 Basic Connect
//!   vendors/      四家厂商策略（支持哪些链路 / 优先级 / 开关）
//! ```
//!
//! 协议实现在两个 `*_link` 里共用，厂商差异只有 `vendors/<厂商>.rs` 的
//! `Caps` —— 与上游 `module/<vendor>` 的分法同构，但按「数据面」而不是
//! 「AT 方言」切分。
//!
//! ## 它做什么
//!
//! 只做**数据呼叫的生命周期**，与外部拨号器的职责边界一致：
//!
//! 1. 打开 `/dev/cdc-wdmN`（从数据网卡的 sysfs 反查，多模组不会串）；
//! 2. QMI：`CTL Sync` → `Get Client ID` → `WDS Start`（v4 service 1 / v6 0x11）；
//!    MBIM：`OPEN` → `CONNECT(activate)` → `IP_CONFIGURATION`；
//! 3. 退出时对称拆除（QMI 发 `Stop` 并释放 client id；MBIM 去激活后 `CLOSE`）。
//!
//! 地址/DNS/默认路由**不在**这里配：`netprov` 建的 `modem<id>` 接口仍由 netifd
//! 的 `modem_pro`（udhcpc + odhcp6c）取址 —— 与外部拨号器时代完全一样，
//! 所以切换后端不会改变接口形态，也不会和 netifd 抢同一张网卡。
//!
//! ## 为什么值得做
//!
//! - 外部二进制要跟固件/内核版本对齐（qmi_wwan 的 raw_ip、QMAP、GobiNet 差异），
//!   出问题只能等厂商更新；
//! - 状态能直接回给 `/api/dial`（handle / 地址 / MTU / 最后错误），排障不用猜；
//! - 参数（APN / family / profile）与配置强绑定，不会被拨号器自己的默认值覆盖。
//!
//! ## 接入方式
//!
//! 原生拨号是**唯一**路径（`[net] host_dial_bin = "native"`，空值/`rust`/`builtin`
//! 同义）。控制口支持四种驱动布局：
//!
//! | 驱动 | 控制口 | 说明 |
//! | --- | --- | --- |
//! | `qmi_wwan*` / `cdc_mbim` | `/dev/cdc-wdmN` | USB 主力路径 |
//! | GobiNet | `/dev/GobiQMIx` | 与 cdc-wdm 同一套 QMUX 帧 |
//! | `pcie_mhi` | `/dev/mhi_QMIx` | PCIe / MHI，帧格式相同，端点类型不同 |
//! | 主线 MHI（`mhi_net` / `mhi_wwan_*`） | `/dev/wwan*`（`/sys/class/wwan`） | generic MHI：裸 IP、无聚合、BindMhi（无 mux id） |
//!
//! RNDIS 模组（FM350 / RW350）不走这里：它们没有 QMI 控制口，
//! 数据面由模组侧 AT 拨号完成。

pub mod qmi_link;
pub mod vendors;

#[cfg(unix)]
pub mod mbim_link;

use std::path::{Path, PathBuf};

use crate::modem::vendor::Vendor;
use crate::runtime::dialer::PlanError;

use super::qmi;
use vendors::{Caps, DataLink};

/// 控制口的驱动类型（决定端点类型，以及要不要跳过 WDA）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// `qmi_wwan` / `qmi_wwan_q` / `qmi_wwan_f` / `cdc_mbim`：`/dev/cdc-wdmN`。
    CdcWdm,
    /// GobiNet：`/dev/GobiQMIx`。驱动自己处理 QMAP 数据格式。
    GobiNet,
    /// `pcie_mhi`：`/dev/mhi_QMIx`（PCIe 模组）。
    Mhi,
    /// 主线 MHI（`mhi_net` / `mhi_wwan_qmi` / `mhi_wwan_mbim`）：
    /// `/dev/wwan*`，控制口类型从 `/sys/class/wwan/*/type` 读出。
    /// 固件侧按「裸 IP、无聚合」配数据格式，端点绑定不带 mux id。
    MhiGeneric,
}

/// wwan 类控制口的协议类型（`/sys/class/wwan/*/type` 的内容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WwanProto {
    Qmi,
    Mbim,
}

impl WwanProto {
    pub fn as_str(self) -> &'static str {
        match self {
            WwanProto::Qmi => "qmi",
            WwanProto::Mbim => "mbim",
        }
    }

    /// sysfs `type` 文件里的原文（quectel-CM `cm_mhi_detect` 按整行精确比对）。
    fn sysfs_name(self) -> &'static str {
        match self {
            WwanProto::Qmi => "QMI",
            WwanProto::Mbim => "MBIM",
        }
    }
}

impl ControlKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ControlKind::CdcWdm => "cdc-wdm",
            ControlKind::GobiNet => "gobinet",
            ControlKind::Mhi => "mhi",
            ControlKind::MhiGeneric => "mhi-generic",
        }
    }

    /// 要不要下发 WDA `Set Data Format`。
    ///
    /// GobiNet 是例外：quectel-CM 在 `qmidev_is_gobinet()` 分支里**明确跳过**
    /// （「when QMAP enabled, set data format in GobiNet driver」）。
    pub fn handles_data_format(self) -> bool {
        self != ControlKind::GobiNet
    }

    /// PCIe/MHI 的 QMAP 端点类型是 `DATA_EP_TYPE_PCIE`。
    pub fn is_pcie(self) -> bool {
        matches!(self, ControlKind::Mhi | ControlKind::MhiGeneric)
    }
}

/// 一个控制口（路径 + 驱动类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlDev {
    pub path: String,
    pub kind: ControlKind,
    /// wwan 类控制口的协议类型；其余布局为 `None`。
    pub wwan: Option<WwanProto>,
}

/// 从数据网卡反查控制口。
///
/// 顺序：`cdc-wdm` → GobiNet → 厂商 MHI → 主线 MHI（wwan 类）。**不猜**
/// `/dev/cdc-wdm0`：多模组时会把呼叫建到别人身上，所以每一路都要求 sysfs
/// 能对上，对不上就返回 `None`。wwan 类还要求端口协议与请求的链路一致
/// （QMI 链路配 `QMI` 口、MBIM 链路配 `MBIM` 口）。
pub fn find_control(iface: &str, link: DataLink) -> Option<ControlDev> {
    if let Some(path) = find_cdc_wdm(iface) {
        return Some(ControlDev {
            path,
            kind: ControlKind::CdcWdm,
            wwan: None,
        });
    }
    if let Some(path) = find_gobinet(iface) {
        return Some(ControlDev {
            path,
            kind: ControlKind::GobiNet,
            wwan: None,
        });
    }
    if let Some(path) = find_mhi(iface) {
        return Some(ControlDev {
            path,
            kind: ControlKind::Mhi,
            wwan: None,
        });
    }
    // 主线 MHI（mhi_net / mhi_wwan_*）：控制口在 /sys/class/wwan，
    // 类型由 `type` 文件给出，必须与要拨的链路一致才接。
    let want = match link {
        DataLink::Qmi => WwanProto::Qmi,
        DataLink::Mbim => WwanProto::Mbim,
    };
    find_wwan_mhi(iface, want).map(|path| ControlDev {
        path,
        kind: ControlKind::MhiGeneric,
        wwan: Some(want),
    })
}

/// `qmi_wwan` 的布局是 `<usb interface>/usbmisc/cdc-wdmN`；不同内核/驱动层级
/// 差一层，所以逐级向上找。
///
/// 子目录清单与 quectel-CM `detect_path_cdc_wdm_or_qcqmi()`
/// （`device.c:198-252`）、fibocom-dial 同名函数（`src/main.c:745-765`）对齐：
/// 两家都在 `<接口>/{GobiQMI,usbmisc,usb}` 三级里找控制口。GobiQMI 里放的是
/// `qcqmiX`（由 [`find_gobinet`] 扫 `/dev` 处理），这里只处理 `cdc-wdm*`，
/// 所以子目录取 `usbmisc` 与 `usb` 两个；早先漏了 `usb` —— 新版 qmi_wwan
/// 把 usbmisc 节点挂到接口的 `usb` 子目录下，只认 `usbmisc` 会找不到控制口，
/// 拨号器直接以 `NoWdm` 退避，看起来像「模组不支持」。
fn find_cdc_wdm(iface: &str) -> Option<String> {
    let base = parent_iface(iface);
    let mut dir = PathBuf::from(format!("/sys/class/net/{base}/device"));
    for _ in 0..4 {
        for sub in [
            dir.join("usbmisc"),
            dir.join("../usbmisc"),
            dir.join("usb"),
            dir.join("../usb"),
        ] {
            if let Ok(rd) = std::fs::read_dir(&sub) {
                let mut names: Vec<String> = rd
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.starts_with("cdc-wdm"))
                    .collect();
                names.sort();
                if let Some(n) = names.into_iter().next() {
                    return Some(format!("/dev/{n}"));
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn sysfs_device(iface: &str) -> Option<PathBuf> {
    std::fs::canonicalize(format!("/sys/class/net/{iface}/device")).ok()
}

/// USB 接口路径归一：`.../1-1.2/1-1.2:1.0` 与 `.../1-1.2` 视为同一设备。
///
/// **只认 USB 接口后缀**（`:<配置>.<接口>`，两段都是数字）。PCIe 设备名里
/// 也有冒号（`0000:01:00.0`），按「含冒号就切」会把它们错误地砍成父目录。
fn usb_dev_dir(p: &Path) -> PathBuf {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return p.to_path_buf();
    };
    let is_usb_iface = name
        .rsplit_once(':')
        .map(|(dev, iface)| {
            // USB 设备名形如 `1-1.2`（带 `-`）；PCIe 的 `0000:01` 不带
            dev.contains('-')
                && iface.split_once('.').is_some_and(|(cfg, num)| {
                    !cfg.is_empty()
                        && !num.is_empty()
                        && !num.contains('.')
                        && cfg.chars().all(|c| c.is_ascii_digit())
                        && num.chars().all(|c| c.is_ascii_digit())
                })
        })
        .unwrap_or(false);
    if is_usb_iface {
        p.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| p.to_path_buf())
    } else {
        p.to_path_buf()
    }
}

/// GobiNet：字符设备以 `/sys/class/GobiQMI/<name>/device` 反指 USB 接口。
fn find_gobinet(iface: &str) -> Option<String> {
    let dev = usb_dev_dir(&sysfs_device(iface)?);
    let mut hits = Vec::new();
    for entry in std::fs::read_dir("/dev").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("GobiQMI") {
            continue;
        }
        let link = PathBuf::from(format!("/sys/class/GobiQMI/{name}/device"));
        if let Ok(target) = std::fs::canonicalize(link) {
            if usb_dev_dir(&target) == dev {
                hits.push(format!("/dev/{name}"));
            }
        }
    }
    hits.sort();
    if hits.len() == 1 {
        return hits.pop();
    }
    None
}

/// MHI：`/dev/mhi_QMIx` 与 PCIe 设备对应；`/sys/class/mhi/mhiN/device` 是同一
/// 个 PCI 设备（数据网卡也叫 `mhi_wwan*`）。层级因驱动版本有差异，所以双向比。
fn find_mhi(iface: &str) -> Option<String> {
    let dev = sysfs_device(iface)?;
    let mut all = Vec::new();
    let mut hits = Vec::new();
    for entry in std::fs::read_dir("/dev").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(idx) = name.strip_prefix("mhi_QMI") else {
            continue;
        };
        let path = format!("/dev/{name}");
        all.push(path.clone());
        let link = PathBuf::from(format!("/sys/class/mhi/mhi{idx}/device"));
        if let Ok(target) = std::fs::canonicalize(link) {
            if target == dev || target.starts_with(&dev) || dev.starts_with(&target) {
                hits.push(path);
            }
        }
    }
    hits.sort();
    if hits.len() == 1 {
        return hits.pop();
    }
    all.sort();
    // 只有一个 MHI 控制口、网卡名又是 mhi_* ⇒ 就是它（否则等于「有口不用」）
    if all.len() == 1 && iface.starts_with("mhi") {
        return all.pop();
    }
    None
}

/// 从设备 realpath 里取出 MHI 控制器路径（`…/mhi0/…` → `…/mhi0`）。
///
/// 只认**整段** `mhi<数字>` 路径组件；`mhi_wwan` 这类厂商驱动名不是控制器，
/// 不能当锚点（对齐 quectel-CM `cm_mhi_detect` 的判定：组件内必须是纯数字）。
pub fn mhi_controller_from_realpath(real: &Path) -> Option<PathBuf> {
    let s = real.to_str()?;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        // 找 `/mhi<数字>(/|$)`：组件名恰为 mhi+纯数字
        if bytes[i] == b'/'
            && s[i + 1..].starts_with("mhi")
            && s[i + 4..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .count()
                > 0
        {
            let digits = s[i + 4..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .count();
            let rest = &s[i + 4 + digits..];
            if rest.is_empty() || rest.starts_with('/') {
                return Some(PathBuf::from(&s[..i + 4 + digits]));
            }
        }
        i += 1;
    }
    None
}

/// 从 wwan 类端口里挑控制口（纯函数，便于用假目录表测试）。
///
/// 候选必须同时满足：`type` 文件内容精确等于期望协议、端口 realpath 位于该
/// MHI 控制器子树内、`/dev/<端口名>` 存在。**候选数 ≠ 1 直接放弃** ——
/// 上游 `cm_mhi_detect` 的 fail-closed 口径：独立端点保持歧义时绝不按
/// 目录顺序瞎选，选错控制口会把呼叫建到别的通道上。
pub fn pick_wwan_control(
    controller: &Path,
    want: WwanProto,
    ports: &[(String, String, PathBuf)],
    dev_exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut hits: Vec<String> = ports
        .iter()
        .filter(|(name, ty, real)| {
            let prefix = format!("{}/", controller.display());
            ty.trim() == want.sysfs_name()
                && (real == controller || real.starts_with(&prefix))
                && dev_exists(name)
        })
        .map(|(name, _, _)| name.clone())
        .collect();
    hits.sort();
    if hits.len() == 1 {
        return Some(format!("/dev/{}", hits.remove(0)));
    }
    None
}

/// 主线 MHI 的控制口发现：数据网卡挂在 `mhi<N>` 控制器下，且该控制器子树里
/// 恰有一个期望协议的 wwan 端口（`/dev/wwan*`）。
fn find_wwan_mhi(iface: &str, want: WwanProto) -> Option<String> {
    let dev = sysfs_device(iface)?;
    // 软件通道（`_IP_SW`）不是数据端点，出现在路径里直接放弃（上游同口径）
    if dev.to_str()?.contains("_IP_SW") {
        return None;
    }
    let controller = mhi_controller_from_realpath(&dev)?;
    let mut ports = Vec::new();
    for entry in std::fs::read_dir("/sys/class/wwan").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let port = PathBuf::from(format!("/sys/class/wwan/{name}"));
        let ty = std::fs::read_to_string(port.join("type")).unwrap_or_default();
        let real = std::fs::canonicalize(&port).unwrap_or(port);
        ports.push((name, ty, real));
    }
    pick_wwan_control(&controller, want, &ports, |n| {
        Path::new(&format!("/dev/{n}")).exists()
    })
}

/// QMAP 的父口名（`wwan0_1` → `wwan0`）。
pub fn parent_iface(iface: &str) -> String {
    // 只认 QMAP 的 `<父口>_<mux>`（后缀必须是数字）；`rmnet_data0` 这类名字
    // 里的下划线不是 mux 分隔符，不能切。
    match iface.rsplit_once('_') {
        Some((p, n)) if !p.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            p.to_string()
        }
        _ => iface.to_string(),
    }
}

/// 把 qmi_wwan 的 `raw_ip` 置 `Y`（数据面走 IP 而不是 802.3）。
///
/// 返回 `true` = 现在是 raw IP（或本来没有这个 sysfs，不需要设）。
/// 内核里这个属性有 `raw_ip` 与老版本的 `rawip` 两种写法，都试。
///
/// ## 为什么必须先 down 再写（对齐上游 `prepareQMIWwanRawIP`）
///
/// 主线 qmi_wwan 的 `raw_ip_store()` 在 `netif_running()` 时直接返回
/// **-EBUSY**（"Cannot change a running device"）。而 netifd 的 `modem_pro`
/// 接口把数据网卡常年拉在 up（udhcpc 就跑在它上面）—— 不 down 就写，
/// **每一次都失败**：会话起不来 → 5→60s 退避 → 页面上原生拨号器永远「未运行」。
/// 上游 `hostdial_posix.go` 正是为了这个才实现 down → 写 → up，
/// 并对「模组刚重新枚举完、属性短暂 EBUSY」的窗口做 5 次 × 800ms 重试。
///
/// 网卡被留在 down 状态比切换失败更糟，所以每次尝试后都把网卡拉回来
/// （哪怕这次没写成功）。写成功后 netifd 的 udhcpc 会按新模式重新取址。
pub fn ensure_raw_ip(iface: &str) -> bool {
    let base = parent_iface(iface);
    for name in ["raw_ip", "rawip"] {
        let path = format!("/sys/class/net/{base}/qmi/{name}");
        let Ok(cur) = std::fs::read_to_string(&path) else {
            continue;
        };
        let cur = cur.trim();
        if cur == "Y" || cur == "1" {
            return true;
        }
        // raw_ip 只能在网卡 down 时改；模组刚重新枚举的头几秒属性还会短暂 EBUSY，
        // 所以重试（与上游 rawIPAttempts=5 / rawIPRetryGap=800ms 一致）。
        let mut last_err = None;
        for attempt in 0..5 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(800));
                // 上一轮可能已经把值写下去了：先重读，避免无谓地再 down 一次
                if let Ok(v) = std::fs::read_to_string(&path) {
                    let v = v.trim();
                    if v == "Y" || v == "1" {
                        log::info!("原生拨号：{base} 的 qmi/{name} 已是 Y（数据面走 raw IP）");
                        return true;
                    }
                }
            }
            let _ = std::process::Command::new("ip")
                .args(["link", "set", "dev", &base, "down"])
                .output();
            let write = std::fs::write(&path, "Y\n");
            // 每次尝试后都把网卡恢复起来（up 失败不影响成败判定）
            let _ = std::process::Command::new("ip")
                .args(["link", "set", "dev", &base, "up"])
                .output();
            match write {
                Ok(_) => {
                    log::info!(
                        "原生拨号：把 {base} 的 qmi/{name} 置为 Y（第 {} 次尝试，数据面走 raw IP）",
                        attempt + 1
                    );
                    return true;
                }
                Err(e) => {
                    log::warn!("写 {path} 失败（第 {}/5 次尝试）: {e}", attempt + 1);
                    last_err = Some(e);
                }
            }
        }
        if let Some(e) = last_err {
            log::warn!("原生拨号：{path} 重试 5 次仍未写成（{e}），本轮不拉起拨号器");
        }
        return false;
    }
    // 没有这个属性：要么不是 qmi_wwan（比如 GobiNet），要么已经由驱动处理好
    true
}

/// 原生拨号计划（纯数据，便于比较与日志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePlan {
    pub modem_id: u32,
    /// 厂商（决定用哪套策略）。
    pub vendor: Vendor,
    /// 数据面（QMI / MBIM）。
    pub link: DataLink,
    /// 控制口（`/dev/cdc-wdmN` / `/dev/GobiQMIx` / `/dev/mhi_QMIx`）。
    pub control: ControlDev,
    /// 数据网卡（raw_ip sysfs 在它上面；QMAP 时取父口）。
    pub iface: String,
    pub apn: String,
    pub ipv6: bool,
    /// profile index（`[net] host_dial_pdn`，0 = 不指定）。
    pub pdn: i64,
    /// QMAP 参数（非 QMAP 模组为 `None`）。
    pub qmap: Option<qmi::QmapSettings>,
    /// 主线 MHI（generic MHI）：WDA 走「裸 IP、无聚合」，端点绑定不带 mux id。
    pub mhi_generic: bool,
    /// MBIM session id（QMI 忽略）。
    pub session: u32,
}

impl NativePlan {
    /// 一行可读描述（日志 / 状态接口用）。
    pub fn describe(&self) -> String {
        format!(
            "native {} {} [{} {}] (apn={}, v6={}, pdn={}, qmap={})",
            self.vendor.as_str(),
            self.link.as_str(),
            self.control.kind.as_str(),
            self.control.path,
            if self.apn.is_empty() {
                "默认"
            } else {
                &self.apn
            },
            self.ipv6,
            self.pdn,
            self.qmap
                .map(|q| format!("0x{:02x}", q.mux_id))
                .unwrap_or_else(|| "-".to_string())
        )
    }

    fn caps(&self) -> &'static Caps {
        vendors::select(self.vendor)
    }
}

/// 把链路类型字符串归一（`qmi` / `mbim`）。
fn parse_link(kind: &str) -> Option<DataLink> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "qmi" => Some(DataLink::Qmi),
        "mbim" => Some(DataLink::Mbim),
        _ => None,
    }
}

/// 从数据网卡名反推 PDN（`wwan0_1` → 1）；`rmnet_data0` 这类不算。
fn pdn_from_iface(iface: &str) -> Option<i64> {
    let tail = iface.rsplit('_').next()?;
    let n: i64 = tail.parse().ok()?;
    (n > 0).then_some(n)
}

/// 解析 PDN / mux 序号。
///
/// `> 0` 强制；`-1` 只按网卡名反推；`0`（默认）先反推，QMAP 单承载驱动按 1。
fn resolve_pdn(device: &str, cfg: i64, qmap_mode: i64) -> i64 {
    match cfg {
        n if n > 0 => n,
        -1 => pdn_from_iface(device).unwrap_or(0),
        _ => pdn_from_iface(device)
            .or((qmap_mode == 1).then_some(1))
            .unwrap_or(0),
    }
}

/// 聚合窗口：驱动每网卡的 `qmap_size` 属性（父口上没有就看数据口），
/// 读不到交给缺省。
///
/// 依据：`qmi_wwan_q` / `qmi_wwan_f` 都把窗口做成**每网卡** sysfs 属性
/// （`DEVICE_ATTR(qmap_size)`），不是模块参数；取值来自驱动 USB ID 表
/// （如 16 KiB / 31 KiB，USB3 缺省 16 KiB）。下面的模块参数路径是防御性
/// 保留（当前核对的这两个驱动都没有该参数），命中不了也不影响——
/// 最终值还会被 WDA Set Data Format 应答回读覆盖。
fn qmap_size(parent: &str, device: &str) -> u32 {
    for dev in [parent, device] {
        if let Ok(s) = std::fs::read_to_string(format!("/sys/class/net/{dev}/qmap_size")) {
            if let Ok(v) = s.trim().parse::<u32>() {
                return v;
            }
        }
    }
    // 防御：个别驱动分支可能把窗口挂成模块参数
    for p in [
        format!("/sys/class/net/{parent}/device/driver/module/parameters/qmap_size"),
        "/sys/module/qmi_wwan_q/parameters/qmap_size".to_string(),
    ] {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(v) = s.trim().parse::<u32>() {
                return v;
            }
        }
    }
    0
}

/// 算 QMAP 参数（纯函数）。
///
/// - `qmap_mode <= 0`：不是 QMAP，返回 `None`；
/// - `qmap_mode == 1`：单承载，唯一 mux 是 `0x81`（与 quectel-CM 一致）；
/// - `qmap_mode > 1`：mux = PDN + `0x80`（`wwan0_1` → `0x81`）；
/// - GobiNet：驱动自己设数据格式，不需要 WDA（返回 `None`）。
pub fn qmap_settings(
    kind: ControlKind,
    qmap_mode: i64,
    pdn: i64,
    rx_urb_size: u32,
) -> Option<qmi::QmapSettings> {
    if qmap_mode <= 0 || !kind.handles_data_format() {
        return None;
    }
    let mux = if qmap_mode == 1 {
        0x81
    } else {
        // mux id 是 1 字节：PDN 上限 0x7F，别让配置里的离谱值绕回 0x80
        (pdn.clamp(1, 0x7F) + 0x80) as u8
    };
    Some(if kind.is_pcie() {
        qmi::QmapSettings::pcie(mux, rx_urb_size)
    } else {
        qmi::QmapSettings::usb(mux, rx_urb_size)
    })
}

/// 算一个原生拨号计划（唯一 IO：反查控制口 + 读 qmap sysfs）。
#[allow(clippy::too_many_arguments)] // 计划本来就是「模组 + 类型 + APN + 网卡 + 地址族 + PDN」
pub fn plan_native(
    modem_id: u32,
    vendor: Vendor,
    kind: &str,
    apn: &str,
    device: &str,
    ipv6: bool,
    pdn: i64,
) -> Result<NativePlan, PlanError> {
    let Some(link) = parse_link(kind) else {
        return Err(PlanError::Unsupported(format!(
            "原生拨号器只接管 QMI / MBIM（当前类型 {kind}）；ECM/RNDIS/NCM 走模组侧 AT 拨号"
        )));
    };
    let caps = vendors::select(vendor);
    if !vendors::supports(vendor, link) {
        return Err(PlanError::Unsupported(format!(
            "{} 的原生后端不支持 {}（只支持 {}）：{}",
            vendor.label(),
            link.as_str(),
            caps.links
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
            caps.note
        )));
    }
    // MBIM 必须有 APN（协议要求，空 APN 的 CONNECT 会被设备拒）
    if link == DataLink::Mbim && apn.trim().is_empty() {
        return Err(PlanError::MbimNeedsApn);
    }
    if device.trim().is_empty() {
        return Err(PlanError::NoDevice);
    }
    let parent = parent_iface(device);
    let control = find_control(device, link).ok_or_else(|| {
        PlanError::NoWdm(format!(
            "/sys/class/net/{parent}/device 下没有 cdc-wdm / GobiQMI / mhi_QMI / wwan 控制口"
        ))
    })?;
    // MBIM 的控制口协议由内核驱动实现（cdc_mbim / mhi_wwan_mbim），其余布局
    // （GobiNet / 厂商 pcie_mhi）只有 QMUX，没有 MBIM 帧。
    if link == DataLink::Mbim
        && !matches!(control.kind, ControlKind::CdcWdm | ControlKind::MhiGeneric)
    {
        return Err(PlanError::Unsupported(format!(
            "MBIM 需要 cdc-wdm 或 wwan(MBIM) 控制口，{} 上只有 {}",
            parent,
            control.kind.as_str()
        )));
    }
    let qmap_mode = crate::netprov::provision::qmap_mode(&parent);
    let pdn = resolve_pdn(device, pdn, qmap_mode);
    // generic MHI 固定走「裸 IP、无聚合 + BindMhi」：不读 qmap_mode（mhi_net
    // 没有这个 sysfs，就算有也不是 QMAP 语义），也不算聚合窗口。
    let mhi_generic = control.kind == ControlKind::MhiGeneric;
    let qmap = if mhi_generic {
        None
    } else {
        qmap_settings(control.kind, qmap_mode, pdn, qmap_size(&parent, device))
    };
    Ok(NativePlan {
        modem_id,
        vendor,
        link,
        control,
        iface: parent,
        apn: apn.trim().to_string(),
        ipv6,
        pdn,
        qmap,
        mhi_generic,
        session: caps.mbim_session,
    })
}

/// 一个在跑的原生会话（QMI 或 MBIM）。
pub struct Session {
    pub plan: NativePlan,
    inner: Inner,
    started_at: String,
    /// 会话建立时刻（协调器的掉线自愈用它算「跑了多久还没地址」）。
    pub started: std::time::Instant,
    /// 给页面看的一句话（v6 未起、订阅者未就绪等非致命情况）
    pub note: String,
}

enum Inner {
    Qmi(qmi_link::QmiSession),
    #[cfg(unix)]
    Mbim(mbim_link::MbimSession),
}

impl Session {
    /// 建立数据呼叫（v4 必须成功；v6 失败只记 note，不拖垮 v4）。
    pub fn start(plan: NativePlan) -> Result<Self, String> {
        let caps = plan.caps();
        let (inner, note) = match plan.link {
            DataLink::Qmi => {
                // raw_ip 只对 QMI 有意义；MBIM 走 802.3 封装，设了反而可能出错
                if caps.needs_raw_ip && !ensure_raw_ip(&plan.iface) {
                    return Err(format!("{} 的 qmi/raw_ip 设置失败", plan.iface));
                }
                let (s, note) = qmi_link::QmiSession::start(
                    &plan.control.path,
                    &plan.iface,
                    &plan.apn,
                    plan.ipv6,
                    plan.pdn,
                    caps.disable_autoconnect,
                    plan.qmap,
                    plan.mhi_generic,
                )
                .map_err(|e| format!("{}：{e}", plan.describe()))?;
                (Inner::Qmi(s), note)
            }
            DataLink::Mbim => {
                #[cfg(unix)]
                {
                    let (s, note) = mbim_link::MbimSession::start(
                        &plan.control.path,
                        &plan.apn,
                        plan.ipv6,
                        plan.session,
                    )
                    .map_err(|e| format!("{}：{e}", plan.describe()))?;
                    (Inner::Mbim(s), note)
                }
                #[cfg(not(unix))]
                {
                    return Err("MBIM 原生拨号只支持 Linux".to_string());
                }
            }
        };
        if !note.is_empty() {
            log::warn!("原生拨号：模组 {} {note}", plan.modem_id);
        }
        let s = Self {
            plan,
            inner,
            started_at: crate::logging::rfc3339(crate::logging::unix_now()),
            started: std::time::Instant::now(),
            note,
        };
        log::info!(
            "原生拨号：模组 {} 数据呼叫已建立（{}）",
            s.plan.modem_id,
            s.plan.describe()
        );
        Ok(s)
    }

    /// 数据呼叫还活着吗？传输层出错时返回 `None`（别因为一次读超时就重拨）。
    pub fn alive(&mut self) -> Option<bool> {
        let r = match &mut self.inner {
            Inner::Qmi(s) => s.alive(),
            #[cfg(unix)]
            Inner::Mbim(s) => s.alive(),
        };
        if r == Some(false) {
            log::warn!(
                "原生拨号：模组 {} 数据呼叫已断开（{}）",
                self.plan.modem_id,
                self.plan.describe()
            );
        }
        r
    }

    /// 主动拆掉呼叫（重复调用安全）。
    pub fn stop(&mut self) {
        match &mut self.inner {
            Inner::Qmi(s) => s.stop(),
            #[cfg(unix)]
            Inner::Mbim(s) => s.stop(),
        }
        log::info!(
            "原生拨号：模组 {} 数据呼叫已拆除（{}）",
            self.plan.modem_id,
            self.plan.describe()
        );
    }

    /// 给 `/api/dial` 的状态（不外泄 APN 之外的任何凭据）。
    pub fn status(&self) -> serde_json::Value {
        let detail = match &self.inner {
            Inner::Qmi(s) => s.status(),
            #[cfg(unix)]
            Inner::Mbim(s) => s.status(),
        };
        let mut v = serde_json::json!({
            "backend": format!("native-{}", self.plan.link.as_str()),
            "vendor": self.plan.vendor.as_str(),
            "plan": self.plan.describe(),
            "control": self.plan.control.path,
            "control_kind": self.plan.control.kind.as_str(),
            "device": self.plan.iface,
            "started_at": self.started_at,
            "note": self.note,
        });
        if let (Some(dst), Some(src)) = (v.as_object_mut(), detail.as_object()) {
            for (k, val) in src {
                dst.insert(k.clone(), val.clone());
            }
        }
        v
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_plan_rejects_unsupported_link_kind() {
        let e = plan_native(0, Vendor::Quectel, "ecm", "ctnet", "wwan0", false, 0).unwrap_err();
        assert!(matches!(e, PlanError::Unsupported(_)), "ECM 不该被原生接管");
        let e = plan_native(0, Vendor::Quectel, "qmi", "ctnet", "", false, 0).unwrap_err();
        assert!(matches!(e, PlanError::NoDevice));
    }

    #[test]
    fn vendor_mbim_is_not_gated_by_external_dialer_sources() {
        // 外部时代「MBIM 一律 quectel-CM」；原生后端自己实现 MBIM，
        // Fibocom 的 MBIM 模组不能再因为「fibocom-dial 没 mbim-cm.c」被拒
        for v in [
            Vendor::Fibocom,
            Vendor::Simcom,
            Vendor::TDTech,
            Vendor::Unknown,
        ] {
            assert!(
                vendors::supports(v, DataLink::Mbim),
                "{v:?} 应该能走原生 MBIM"
            );
        }
        // 仍然要拒绝 ECM 这种不归主机侧管的类型
        let e = plan_native(0, Vendor::Fibocom, "ecm", "ctnet", "wwan0", false, 0).unwrap_err();
        assert!(matches!(e, PlanError::Unsupported(_)));
    }

    #[test]
    fn mbim_without_apn_is_rejected_before_io() {
        // MBIM 的 CONNECT 必须带 APN；空 APN 先拦，别等设备回失败
        let e = plan_native(0, Vendor::Quectel, "mbim", "", "wwan0", false, 0).unwrap_err();
        assert!(matches!(e, PlanError::MbimNeedsApn));
    }

    #[test]
    fn describe_carries_control_and_qmap() {
        let p = NativePlan {
            modem_id: 1,
            vendor: Vendor::Quectel,
            link: DataLink::Qmi,
            control: ControlDev {
                path: "/dev/cdc-wdm0".into(),
                kind: ControlKind::CdcWdm,
                wwan: None,
            },
            iface: "wwan0".into(),
            apn: "ctnet".into(),
            ipv6: true,
            pdn: 1,
            qmap: Some(qmi::QmapSettings::usb(0x81, 16 * 1024)),
            mhi_generic: false,
            session: 0,
        };
        let s = p.describe();
        assert!(s.contains("ctnet"), "{s}");
        assert!(s.contains("v6=true"), "{s}");
        assert!(s.contains("quectel"), "{s}");
        assert!(s.contains("qmi"), "{s}");
        assert!(s.contains("cdc-wdm"), "{s}");
        assert!(s.contains("qmap=0x81"), "{s}");
    }

    #[test]
    fn parent_iface_strips_mux_suffix() {
        assert_eq!(parent_iface("wwan0_1"), "wwan0");
        assert_eq!(parent_iface("wwan0"), "wwan0");
        assert_eq!(parent_iface("rmnet_data0"), "rmnet_data0");
    }

    #[test]
    fn find_control_returns_none_for_missing_iface() {
        // 不存在的网卡不能瞎猜 /dev/cdc-wdm0（多模组会建到别人身上）
        assert_eq!(find_control("definitely-not-an-iface", DataLink::Qmi), None);
    }

    #[test]
    fn raw_ip_is_a_noop_without_the_sysfs_attr() {
        // 没有 qmi/raw_ip 属性（非 qmi_wwan 驱动 / 测试机）＝ 不需要也不允许设：
        // 返回 true，绝不能因为找不到属性就判失败（GobiNet / MHI 走这条路）。
        assert!(ensure_raw_ip("definitely-not-an-iface"));
    }

    // ---------- QMAP 计划 ----------

    #[test]
    fn qmap_is_planned_only_for_qmap_drivers() {
        // 非 QMAP：不下发 WDA
        assert_eq!(qmap_settings(ControlKind::CdcWdm, 0, 1, 16384), None);
        // GobiNet：驱动自己设数据格式
        assert_eq!(qmap_settings(ControlKind::GobiNet, 4, 1, 16384), None);
        // 单承载：唯一 mux 0x81
        let s = qmap_settings(ControlKind::CdcWdm, 1, 0, 16384).expect("qmap");
        assert_eq!(s.mux_id, 0x81);
        assert_eq!(s.ep_type, qmi::EP_TYPE_HSUSB);
        // 多承载：PDN + 0x80
        let s = qmap_settings(ControlKind::CdcWdm, 4, 3, 16384).expect("qmap");
        assert_eq!(s.mux_id, 0x83);
        // PCIe/MHI：端点类型不同
        let s = qmap_settings(ControlKind::Mhi, 1, 0, 16384).expect("qmap");
        assert_eq!(s.ep_type, qmi::EP_TYPE_PCIE);
        // PDN 是 1 字节上限，离谱配置不能绕回 0x80
        let s = qmap_settings(ControlKind::CdcWdm, 4, 500, 16384).expect("qmap");
        assert_eq!(s.mux_id, 0xFF);
    }

    #[test]
    fn pdn_resolution_prefers_explicit_then_iface_then_qmap() {
        assert_eq!(resolve_pdn("wwan0_2", 3, 4), 3, "显式配置优先");
        assert_eq!(resolve_pdn("wwan0_2", 0, 4), 2, "其次按网卡名反推");
        assert_eq!(resolve_pdn("wwan0", 0, 1), 1, "单承载 QMAP 兜底 1");
        assert_eq!(resolve_pdn("wwan0", -1, 1), 0, "-1 表示只按网卡名");
        assert_eq!(resolve_pdn("wwan0", 0, 0), 0, "非 QMAP 不猜");
    }

    #[test]
    fn usb_interface_path_is_normalised_to_device() {
        use std::path::PathBuf;
        // qmi_wwan 的 netdev device 是 USB 接口目录（带 :1.0），GobiQMI 也回指它；
        // 归一后两者才能比较，否则永远匹配不上
        let a = usb_dev_dir(&PathBuf::from("/sys/devices/pci0/usb1/1-1.2/1-1.2:1.0"));
        let b = usb_dev_dir(&PathBuf::from("/sys/devices/pci0/usb1/1-1.2"));
        assert_eq!(a, b);
        // 不带冒号的路径原样返回（PCIe 设备名里也有冒号，不能误切）
        let p = PathBuf::from("/sys/devices/pci0000:00/0000:01:00.0");
        assert_eq!(usb_dev_dir(&p), p);
    }

    #[test]
    fn control_kind_matches_driver_expectations() {
        assert!(ControlKind::CdcWdm.handles_data_format());
        assert!(!ControlKind::GobiNet.handles_data_format());
        assert!(ControlKind::Mhi.is_pcie());
        assert!(
            ControlKind::MhiGeneric.is_pcie(),
            "generic MHI 也是 PCIe 端点"
        );
        assert_eq!(ControlKind::GobiNet.as_str(), "gobinet");
        assert_eq!(ControlKind::MhiGeneric.as_str(), "mhi-generic");
    }

    // ---------- generic MHI（主线 mhi_net / mhi_wwan_*） ----------

    #[test]
    fn mhi_controller_is_extracted_from_realpath_components() {
        use std::path::PathBuf;
        // 数据网卡的 device 指回 MHI 控制器子树
        let p = PathBuf::from("/sys/devices/pci0000:00/0000:01:00.0/mhi0/net/mhi_netdev0");
        assert_eq!(
            mhi_controller_from_realpath(&p),
            Some(PathBuf::from("/sys/devices/pci0000:00/0000:01:00.0/mhi0"))
        );
        // 厂商驱动名（mhi_wwan）不是控制器，不能当锚点
        assert_eq!(
            mhi_controller_from_realpath(&PathBuf::from(
                "/sys/devices/pci0000:00/0000:01:00.0/mhi_wwan/net/wwan0"
            )),
            None
        );
        // 没有 MHI 组件的普通 USB 路径
        assert_eq!(
            mhi_controller_from_realpath(&PathBuf::from(
                "/sys/devices/pci0/usb1/1-1.2/1-1.2:1.0/net/wwan0"
            )),
            None
        );
        // mhi0 与 mhi01 必须按整段组件区分，不能前缀误配
        let deep = PathBuf::from("/sys/devices/x/mhi01/net/wwan0");
        assert_eq!(
            mhi_controller_from_realpath(&deep),
            Some(PathBuf::from("/sys/devices/x/mhi01"))
        );
    }

    #[test]
    fn wwan_control_requires_exact_type_and_unique_candidate() {
        let controller = Path::new("/sys/devices/x/mhi0");
        let ports = |qmi: bool, mbim: bool| {
            let mut v: Vec<(String, String, PathBuf)> = Vec::new();
            if qmi {
                v.push((
                    "wwan0qmi0".into(),
                    "QMI\n".into(),
                    PathBuf::from("/sys/devices/x/mhi0/wwan0qmi0"),
                ));
            }
            if mbim {
                v.push((
                    "wwan0mbim0".into(),
                    "MBIM\n".into(),
                    PathBuf::from("/sys/devices/x/mhi0/wwan0mbim0"),
                ));
            }
            v
        };
        let exists = |n: &str| n.starts_with("wwan");
        // 唯一 QMI 口 ⇒ 命中
        assert_eq!(
            pick_wwan_control(controller, WwanProto::Qmi, &ports(true, false), exists),
            Some("/dev/wwan0qmi0".to_string())
        );
        // 唯一 MBIM 口 ⇒ MBIM 命中；同一台设备查 QMI ⇒ 无候选
        assert_eq!(
            pick_wwan_control(controller, WwanProto::Mbim, &ports(false, true), exists),
            Some("/dev/wwan0mbim0".to_string())
        );
        assert_eq!(
            pick_wwan_control(controller, WwanProto::Qmi, &ports(false, true), exists),
            None
        );
        // 类型拼写不精确（带尾随空格的 trim 可以，别的名字不行）
        let mut wrong = ports(true, false);
        wrong[0].1 = "QMI_WWAN\n".into();
        assert_eq!(
            pick_wwan_control(controller, WwanProto::Qmi, &wrong, exists),
            None
        );
        // 端口在别的控制器子树 ⇒ 不算
        let mut other = ports(true, false);
        other[0].2 = PathBuf::from("/sys/devices/x/mhi1/wwan0qmi0");
        assert_eq!(
            pick_wwan_control(controller, WwanProto::Qmi, &other, exists),
            None
        );
        // /dev 节点不存在 ⇒ 不算
        assert_eq!(
            pick_wwan_control(controller, WwanProto::Qmi, &ports(true, false), |_| false),
            None
        );
        // 两个同名候选 ⇒ fail-closed（绝不按目录顺序瞎选）
        let mut dup = ports(true, false);
        dup.push((
            "wwan1qmi0".into(),
            "QMI\n".into(),
            PathBuf::from("/sys/devices/x/mhi0/wwan1qmi0"),
        ));
        assert_eq!(
            pick_wwan_control(controller, WwanProto::Qmi, &dup, exists),
            None
        );
    }
}
