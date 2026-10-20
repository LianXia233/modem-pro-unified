//! 拨号上网域（`/api/dial` 一族）：USB 类型识别、拨号状态机、拨号方式表。
//!
//! 契约见 docs/API.md 的拨号一族：
//!   · 一个 AT 口 = 一个模组（简化模型），`DialState` 挂在 [`Modem`] 上；
//!   · USB 类型（usbnet）决定拨号方式：**QMI / MBIM 的数据呼叫归主机侧厂商拨号器**
//!     （现在由 modemd 托管，见 `runtime/dialer/`），其余走模组侧 AT 激活 PDP；
//!   · 拨号过程逐步记录（`DialStep`），失败也把状态带回去 —— 页面要显示「卡在哪一步」。

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{msg, Result};
use crate::logging::{rfc3339, seconds_since, unix_now};

use super::super::at::{csv, extract_field, is_ok, Prio};
use super::addr;
use super::modem::Modem;
use super::params::{cgact_state, is_internal_apn, valid_apn, Context};
use super::regstate;

// ---------- 拨号状态机 ----------

/// 未拨号（零值归一后的初始态）。
pub const DIAL_IDLE: &str = "idle";
/// 有数据网卡但没拿到地址（或地址已失效）。
pub const DIAL_DISCONNECTED: &str = "disconnected";
/// 正在断开（含主机侧拨号器收尾）。
pub const DIAL_DISCONNECTING: &str = "disconnecting";
/// 连数据网卡都没识别到 —— 状态未知，不是「未连接」。
///
/// 两者必须分开：`disconnected` 表示「模组在、链路没起来」（可以重拨），
/// `unknown` 表示「连该看哪块网卡都不知道」（多半是模组没插好 / 驱动没绑）。
pub const DIAL_UNKNOWN: &str = "unknown";
/// 拨号中。
pub const DIAL_DIALING: &str = "dialing";
/// 已连接。
pub const DIAL_CONNECTED: &str = "connected";
/// 拨号失败。
pub const DIAL_FAILED: &str = "failed";

// ---------------- 链路阶段（`DialState::phase`）----------------
//
// 「拨号成功」有三种完全不同的含义，以前只有一个 `state`：
//   ① 模组说 PDP 激活了（AT+CGACT=1 回 OK）；
//   ② 模组给了本机地址（AT+CGPADDR 有值）；
//   ③ 主机真的能经这块网卡发出去（网卡有地址 + 默认路由经它）。
// ①② 都满足而 ③ 不满足时（实测 FM350：主机网卡挂 169.254、无默认路由），
// 页面照样显示「已连接」，用户以为网络坏了。`phase` 把这三层拆开，
// `state` 保持不变（API 契约不动），前端按 phase 显示细分说明。
pub const PHASE_IDLE: &str = "idle";
pub const PHASE_DIALING: &str = "dialing";
/// 模组侧数据会话已建立（PDP 激活 / 数据呼叫建立），主机侧未验证。
pub const PHASE_SESSION: &str = "session";
/// 主机网卡还没起来（没找到网卡 / 网卡没拿到地址）。
pub const PHASE_IFACE: &str = "iface_pending";
/// 网卡有地址，但默认路由不经它（被别的口抢了 / 没配）。
pub const PHASE_ROUTE: &str = "route_pending";
/// 网卡有地址且默认路由经它 —— 这才是用户体感里的「已连接」。
pub const PHASE_ONLINE: &str = "online";
pub const PHASE_FAILED: &str = "failed";

/// 连接态下的阶段判定（纯函数，便于单测）。
///
/// 三个布尔都要「事实」而不是「推断」：网卡在不在、有没有地址、默认路由走不走它。
/// 任何一层不满足都如实标出来 —— 排障方向完全不同：
/// `iface_pending` 去查 USB / 驱动 / hostnic，`route_pending` 去查 metric / netifd。
fn phase_for_connected(has_iface: bool, has_addr: bool, has_route: bool) -> &'static str {
    match (has_iface, has_addr, has_route) {
        (true, true, true) => PHASE_ONLINE,
        (true, true, false) => PHASE_ROUTE,
        _ => PHASE_IFACE,
    }
}

/// 阶段 → 人话（界面「链路阶段」一列）。
pub fn dial_phase_text(phase: &str) -> String {
    match phase {
        PHASE_IDLE => "未初始化".into(),
        PHASE_DIALING => "拨号执行中".into(),
        PHASE_SESSION => "数据会话已建立（主机侧未验证）".into(),
        PHASE_IFACE => "数据会话已建立，主机网卡未就绪".into(),
        PHASE_ROUTE => "网卡有地址，但默认路由不经它".into(),
        PHASE_ONLINE => "网络就绪（地址 + 默认路由）".into(),
        PHASE_FAILED => "失败".into(),
        other => other.to_string(),
    }
}

/// 数据默认 cid（请求没带 cid 时用它）。
pub const DEFAULT_DATA_CID: i32 = 1;

/// 状态码 → 中文（页面「拨号上网」显示）。
pub fn dial_state_text(state: &str) -> String {
    match state {
        DIAL_DIALING => "拨号中",
        DIAL_CONNECTED => "已连接",
        DIAL_FAILED => "连接异常",
        DIAL_DISCONNECTED => "未连接",
        DIAL_DISCONNECTING => "正在断开",
        DIAL_UNKNOWN => "状态未知",
        _ => "未拨号",
    }
    .to_string()
}

/// cid 合法性（0~42，0 由调用方归一成默认 cid）。
pub fn valid_cid(cid: i32) -> bool {
    (0..=42).contains(&cid)
}

/// PDP 类型合法性（只认实测枚举出来的三种，PPP 不用于数据）。
pub fn valid_pdp_type(t: &str) -> bool {
    matches!(t.trim().to_uppercase().as_str(), "IP" | "IPV6" | "IPV4V6")
}

/// 拨号过程的一步（页面按顺序列出来，失败的那一步一眼能看到）。
#[derive(Debug, Clone, Serialize)]
pub struct DialStep {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub info: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub at: String,
}

/// 当前拨号状态（页面「拨号上网」显示它；WS 推送类型 `dial`）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct DialState {
    pub modem_id: u32,
    pub state: String,
    pub state_text: String,
    /// 拨的是哪条 PDP 上下文（默认 [`DEFAULT_DATA_CID`](self::DEFAULT_DATA_CID)）。
    pub cid: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub apn: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pdp_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ipv4: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ipv6: String,

    /// 拨号时识别出的 USB 类型（见 [`Modem::usb_type`]），它决定主机侧怎么拨。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub type_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub host_setup: String,

    /// 主机侧网卡候选，**仅供参考**：同一台机器上别的网卡也可能命中命名规则。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub host_iface: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub host_ips: Vec<String>,

    #[serde(skip_serializing_if = "String::is_empty")]
    pub connected_at: String,
    pub elapsed_sec: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_error: String,
    /// 用户手动点过「断开」→ 自动组网就别再替他拨回来（尊重人的意思）。
    pub manual_stop: bool,
    /// 这条链路是谁拉起来的（排障关键）：
    /// `modemd` = 本程序下发的拨号；`netifd` = 路由器按 UCI 配置拉的；
    /// 空 = 没链路 / 判不出。
    ///
    /// 早先只有 `state`，于是「netifd 按 static 配置把 eth2 拉起来、本程序从没拨过号」
    /// 会被显示成「未拨号」——实际上网是通的。现在用实际地址判定，并标出来源。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub link_source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<DialStep>,
    /// 链路阶段（细分说明，见 PHASE_* 常量）。
    ///
    /// 与 `state` 的分工：`state` 回答「拨号任务进行到哪了」（对外契约，
    /// 页面按它显示「已连接/未连接」）；`phase` 回答「网络真的通了吗」——
    /// 需要看主机网卡有没有地址、默认路由走不走它。空 = 旧快照，无此概念。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub phase_text: String,
}

impl DialState {
    /// 零值归一（没拨过号 → idle/未拨号）+ 重算「已连接多久」。
    pub fn normalized(mut self) -> Self {
        if self.state.is_empty() {
            self.state = DIAL_IDLE.to_string();
        }
        self.state_text = dial_state_text(&self.state);
        if self.state == DIAL_CONNECTED && !self.connected_at.is_empty() {
            self.elapsed_sec = seconds_since(&self.connected_at).unwrap_or(0) as i64;
        }
        self
    }
}

/// 一次拨号请求（HTTP 层与引擎共用）。
#[derive(Debug, Clone, Default)]
pub struct DialRequest {
    pub cid: i32,
    pub apn: String,
    pub pdp_type: String,
    /// 已激活的先断开再连，让刚写入的 APN 生效。
    pub reconnect: bool,
    /// 重新识别 USB 类型（不走缓存）。
    pub force: bool,
}

// ---------- USB 类型 → 拨号方式 ----------

/// 一种 USB 网卡类型（= `AT+QCFG="usbnet"` 的取值）对应的**拨号方式**。
///
/// `kind` 是归一化的协议名（qmi / ecm / mbim / rndis / ncm / unknown），
/// 界面与状态里用 `kind` 做判定，展示名用 `name`。
#[derive(Debug, Clone, Serialize)]
pub struct DialType {
    /// usbnet 取值，如 "1"。
    pub value: String,
    /// 展示名，如 "ECM"。
    pub name: String,
    /// 归一化协议名：qmi/ecm/mbim/rndis/ncm。
    pub kind: String,
    /// 主机内核里长什么样。
    pub protocol: String,
    /// 主机侧需要的驱动 / 拨号工具。
    pub host_driver: String,
    /// 主机侧要执行的拨号动作（照抄即可）。
    pub host_setup: String,
    /// 模组侧 AT 拨号（= 本页「拨号」按钮）是否够用。
    pub at_dial: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// 全部已知类型（页面用这份列表把当前取值翻成「该怎么拨」）。
pub fn dial_types() -> Vec<DialType> {
    vec![
        DialType {
            value: "0".into(),
            name: "QMI/NDIS".into(),
            kind: "qmi".into(),
            protocol: "rmnet（wwan0）".into(),
            host_driver: "qmi_wwan（或 qmi_wwan_q/f）+ modemd 内置 QMI 拨号；手工排障等价于 uqmi -d /dev/cdc-wdm0 --start-network；Windows 装高通或移远驱动".into(),
            host_setup: "主机侧拨号：modemd 自动发 QMI `WDS Start`（APN 来自「拨号上网」；QMAP 模组先发 WDA Set Data Format 并 Bind Mux），地址由 netifd 的 modem_pro（udhcpc）取；手工排障可用 uqmi -d /dev/cdc-wdm0 --start-network <APN>；Windows 由 WWAN 服务自动建立。".into(),
            at_dial: true,
            note: "高通原生协议，模组自己不发 DHCP；AT 侧激活 PDP 只代表模组这一端就绪。".into(),
        },
        DialType {
            value: "1".into(),
            name: "ECM".into(),
            kind: "ecm".into(),
            protocol: "cdc_ether（usb0 / eth1）".into(),
            host_driver: "Linux 内核自带 cdc_ether，免驱".into(),
            host_setup: "AT 侧拨号成功后在主机跑 DHCP：udhcpc -i usb0（OpenWrt：ifup usb0 / 建 usb0 接口设为 DHCP 客户端）。".into(),
            at_dial: true,
            note: "地址常由模组侧的 DHCP 下发；接口名以主机实际为准（usb0 / eth1 / enx…）。".into(),
        },
        DialType {
            value: "2".into(),
            name: "MBIM".into(),
            kind: "mbim".into(),
            protocol: "cdc_mbim（wwan0 + /dev/cdc-wdm0）".into(),
            host_driver: "cdc_mbim + modemd 内置 MBIM Basic Connect（Windows 自带 MBIM 类驱动）".into(),
            host_setup: "主机侧拨号：modemd 自己发 MBIM `OPEN → CONNECT → IP_CONFIGURATION`（**APN 必填**）。等价的手工命令：`umbim -d /dev/cdc-wdm0 connect`（netifd 的 mbim.sh 用的就是它）。".into(),
            at_dial: false,
            note: "MBIM 是微软标准，一套会话里既有拨号也有 IP 配置指令；模组的 DHCP 服务器通常也在链路上，所以 udhcpc 能拿到同一份地址。⚠️ 模组只有**一个**数据承载：模组侧先激活过 PDP 的话，MBIM_CID_CONNECT 会被拒（Status=2），得先把它收掉。".into(),
        },
        DialType {
            value: "3".into(),
            name: "RNDIS".into(),
            kind: "rndis".into(),
            protocol: "rndis_host（usb0 / eth1）".into(),
            host_driver: "Windows 免驱；Linux 需 rndis_host（部分固件兼容性一般）".into(),
            host_setup: "同 ECM：AT 侧拨号成功后在主机对该网卡跑 DHCP 即可。".into(),
            at_dial: true,
            note: "经典 USB 网卡，兼容性广但性能一般。".into(),
        },
        DialType {
            value: "5".into(),
            name: "NCM".into(),
            kind: "ncm".into(),
            protocol: "cdc_ncm（wwan0 / usb0）".into(),
            host_driver: "cdc_ncm + cdc_ether（Linux 自带；OpenWrt 装 kmod-usb-net-cdc-ncm）".into(),
            host_setup: "同 ECM：AT 侧拨号成功后在主机对该网卡跑 DHCP 即可。".into(),
            at_dial: true,
            note: "高速 USB 网络，带宽比 ECM/RNDIS 高。".into(),
        },
    ]
}

/// usbnet 取值 → 归一化协议名（表外/空值都是 unknown）。
pub fn dial_kind_of(value: &str) -> String {
    match value.trim() {
        "0" => "qmi",
        "1" => "ecm",
        "2" => "mbim",
        "3" => "rndis",
        "5" => "ncm",
        _ => "unknown",
    }
    .to_string()
}

/// 把**本机取值表里的展示名**归一成协议名。
///
/// 为什么需要它：各家的 usbnet 取值完全不同 —— 移远 `0` 是 QMI，而广和通 `32` 才是
/// QMI（`AT+GTUSBMODE` 那套）。DialTypes 表是按**移远取值**做 key 的，所以跨厂商时
/// 必须先按名字认出协议，再用协议查表。
pub fn dial_kind_from_name(name: &str) -> String {
    let s = name.trim().to_lowercase();
    if s.is_empty() {
        return String::new();
    }
    // ⚠️ rndis 必须排在 ndis 前面："RNDIS" 里就含 "ndis"，
    // 否则 RNDIS 会被判成 QMI（实测踩过）
    if s.contains("rndis") {
        return "rndis".into();
    }
    if s.contains("mbim") {
        return "mbim".into();
    }
    if s.contains("ecm") {
        return "ecm".into();
    }
    if s.contains("ncm") {
        return "ncm".into();
    }
    if s.contains("qmi") || s.contains("ndis") {
        return "qmi".into();
    }
    String::new()
}

/// 协议名的中文展示（没认出来就返回原值）。
pub fn dial_kind_label(kind: &str) -> String {
    match kind.trim().to_lowercase().as_str() {
        "qmi" => "QMI",
        "ecm" => "ECM",
        "mbim" => "MBIM",
        "rndis" => "RNDIS",
        "ncm" => "NCM",
        other => other,
    }
    .to_string()
}

/// 取某个取值的拨号方式（表外取值返回 None）。
pub fn dial_type_by_value(value: &str) -> Option<DialType> {
    let v = value.trim();
    dial_types().into_iter().find(|t| t.value == v)
}

/// 按归一化协议名取拨号方式（跨厂商时用这条；空 / unknown 返回 None）。
pub fn dial_type_by_kind(kind: &str) -> Option<DialType> {
    let k = kind.trim().to_lowercase();
    if k.is_empty() || k == "unknown" {
        return None;
    }
    dial_types().into_iter().find(|t| t.kind == k)
}

/// 这台模组的数据呼叫是不是归主机侧拨号器（QMI/MBIM 归主机侧拨号器，其余走模组侧 AT 激活）。
///
/// ⚠️ 只看类型，不看 `[net] host_dial`：host_dial 关掉时是「不许拨号器跑」，
/// 由 hostDialStopReason 负责拦住。
pub fn wan_host_dial_by_daemon(kind: &str) -> bool {
    matches!(kind.trim().to_lowercase().as_str(), "qmi" | "mbim")
}

// ---------- USB 描述符 ----------

/// 模组 USB 侧的实际描述（AT 取值拿不到时的兜底依据）。
///
/// 只看两样：VID/PID 与**各接口的类码**。类码是 USB-IF 的通用约定
/// （02/06 = CDC-ECM 控制口、02/0d = CDC-NCM 控制口、02/0e = MBIM、ef/04/01 = RNDIS；
/// 0a 只是 ECM/NCM/MBIM 共用的数据口），与厂商无关。
#[derive(Debug, Clone, Serialize, Default)]
pub struct UsbDesc {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub product: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// 拆一个「类码」字符串 → 类/子类/协议（都归一成两位小写十六进制）。
///
/// 容忍三种写法（三种来源各写各的）：
///   · `02/06/00`                            通用（sysfs 拼接、页面展示）
///   · `02-06-00`                            连字符
///   · `USB\Class_02&SubClass_06&Prot_00`    Windows 注册表 CompatibleIDs 原文
///
/// 只有类也能用（子类/协议留空），因为有些固件不填后两位。
pub fn split_usb_class_code(s: &str) -> (String, String, String, bool) {
    let s = s.trim();
    if s.is_empty() {
        return (String::new(), String::new(), String::new(), false);
    }
    // Windows CompatibleIDs 的写法：Class_xx&SubClass_yy&Prot_zz
    let upper = s.to_uppercase();
    if let Some(i) = upper.find("CLASS_") {
        let rest = &s[i + "CLASS_".len()..];
        let rest = rest
            .to_lowercase()
            .replace("subclass_", "")
            .replace("prot_", "");
        let fields: Vec<&str> = rest
            .split(['&', '/', '-'])
            .filter(|f| !f.is_empty())
            .collect();
        return normalize_hex_fields(&fields);
    }
    let fields: Vec<&str> = s
        .split(['/', '-', ',', ' ', ':'])
        .filter(|f| !f.is_empty())
        .collect();
    normalize_hex_fields(&fields)
}

/// 类码字段归一：两位小写十六进制（不够两位补前导零，不是 hex 原样保留）。
fn normalize_hex_fields(fields: &[&str]) -> (String, String, String, bool) {
    let norm = |f: &str| -> Option<String> {
        let f = f.trim();
        if f.len() > 2 || !f.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(format!("{:0>2}", f.to_lowercase()))
    };
    let class = fields.first().and_then(|f| norm(f));
    let Some(class) = class else {
        return (String::new(), String::new(), String::new(), false);
    };
    let sub = fields.get(1).and_then(|f| norm(f)).unwrap_or_default();
    let proto = fields.get(2).and_then(|f| norm(f)).unwrap_or_default();
    (class, sub, proto, true)
}

/// 接口类码 → usbnet 取值（越具体越优先）。
///
/// 第一轮：控制口子类 / RNDIS（02/0e→MBIM、02/0d→NCM、ef/04→RNDIS）；
/// 第二轮：CDC-ECM（02/06，必须在 0a 兜底之前）；
/// 第三轮：只剩 CDC 数据口（0a）→ 认不出 ECM / NCM / MBIM，按 NCM 处理。
pub fn classify_usb_classes(classes: &[String]) -> Option<&'static str> {
    for c in classes {
        let (class, sub, _, ok) = split_usb_class_code(c);
        if !ok {
            continue;
        }
        match (class.as_str(), sub.as_str()) {
            ("02", "0e") => return Some("2"),
            ("02", "0d") => return Some("5"),
            ("ef", "04") => return Some("3"),
            _ => {}
        }
    }
    for c in classes {
        let (class, sub, _, ok) = split_usb_class_code(c);
        if ok && class == "02" && sub == "06" {
            return Some("1");
        }
    }
    // ⚠️ 不再用「单独出现 0a（CDC Data）⇒ NCM」：
    // CDC Data 接口在 ECM / NCM / MBIM 里都会出现，单独一条定不了类型。
    // 实测 FM350-GL 的接口是 `02/02/ff`（ACM）、`0a/00/00`（Data）、`ff/42/01`（私有 RNDIS）
    // —— 按老规则会被判成 NCM，而它实际是 RNDIS（主机侧驱动 rndis_host）。
    // 数据面类型交给「主机侧驱动 > 描述符标准类码 > 机型兜底」这条链（见 `usb_type`）。
    None
}

/// 有没有厂商自定义接口（类码 ff）—— 只有它时按 QMI/NDIS 推断。
pub fn has_vendor_interface(classes: &[String]) -> bool {
    classes.iter().any(|c| {
        let (class, _, _, ok) = split_usb_class_code(c);
        ok && class == "ff"
    })
}

/// 把接口类码翻成人话（页面「USB 描述」一栏）。认不出的原样回传，不做猜测。
///
/// ⚠️ CDC 设备里「协议」写在**控制接口（class 02）的子类**上，数据接口（class 0a）是共用的：
/// 02/0d = NCM 控制口、02/06 = ECM 控制口、02/0e = MBIM 控制口、
/// 0a = 数据口（ECM / NCM / MBIM 都长这样，光看它认不出协议）。
pub fn class_summary(code: &str) -> String {
    let (class, sub, _, ok) = split_usb_class_code(code);
    if !ok {
        return code.to_string();
    }
    let sub_text = if sub.is_empty() { "-" } else { sub.as_str() };
    let label = match (class.as_str(), sub.as_str()) {
        ("02", "0d") => "CDC-NCM 控制接口",
        ("02", "0e") => "CDC-MBIM 控制接口",
        ("02", "06") => "CDC-ECM 控制接口",
        ("02", "02") => "CDC-ACM 串口（AT 口）",
        ("02", _) => return format!("{code} CDC 控制接口（子类 {sub_text}）"),
        ("0a", _) => "CDC 数据接口（ECM / NCM / MBIM 共用）",
        ("ef", "04") => "RNDIS",
        ("ff", "06") => "厂商自定义串口（AT / 诊断口）",
        ("ff", _) => "厂商自定义（AT / QMI / 诊断口）",
        ("08", _) => "大容量存储（U 盘）",
        ("0e", _) => "视频",
        ("01", _) => "音频",
        ("03", _) => "HID",
        ("e0", _) => "无线控制器（蓝牙等）",
        ("00", _) => "未指定（在接口级别定义）",
        _ => return code.to_string(),
    };
    format!("{code} {label}")
}

/// 一串类码 → 一串人话（去重，保持原顺序）。
pub fn class_summary_list(classes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in classes {
        let s = class_summary(c);
        if s.is_empty() || out.contains(&s) {
            continue;
        }
        out.push(s);
    }
    out
}

// ---------- USB 类型识别结果 ----------

/// 主机侧网卡探测结果（读 sysfs 才有值；见 [`host_iface_hint`]）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct HostIfaceHint {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub iface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub driver: String,
    /// 据驱动推断出的 usbnet 取值（认不出为空串 = 不猜）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub product: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
}

/// USB 类型识别的完整结果（`GET /api/dial` 的 `usb` 字段）。
///
/// 三条识别路：
///   ① 问固件 `AT+QCFG="usbnet"`（source=at，最权威）；
///   ② 读 USB 描述符的接口类码（source=descriptor）；
///   ③ 读主机侧网卡的驱动（source=hostiface）。
/// 协议的判定顺序：**AT 取值表里的名字** → usbnet 取值表 → 主机侧驱动名。
#[derive(Debug, Clone, Serialize, Default)]
pub struct DialTypeInfo {
    pub modem_id: u32,
    /// usbnet 取值（认不出为空串）
    pub value: String,
    /// 取值表里的展示名（如 "QMI/NDIS"）
    pub name: String,
    /// 归一化协议名：qmi / ecm / mbim / rndis / ncm / unknown
    pub kind: String,
    /// 识别依据：at / descriptor / hostiface / none
    pub source: String,
    pub confident: bool,
    /// 本机实际用的查询指令（目前只有移远系 `AT+QCFG="usbnet"` 一条路）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub query_cmd: String,
    /// 该类型的拨号方式（取值/协议查表；认不出为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dial: Option<DialType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<UsbDesc>,
    /// 主机侧网卡探测结果（页面字段名沿用上游 `host_iface_hint`）
    #[serde(rename = "host_iface_hint", skip_serializing_if = "Option::is_none")]
    pub host_iface: Option<HostIfaceHint>,
    /// 总线：usb（PCIe 识别属后续阶段，暂不产出）
    pub bus: String,
    /// 命中模块支持清单时给出本型号（型号不在清单里就留空，不猜）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub catalog_model: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub catalog_usb: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub catalog_pcie: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub probed_at: String,
}

/// usbnet 取值 → 展示名（跨厂商通用表）。
///
/// ⚠️ 与 [`super::usbnet`] 页面那张表的 `RMNET` 口径不同：这里要拿名字反推协议
/// （`dial_kind_from_name`），"QMI/NDIS" 才能认出 qmi，"RMNET" 认不出。
pub fn usbnet_mode_name(value: &str) -> &'static str {
    match value.trim() {
        "0" => "QMI/NDIS",
        "1" => "ECM",
        "2" => "MBIM",
        "3" => "RNDIS",
        "5" => "NCM",
        _ => "",
    }
}

// ---------- 主机侧网卡（sysfs，不发 AT） ----------

/// 一张主机网卡的快照。
#[derive(Debug, Clone, Default)]
pub struct IfaceInfo {
    pub name: String,
    pub up: bool,
    /// 强候选（名字里直接指向 USB / WWAN）
    pub strong: bool,
    pub addr: Vec<String>,
}

/// 拒绝名单：名字里出现这些词的一定不是模组数据口。
const DIAL_IFACE_REJECT: &[&str] = &[
    "loopback",
    "docker",
    "veth",
    "vethernet",
    "wsl",
    "bluetooth",
    "蓝牙",
    "wi-fi",
    "wifi",
    "wlan",
    "wireless",
    "无线",
    // OpenWrt 的无线接口不一定叫 wlan*（新命名是 phy0-ap0 / phy1-mesh0 这种）
    "phy",
    "-ap",
    "mesh",
    "virtualbox",
    "vmware",
    "vmnet",
    "hyper-v",
    "tap",
    "tun",
    "tailscale",
    "zerotier",
    "openvpn",
    "wireguard",
    "hamachi",
    "npcap",
    "radmin",
    "default switch",
    "bridge",
    "br-",
    "virbr",
    "npf",
    "vpn",
];

/// 强候选：名字里直接指向 USB / WWAN（可信度高）。
const DIAL_IFACE_STRONG: &[&str] = &[
    "wwan", "rmnet", "wwx", "usb", "enx", "rndis", "cdc-wdm", "qmi",
];

/// 弱候选：通用以太网名（只作参考）。
const DIAL_IFACE_WEAK: &[&str] = &["eth", "en", "以太网", "ethernet"];

/// 这张网卡像不像模组的数据口。第二个返回值表示「可信」（强候选）。
pub fn match_dial_iface(name: &str) -> (bool, bool) {
    let n = name.trim().to_lowercase();
    if n.is_empty() {
        return (false, false);
    }
    for bad in DIAL_IFACE_REJECT {
        if n.contains(bad) {
            return (false, false);
        }
    }
    for p in DIAL_IFACE_STRONG {
        if n.contains(p) {
            return (true, true);
        }
    }
    for p in DIAL_IFACE_WEAK {
        if n.contains(p) {
            return (true, false);
        }
    }
    (false, false)
}

/// 读某块网卡的 IPv4 地址（带 5 秒缓存）。
///
/// ## 为什么用外部 `ip` 而不是 sysfs
///
/// `/sys/class/net/<if>/` 里**没有 IP 地址**（只有 MAC/统计/operstate）；
/// IPv6 能从 `/proc/net/if_inet6` 读到，IPv4 则必须走 `getifaddrs` ——
/// Rust 标准库没有这个接口，而本仓库刻意不引 libc 依赖（OpenWrt 交叉编译越省越好）。
///
/// 于是用 `ip -4 -o addr show dev <dev>`：OpenWrt 自带 iproute2，输出稳定，
/// 一行一台地址。**带 5 秒缓存**是因为 `/api/dial` 与 `/api/snapshot` 都会被
/// 页面轮询，不缓存就会每 3 秒 fork 一个进程 —— 在 MT7621 这类小机器上很浪费。
fn iface_addrs_cached(dev: &str) -> Vec<String> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(String, Instant, Vec<String>)>> = Mutex::new(None);

    if dev.trim().is_empty() {
        return Vec::new();
    }
    {
        let g = CACHE.lock().unwrap();
        if let Some((d, at, addrs)) = g.as_ref() {
            if d == dev && at.elapsed() < Duration::from_secs(5) {
                return addrs.clone();
            }
        }
    }
    let addrs = read_iface_addrs(dev);
    *CACHE.lock().unwrap() = Some((dev.to_string(), Instant::now(), addrs.clone()));
    addrs
}

/// 真正执行 `ip -4 -o addr show dev <dev>` 并解析。
fn read_iface_addrs(dev: &str) -> Vec<String> {
    let out = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", "dev", dev])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut v = Vec::new();
    for line in text.lines() {
        // 形如：`2: eth2    inet 10.42.0.16/24 brd 10.42.0.255 scope global eth2`
        let Some(pos) = line.find(" inet ") else {
            continue;
        };
        let rest = &line[pos + 6..];
        let addr = rest.split_whitespace().next().unwrap_or("");
        if addr.is_empty() {
            continue;
        }
        v.push(addr.to_string());
    }
    v
}

/// netifd 接口在线时长（秒）—— `ifstatus <iface>` 的 `uptime`，带 5 秒缓存。
///
/// 这才是页面「连接时间」该跟随的「接口时间」，见 `dial_status()` 第 ⑤ 步：
/// 模组会话 `connected_at` 会因重拨/接口重建与接口真实在线时间对不上，内存
/// 观测起点又会因 modemd 重启归零；netifd 的 uptime 跨重启保持正确。
/// 与 `iface_addrs_cached` 同款缓存理由：`/api/dial`、`/api/snapshot` 被轮询，
/// 不缓存就会每次 fork 一个 `ifstatus`。
fn netifd_iface_uptime_cached(iface: &str) -> Option<u64> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(String, Instant, Option<u64>)>> = Mutex::new(None);

    let iface = iface.trim();
    if iface.is_empty() {
        return None;
    }
    {
        let g = CACHE.lock().unwrap();
        if let Some((n, at, up)) = g.as_ref() {
            if n == iface && at.elapsed() < Duration::from_secs(5) {
                return *up;
            }
        }
    }
    let up = read_iface_uptime(iface);
    *CACHE.lock().unwrap() = Some((iface.to_string(), Instant::now(), up));
    up
}

/// 真正执行 `ifstatus <iface>` 并解析 JSON 里的 `uptime`（秒）。
fn read_iface_uptime(iface: &str) -> Option<u64> {
    let out = std::process::Command::new("ifstatus").arg(iface).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_iface_uptime(&String::from_utf8_lossy(&out.stdout))
}

/// 从 `ifstatus` 的 JSON 输出里取 `uptime`（秒）。0 / 缺失 / 非数字 ⇒ None。
///
/// 单独成函数是为了能单测（`ifstatus` 是外部命令，测试里跑不了）。
fn parse_iface_uptime(json: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let up = match v.get("uptime") {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    up.filter(|&u| u > 0)
}

/// 扫一遍主机网卡，挑出可能是模组数据口的那些（强候选在前，其余按名字排）。
///
/// 只是**提示**：真正把网卡拉起来要看主机侧的驱动/拨号器（见 [`DialType::host_setup`]），
/// 本程序不去改主机的路由/接口（那属于路由器固件该干的事）。
///
/// ⚠️ 已知差距（如实记录）：枚举接口能同时拿到**地址**，
/// Rust 标准库没有等价的枚举接口，`addr` 暂为空（主机侧 IP 由拨号器/DHCP 配置，
/// 见页面「拨号上网」卡片）。`up` 用 sysfs 的 `operstate != "down"` 近似
/// （IFF_UP 的管理态在 sysfs 里没有单独文件）。
pub fn dial_iface_snapshot() -> Vec<IfaceInfo> {
    let mut out: Vec<IfaceInfo> = Vec::new();
    let entries = match std::fs::read_dir("/sys/class/net") {
        Ok(e) => e,
        Err(_) => return out,
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let (ok, strong) = match_dial_iface(&name);
        if !ok {
            continue;
        }
        let up = std::fs::read_to_string(e.path().join("operstate"))
            .map(|s| s.trim() != "down")
            .unwrap_or(false);
        // addr 现在是真的（带缓存）：早先留空，导致「链路有没有地址」判不出来，
        // 拨号状态只能退化成「接口 UP 就算连着」——那正是用户报的「读不到状态」。
        let addr = iface_addrs_cached(&name);
        out.push(IfaceInfo {
            name,
            up,
            strong,
            addr,
        });
    }
    out.sort_by(|a, b| match (a.strong, b.strong) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out
}

/// 从候选里挑一个最像的：优先「已 up」的强候选，其次任意强候选，最后弱候选。
pub fn pick_dial_iface(list: &[IfaceInfo]) -> Option<IfaceInfo> {
    let rank = |i: &IfaceInfo| -> u8 {
        match (i.strong, i.up, i.addr.is_empty()) {
            (true, true, _) => 0,
            (true, _, _) => 1,
            (false, true, false) => 2,
            _ => 3,
        }
    };
    let mut best: Option<&IfaceInfo> = None;
    for i in list {
        if best.map(|b| rank(i) < rank(b)).unwrap_or(true) {
            best = Some(i);
        }
    }
    best.cloned()
}

// ---------- sysfs 小工具（读不到就返回空，跨平台行为一致） ----------

/// 读 sysfs 文本文件的首行（sysfs 的「一行一个值」）。
fn read_sysfs_line(path: &std::path::Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(s) => s.lines().next().unwrap_or("").trim().to_string(),
        Err(_) => String::new(),
    }
}

/// 读 sysfs 里的一个十六进制**字节**值（归一成两位小写）。
///
/// ⚠️ 只用于 bInterfaceClass 这类单字节属性 —— VID/PID 是 **4 位**，
/// 必须用 [`read_sysfs_id`]（实测踩过：用错函数 VID/PID 永远是空）。
fn read_sysfs_hex(path: &std::path::Path) -> String {
    let s = read_sysfs_line(path).to_lowercase();
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::new();
    }
    format!("{:0>2}", s)
}

/// 读 sysfs 里的 USB ID（idVendor / idProduct = 4 位十六进制，小写归一）。
fn read_sysfs_id(path: &std::path::Path) -> String {
    let s = read_sysfs_line(path).to_lowercase();
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return String::new();
    }
    s
}

/// USB 接口目录名（`1-1:1.0`；测试/Windows 退化写法 `1-1.0` 也认）。
fn is_usb_interface_dir_name(name: &str) -> bool {
    name.contains(':') || (name.contains('-') && name.contains('.'))
}

/// 读一个目录的接口类码，拼成 `02/06/00`（读不到返回空串）。
///
/// 接口目录优先用 bInterface*；设备目录只有 bDevice*（非复合设备）。
fn usb_interface_code(dir: &std::path::Path) -> String {
    let mut class = read_sysfs_hex(&dir.join("bInterfaceClass"));
    let mut sub = read_sysfs_hex(&dir.join("bInterfaceSubClass"));
    let mut proto = read_sysfs_hex(&dir.join("bInterfaceProtocol"));
    if class.is_empty() {
        class = read_sysfs_hex(&dir.join("bDeviceClass"));
        sub = read_sysfs_hex(&dir.join("bDeviceSubClass"));
        proto = read_sysfs_hex(&dir.join("bDeviceProtocol"));
    }
    if class.is_empty() {
        return String::new();
    }
    format!("{class}/{sub}/{proto}")
}

/// 从一个 sysfs 路径往上找含 idVendor 的目录（= USB 设备目录）。非 USB 设备找不到。
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

/// AT 口 → USB 设备目录（往上逐层找到含 idVendor 的那层）。
///
/// 返回 None 表示这个口不是 USB 串口（主板物理口 / 蓝牙虚拟口 / 名字不是 tty*）。
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

/// 读模组 USB 侧的实际描述（VID/PID + 各接口类码）。
///
/// 拿不到（不是 USB 串口 / 没有 sysfs）时返回 None，调用方退化成「类型未知」而不是报错。
pub fn usb_descriptor_for_port(port: &str) -> Option<UsbDesc> {
    let dev = usb_device_dir(port)?;
    let mut d = UsbDesc {
        source: "sysfs".into(),
        ..UsbDesc::default()
    };
    d.vid = read_sysfs_id(&dev.join("idVendor"));
    d.pid = read_sysfs_id(&dev.join("idProduct"));
    d.product = read_sysfs_line(&dev.join("product"));
    // 各接口（1-1:1.0 / 1-1:1.1 …）的类码
    if let Ok(entries) = std::fs::read_dir(&dev) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() || !is_usb_interface_dir_name(&name) {
                continue;
            }
            let code = usb_interface_code(&e.path());
            if !code.is_empty() {
                d.classes.push(code);
            }
        }
    }
    if d.classes.is_empty() {
        // 非复合设备（或老内核不在接口目录放类码）→ 退回设备级类码
        let code = usb_interface_code(&dev);
        if !code.is_empty() {
            d.classes.push(code);
        }
    }
    d.classes = reduce_usb_codes(&d.classes);
    if d.vid.is_empty() && d.classes.is_empty() {
        return None;
    }
    Some(d)
}

/// 这个 AT 口对应的 USB 设备**持有的网卡名**（`<usb dir>/net/*`，排序后）。
///
/// 组网要按模组逐个绑 `device`，而「主机侧网卡候选」是**全局扫描**、分不清哪个
/// 口属于哪个模组 —— 多模组时会把两张卡都绑到 `modem1` 上。sysfs 的 `net/`
/// 子目录按 USB 设备组织，从这里取才是准的。
///
/// QMAP 模组会返回两张（`wwan0` 父口 + `wwan0_1` mux 数据口）：两个都交出去，
/// 由调用方按 `qmap_mode` 挑数据口（见 `netprov/provision.rs`）。
pub fn usb_netifs_for_port(port: &str) -> Vec<String> {
    let Some(dev) = usb_device_dir(port) else {
        return Vec::new();
    };
    let mut out = Vec::new();

    // ① USB **device** 层：/sys/bus/usb/devices/2-1/net/
    //
    //    少数固件把网卡挂在这一层，保留是为了兼容。
    collect_net_names(&dev, &mut out);

    // ② USB **interface** 层：/sys/bus/usb/devices/2-1/2-1:1.0/net/
    //
    //    ⚠️ **这才是常见位置**，早先漏了这一层，导致 RNDIS 模组永远
    //    推不出数据网卡 —— 实机 FM350-GL：AT 口在 `2-1:1.6`，而
    //    `eth2` 的 `net/` 挂在 `2-1:1.0`（另一个 interface）。只查 device 层
    //    拿到空列表，`host_iface` 就为空，拨号页显示「状态未知」而实际
    //    链路早就通了（eth2 有地址、默认路由也在走它）。
    //
    //    QMI 的 `wwan0` 同理（在 QMI interface 下）。
    if let Ok(entries) = std::fs::read_dir(&dev) {
        for e in entries.flatten() {
            let sub = e.path();
            if sub.is_dir() {
                collect_net_names(&sub, &mut out);
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

/// 把 `dir/net/*` 里的网卡名收进 `out`（目录不存在就是空操作）。
fn collect_net_names(dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir.join("net")) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.is_empty() {
            out.push(name);
        }
    }
}

/// 去重 + 收敛成最具体的那条类码（同一类里，别人比它具体 → 它是简写，去掉）。
pub fn reduce_usb_codes(codes: &[String]) -> Vec<String> {
    let mut triples: Vec<(String, String, String)> = Vec::new();
    for c in codes {
        let (class, sub, proto, ok) = split_usb_class_code(c);
        if ok {
            triples.push((class, sub, proto));
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (i, t) in triples.iter().enumerate() {
        let mut shorthand = false;
        for (j, u) in triples.iter().enumerate() {
            if i == j || u.0 != t.0 {
                continue;
            }
            if t.1.is_empty() && !u.1.is_empty() {
                shorthand = true;
                break;
            }
            if t.2.is_empty() && !u.2.is_empty() && u.1 == t.1 {
                shorthand = true;
                break;
            }
        }
        if shorthand {
            continue;
        }
        let mut code = t.0.clone();
        if !t.1.is_empty() {
            code.push('/');
            code.push_str(&t.1);
        }
        if !t.2.is_empty() {
            code.push('/');
            code.push_str(&t.2);
        }
        if !out.contains(&code) {
            out.push(code);
        }
    }
    out.sort();
    out
}

/// 主机侧网卡**驱动名** → usbnet 取值（认不出返回空串 = 不猜）。
///
/// 驱动名是内核给的、与厂商无关，所以可以据此断定协议：
/// QMI 只有 qmi_wwan（或老 GobiNet）在管，MBIM 只有 cdc_mbim，ECM 只有 cdc_ether。
pub fn usbnet_from_host_driver(driver: &str) -> String {
    let d = driver.trim().to_lowercase();
    if d.is_empty() {
        return String::new();
    }
    let exact = match d.as_str() {
        "qmi_wwan" | "gobinet" | "gobi" => "0",
        "cdc_mbim" => "2",
        "cdc_ether" | "cdc_subset" => "1",
        "rndis_host" => "3",
        "cdc_ncm" | "huawei_cdc_ncm" => "5",
        _ => "",
    };
    if !exact.is_empty() {
        return exact.to_string();
    }
    // 有些内核/定制固件给驱动名加后缀（如 qmi_wwan_q / cdc_mbim_qcom）
    if d.starts_with("qmi_wwan") || d.starts_with("gobinet") {
        return "0".into();
    }
    if d.starts_with("cdc_mbim") {
        return "2".into();
    }
    if d.starts_with("cdc_ncm") || d.ends_with("_cdc_ncm") {
        return "5".into();
    }
    if d.starts_with("cdc_ether") || d.starts_with("cdc_subset") {
        return "1".into();
    }
    if d.starts_with("rndis") {
        return "3".into();
    }
    String::new()
}

/// 从网卡 uevent 里取 DRIVER=xxx（读不到返回空串）。
fn net_uevent_driver(path: &std::path::Path) -> String {
    let Ok(s) = std::fs::read_to_string(path) else {
        return String::new();
    };
    for line in s.lines() {
        if let Some(v) = line.trim().strip_prefix("DRIVER=") {
            return v.trim().to_string();
        }
    }
    String::new()
}

/// 读单张网卡的驱动名 + USB 祖先信息（读不到返回 None）。
fn read_host_iface_hint(name: &str) -> Option<HostIfaceHint> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let base = std::path::Path::new("/sys/class/net").join(name);
    if !base.exists() {
        return None;
    }
    let mut h = HostIfaceHint {
        iface: name.to_string(),
        ..HostIfaceHint::default()
    };
    h.driver = net_uevent_driver(&base.join("uevent"));
    if h.driver.is_empty() {
        if let Ok(real) = std::fs::canonicalize(base.join("device").join("driver")) {
            h.driver = real
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
        }
    }
    h.value = usbnet_from_host_driver(&h.driver);
    // USB 祖先目录（网卡绑在 USB 接口上时才有）：VID/PID/产品名 + 各接口类码
    if let Some(dev) = usb_ancestor_dir(&base.join("device")) {
        h.vid = read_sysfs_id(&dev.join("idVendor"));
        h.pid = read_sysfs_id(&dev.join("idProduct"));
        h.product = read_sysfs_line(&dev.join("product"));
        if let Ok(entries) = std::fs::read_dir(&dev) {
            for e in entries.flatten() {
                let ename = e.file_name().to_string_lossy().to_string();
                if !e.path().is_dir() || !is_usb_interface_dir_name(&ename) {
                    continue;
                }
                let code = usb_interface_code(&e.path());
                if !code.is_empty() {
                    h.classes.push(code);
                }
            }
        }
        if h.classes.is_empty() {
            let code = usb_interface_code(&dev);
            if !code.is_empty() {
                h.classes.push(code);
            }
        }
        h.classes = reduce_usb_codes(&h.classes);
    }
    if h.driver.is_empty() && h.vid.is_empty() && h.classes.is_empty() {
        return None;
    }
    Some(h)
}

/// 扫本机网卡，挑一张「像模组数据口」的，读它的驱动名与 USB 祖先。
///
/// 优先返回**能据驱动判定协议**的那张（如 wwan0 + qmi_wwan）；
/// 一张都判不出来时，返回有名字信息的那张（至少能把 VID:PID 补上）。
pub fn host_iface_hint() -> Option<HostIfaceHint> {
    let mut fallback: Option<HostIfaceHint> = None;
    for c in dial_iface_snapshot() {
        let Some(h) = read_host_iface_hint(&c.name) else {
            continue;
        };
        if !h.value.is_empty() {
            return Some(h);
        }
        if fallback.is_none() && (!h.driver.is_empty() || !h.vid.is_empty()) {
            fallback = Some(h);
        }
    }
    fallback
}

/// 按**这个模组**的 USB 设备取主机侧网卡驱动提示（多模组时不会串）。
///
/// `usb_netifs_for_port` 只列该 USB 设备 `net/` 下的网卡；QMAP 模组会同时给出
/// 父口与 mux 口，优先挑 mux 数据口（地址在它上面）。
pub fn iface_hint_for_port(port: &str) -> Option<HostIfaceHint> {
    let names = usb_netifs_for_port(port);
    let mut fallback: Option<HostIfaceHint> = None;
    for n in &names {
        let Some(h) = read_host_iface_hint(n) else {
            continue;
        };
        if !h.value.is_empty() {
            return Some(h);
        }
        if fallback.is_none() {
            fallback = Some(h);
        }
    }
    fallback
}

/// 机型特例：广和通 350 系列（FM350 / RW350）主机侧 DHCP 很慢/不稳。
///
/// 判据统一走 [`is_fibocom_350`]，与 `hostnic::quirk` / `is_fibocom_mtk` 同源。
pub fn host_nic_quirk(model: &str) -> Option<&'static str> {
    if super::modem::is_fibocom_350(model) {
        return Some(
            "广和通 350 系列（FM350 / RW350）的 USB 网卡 DHCP 很慢/不稳：实测要么几十秒到一两分钟才回，\
             要么干脆不回 —— 主机侧就长期挂在 169.254.x.x 上、没有默认路由，模组侧拨号成功也上不了网。\
             两条路：等它 DHCP（「先试 DHCP」），或把网卡 IP 手工配成与模组对应（同网段 + 网关指向模组）。",
        );
    }
    None
}

// ---------- 拨号前置体检 ----------

/// 拨号前置体检结果：把「拨号失败」还原成「卡在哪一步之前」。
///
/// 只在**失败路径**调用（成功路径零成本），因为激活 PDP 失败的原因十有八九
/// 不在 PDP 本身，而在更前面的 SIM / 飞行模式 / 注册态。缺了这一步，
/// 界面上只会看到一句「激活失败：+CME ERROR: xxx」，排障时无从下手。
#[derive(Debug, Default, Clone, Serialize)]
pub struct DialReadiness {
    /// `AT+CPIN?` 是否 READY（None = 问不出来）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_ready: Option<bool>,
    /// `AT+CFUN?` 的第一个字段（1 = 全功能，4 = 飞行模式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cfun: Option<i32>,
    /// PS 注册态（`AT+CEREG?` 第 2 字段；1 = 归属已注册，5 = 漫游已注册）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reg_state: Option<i32>,
    /// 三条原始应答，排障时直接贴出来
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<(String, String)>,
}

impl DialReadiness {
    /// 三项全中就绪。
    pub fn ok(&self) -> bool {
        self.sim_ready == Some(true)
            && self.cfun != Some(4)
            && self.reg_state.is_some_and(|s| s == 1 || s == 5)
    }

    /// 未就绪的中文原因串；全部就绪时返回空串。
    pub fn reason(&self) -> String {
        let mut why = Vec::new();
        match self.sim_ready {
            Some(false) => why.push("SIM 卡未就绪（AT+CPIN? 不是 READY，可能要输 PIN 或卡没插好）"),
            None => why.push("读不到 SIM 卡状态（AT+CPIN? 无应答）"),
            Some(true) => {}
        }
        if self.cfun == Some(4) {
            why.push("模组处于飞行模式（AT+CFUN? = 4），要先退出");
        }
        match self.reg_state {
            Some(s) if s == 1 || s == 5 => {}
            Some(_) => why.push("模组未注册上网络（AT+CEREG? 第 2 字段既不是 1 也不是 5）"),
            None => why.push("读不到注册状态（AT+CEREG? 无应答）"),
        }
        why.join("；")
    }
}

impl Modem {
    /// 做一次拨号前置体检（三条 AT，只在拨号失败时调）。
    ///
    /// 注册态必须问 `AT+CEREG?`：5G 附着时 `AT+CREG?` 可能恒回 `0,0`，
    /// 拿它当判据会把「已注册」误判成「未注册」。某家没有 CEREG 时
    /// 回落 CREG / CGREG（复用注册态解析的分派）。
    pub fn dial_readiness(&self) -> DialReadiness {
        let mut rd = DialReadiness::default();

        let pin = self.at_cmd_prio(Prio::Data, "AT+CPIN?");
        rd.sim_ready = Some(pin.lines().any(|l| l.to_uppercase().contains("READY")));
        rd.raw
            .push(("AT+CPIN?".to_string(), pin.trim().to_string()));

        let cfun = self.at_cmd_prio(Prio::Data, "AT+CFUN?");
        // 形如 `+CFUN: 1`；飞行模式是 4
        rd.cfun = cfun
            .lines()
            .find(|l| l.contains("CFUN"))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.trim().split(',').next().unwrap_or("").trim())
            .and_then(|v| v.parse::<i32>().ok());
        rd.raw
            .push(("AT+CFUN?".to_string(), cfun.trim().to_string()));

        // CEREG 优先；回显里没有就换 CREG / CGREG 再试
        let mut reg: Option<i32> = None;
        let mut used = String::new();
        let mut resp = self.at_cmd_prio(Prio::Data, "AT+CEREG?");
        if resp.contains("+CEREG") {
            used = "AT+CEREG?".to_string();
            reg = Some(regstate::parse(&resp, "+CEREG").stat);
        } else {
            for (cmd, tag) in [("AT+CGREG?", "+CGREG"), ("AT+CREG?", "+CREG")] {
                let r = self.at_cmd_prio(Prio::Data, cmd);
                if r.contains(tag.trim_start_matches('+')) {
                    used = cmd.to_string();
                    resp = r;
                    reg = Some(regstate::parse(&resp, tag).stat);
                    break;
                }
            }
        }
        rd.reg_state = reg;
        rd.raw.push((used, resp.trim().to_string()));

        rd
    }
}

// ---------- AT+CGPADDR 解析（三种固件写法） ----------

/// 粗校验：可用的点分十进制 IPv4（恰好 4 段、0~255、不是全零）。
pub fn looks_like_ip(s: &str) -> bool {
    let s = s.trim();
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let mut all_zero = true;
    for p in parts {
        match p.parse::<u32>() {
            Ok(n) if n <= 255 => {
                if n != 0 {
                    all_zero = false;
                }
            }
            _ => return false,
        }
    }
    !all_zero
}

/// 拆一个地址字段 → (ipv4, ipv6)。三种固件写法都认：
///
///   · 标准：`"10.42.1.67","2001:db8:1:2::8"`（两引号字段）
///   · 双栈挤在一个字段里，空格分隔
///   · 16 段点分十进制 = IPv6（广和通 / 移远部分固件的怪写法）
///
/// 两个家族都走 [`super::addr`] 的归一化，与 `AT+CGCONTRDP` 那条路
/// （`network::split_addr`）同一套判据；只报接口标识（前 8 字节全 0）
/// 的地址按项目惯例**不显示**，实测见 `addr` 的模块文档。
pub fn split_addrs(field: &str) -> (String, String) {
    let mut ipv4 = String::new();
    let mut ipv6 = String::new();
    for part in field.split(',') {
        let part = part.trim().trim_matches('"').trim();
        if part.is_empty() {
            continue;
        }
        // 标准写法（含冒号）走统一归一化：结构不合格（`gg::1`、两个 `::`、
        // 组数不对、组过长）与占位地址（`::` / `::1`）都在这里被拦下。
        // 比 16 字节十进制那种怪写法更常见，所以先判它。
        if part.contains(':') {
            if ipv6.is_empty() {
                if let Some(v6) = addr::normalize_ipv6(part) {
                    ipv6 = v6;
                }
            }
            continue;
        }
        let segs: Vec<&str> = part.split('.').collect();
        if segs.len() == 4 && looks_like_ip(part) && ipv4.is_empty() {
            ipv4 = part.to_string();
            continue;
        }
        // 16 段点分十进制 = IPv6（广和通 / 移远部分固件的怪写法）。
        // 解码与「不可信地址」判据都在 addr 里，与 AT+CGCONTRDP 那条路
        // （network::split_addr）共用同一套 —— 同一份固件输出不能因为
        // 来自不同指令就解出两个样。
        if segs.len() == 16 && ipv6.is_empty() {
            if let Some(v6) = addr::normalize_ipv6(part) {
                ipv6 = v6;
            }
        }
    }
    (ipv4, ipv6)
}

/// 从 `AT+CGPADDR` 应答里取主数据上下文的 IPv4/IPv6。
///
/// 优先 preferCID（通常 cid=1），拿不到再取第一个非空的上下文。
pub fn parse_cgpaddr(resp: &str, prefer_cid: i32) -> (String, String) {
    let mut first: (String, String) = (String::new(), String::new());
    for line in resp.lines() {
        let Some(field) = extract_field(line, "+CGPADDR") else {
            continue;
        };
        let f = csv(&field);
        let Some(cid) = f.first().and_then(|s| s.trim().parse::<i32>().ok()) else {
            continue;
        };
        // ⚠️ 地址是第一个逗号之后的**全部**字段：可能有两个
        // 引号字段（IPv4 + IPv6），csv 已把引号去掉，这里用逗号拼回去交给 split_addrs。
        let addrs = f[1..].join(",");
        let (v4, v6) = split_addrs(&addrs);
        if v4.is_empty() && v6.is_empty() {
            continue;
        }
        if cid == prefer_cid {
            return (v4, v6);
        }
        if first.0.is_empty() && first.1.is_empty() {
            first = (v4, v6);
        }
    }
    first
}

// ---------- IMS 上下文 ----------

/// APN 名字看起来是 IMS：`ims` / `IMS` / `ims.apn` / `ims-xxx`。
///
/// 以 ims 开头，且紧接着不是字母数字（`imsfoo` 不算，防止误伤普通 APN）。
pub fn ims_apn_name(apn: &str) -> bool {
    let s = apn.trim().to_lowercase();
    let Some(rest) = s.strip_prefix("ims") else {
        return false;
    };
    match rest.chars().next() {
        None => true,
        Some(c) => !(c.is_ascii_lowercase() || c.is_ascii_digit()),
    }
}

/// 从上下文列表里挑出「IMS 上下文」的 cid（去重、升序）。
///
/// 判据：
///   1. APN 就是 ims 的各种写法；
///   2. 内部上下文（被标成 IMS/sos）且 **cid == 1**、APN 为空 —— 有些固件 IMS 那条
///      不写 APN；3GPP 的惯例是 cid 1 留给 IMS。别的空 APN 上下文只是「没配置」。
pub fn ims_context_cids(ctxs: &[Context]) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    for c in ctxs {
        let hit = ims_apn_name(&c.apn) || (c.cid == 1 && c.apn.trim().is_empty() && c.internal);
        if !hit || out.contains(&c.cid) {
            continue;
        }
        out.push(c.cid);
    }
    out.sort_unstable();
    out
}

/// 一次「顺带激活 IMS」的结果（给步骤/日志用）。
#[derive(Debug, Clone)]
pub struct ImsActivateResult {
    /// ok（激活成功或本来就在激活状态）/ skip（没有 IMS 上下文）/ fail
    pub status: String,
    pub note: String,
}

impl Modem {
    /// 把 IMS 上下文激活起来（拨号后调用）。
    ///
    /// `ctxs` 传调用方**刚读到**的上下文列表（省一条 `AT+CGDCONT?`）。
    /// 已经是激活状态的不再发指令；失败只记日志，**绝不返回 error 打断上层**
    /// （实测 FM350 拒绝手动激活 IMS：`AT+CGACT=1,1` 直接 ERROR）。
    pub fn activate_ims(&self, ctxs: &[Context]) -> ImsActivateResult {
        let cids = ims_context_cids(ctxs);
        if cids.is_empty() {
            return ImsActivateResult {
                status: "skip".into(),
                note: "模组没上报 IMS 上下文（跳过）".into(),
            };
        }
        let mut done: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        for cid in cids {
            if let Some(c) = ctxs.iter().find(|c| c.cid == cid) {
                if c.active {
                    skipped.push(cid.to_string());
                    continue;
                }
            }
            match self.set_pdp(cid, true, false) {
                Ok(_) => done.push(cid.to_string()),
                Err(e) => failed.push(format!("cid={cid}（{e}）")),
            }
        }
        let (status, note) = if !failed.is_empty() && done.is_empty() && skipped.is_empty() {
            (
                "fail".to_string(),
                format!(
                    "IMS 激活失败：{}（有些固件/卡不许手动激活 IMS，不影响数据上网）",
                    failed.join("；")
                ),
            )
        } else if !failed.is_empty() {
            (
                "ok".to_string(),
                format!(
                    "IMS 部分成功（已激活 cid={}；失败：{}）",
                    done.join(","),
                    failed.join("；")
                ),
            )
        } else if !done.is_empty() {
            ("ok".to_string(), format!("已激活 cid={}", done.join(",")))
        } else {
            (
                "ok".to_string(),
                format!("已激活（cid={}）", skipped.join(",")),
            )
        };
        ImsActivateResult { status, note }
    }
}

// ---------- Modem：USB 类型 / 拨号状态机 ----------

/// USB 类型识别缓存时长。
const DIAL_TYPE_TTL: Duration = Duration::from_secs(30);

impl Modem {
    /// 识别模组当前的 USB 类型（页面「模块 USB 类型」卡片）。
    ///
    /// 三条路都会走一遍（都只是本地读取/一条 AT，很便宜），各补各的信息：
    /// ① 问固件 `AT+QCFG="usbnet"`；② 读 USB 描述符 → VID:PID、接口类码；
    /// ③ 读主机侧网卡的驱动。
    /// 30 秒内重复调用走缓存（`force=true` 强制重认）。
    pub fn usb_type(&self, force: bool) -> DialTypeInfo {
        if !force {
            let cache = self.dial_type.lock().unwrap();
            if let Some((info, at)) = cache.as_ref() {
                if at.elapsed() < DIAL_TYPE_TTL {
                    return info.clone();
                }
            }
        }
        let mut info = DialTypeInfo {
            modem_id: self.id,
            kind: "unknown".into(),
            source: "none".into(),
            probed_at: rfc3339(unix_now()),
            ..DialTypeInfo::default()
        };

        // ① 固件自己说（移远系：0=QMI/NDIS、1=ECM、2=MBIM、3=RNDIS、5=NCM）
        let st = self.usbnet();
        if st.supported && !st.value.is_empty() {
            info.value = st.value.clone();
            info.source = "at".into();
            info.confident = true;
            info.name = usbnet_mode_name(&info.value).to_string();
            info.query_cmd = "AT+QCFG=\"usbnet\"".into();
        }

        // ② USB 描述符：VID:PID / 产品名 / 接口类码（AT 成功时也读，页面那两格要显示）
        info.descriptor = usb_descriptor_for_port(&self.port());

        // ③ 主机侧网卡的驱动（本地 sysfs，不发 AT）
        //
        // 优先看**这个模组 USB 设备下的网卡**（多模组时不会拿别人的卡来判），
        // 找不到再退回全局扫描。
        info.host_iface = iface_hint_for_port(&self.port()).or_else(host_iface_hint);

        // 取值顺序：AT（最权威）→ 主机侧驱动 → USB 描述符 → 机型兜底
        //
        // 为什么把「驱动」提到描述符之前：**驱动名比类码确定**。
        // 实测 FM350-GL 的接口是 `02/02/ff`(ACM) + `0a/00/00`(Data) + `ff/42/01`(私有 RNDIS)，
        // 没有任何标准 RNDIS 类码（`ef/04`），而主机侧 `rndis_host` 一眼就是 RNDIS。
        if info.value.is_empty() {
            if let Some(h) = info.host_iface.as_ref() {
                if !h.value.is_empty() {
                    info.value = h.value.clone();
                    info.source = "hostiface".into();
                    info.confident = true;
                    info.note = format!(
                        "按**主机侧网卡的驱动**判定：{} ← {}（{}）",
                        h.iface,
                        h.driver,
                        usbnet_mode_name(&h.value)
                    );
                }
            }
        }
        if info.value.is_empty() {
            if let Some(d) = info.descriptor.as_ref() {
                if let Some(v) = classify_usb_classes(&d.classes) {
                    info.value = v.to_string();
                    info.source = "descriptor".into();
                    info.confident = true;
                }
            }
        }
        // ④ 机型兜底：350 系列（FM350 / RW350）的 RNDIS 是**厂商私有接口**
        //    （`ff/42/01`），标准类码与驱动都拿不到时按 RNDIS 处理 —— 有实机依据。
        if info.value.is_empty() && super::modem::is_fibocom_350(&self.model().unwrap_or_default())
        {
            info.value = "3".into();
            info.source = "model".into();
            info.note = "350 系列（FM350 / RW350）的 RNDIS 走厂商私有接口（ff/42/01），\
依机型判定为 RNDIS"
                .into();
        }
        if info.value.is_empty() {
            if let Some(d) = info.descriptor.as_ref() {
                if has_vendor_interface(&d.classes) {
                    // 只有厂商自定义接口，没有标准数据类 → 按 QMI/NDIS 推断
                    info.value = "0".into();
                    info.source = "descriptor".into();
                    info.note = "USB 接口里没有 ECM/MBIM/NCM/RNDIS 数据类（只有厂商自定义接口），按 QMI/NDIS 推断".into();
                }
            }
        }

        // 协议名：先按本机取值表里的名字（跨厂商关键），再按 usbnet 取值，再看主机侧驱动
        info.kind = dial_kind_from_name(&info.name);
        if info.kind.is_empty() {
            info.kind = dial_kind_of(&info.value);
            if info.kind == "unknown" {
                if let Some(h) = info.host_iface.as_ref() {
                    info.kind = dial_kind_from_name(&h.driver);
                }
            }
        }
        if info.descriptor.is_some() || info.host_iface.is_some() {
            info.bus = "usb".into();
        }
        // 展示名：本机取值表的 Name 优先，否则用协议名（QMI/ECM/…）
        if info.name.is_empty() {
            info.name = dial_kind_label(&info.kind);
        }
        // 拨号方式：先按取值查（移远口径），跨厂商的按协议查
        info.dial = dial_type_by_value(&info.value).or_else(|| dial_type_by_kind(&info.kind));

        if info.value.is_empty() && info.bus != "pcie" {
            info.note =
                "未能识别 USB 类型：固件不支持 AT+QCFG=\"usbnet\"，也没有读到 USB 描述符".into();
            if let Some(h) = info.host_iface.as_ref() {
                if !h.driver.is_empty() {
                    info.note = format!(
                        "主机侧网卡 {} 由 {} 驱动（该驱动不能唯一确定协议），USB 类型仍未能识别",
                        h.iface, h.driver
                    );
                }
            }
        }
        if info.kind.is_empty() {
            info.kind = "unknown".into();
        }
        // ④ 支持清单：本型号在 USB / PCIe 两种形态下支持哪些数据协议（页面直接显示）
        if let Some(e) = crate::modem::catalog::lookup(&self.model().unwrap_or_default()) {
            info.catalog_model = e.model.clone();
            info.catalog_usb = e.usb.clone();
            info.catalog_pcie = e.pcie.clone();
        }
        *self.dial_type.lock().unwrap() = Some((info.clone(), Instant::now()));
        info
    }

    /// 当前拨号状态（对外给状态时顺手算一下「已连接多久」）。
    ///
    /// 另外补两件「不发 AT 的本地事实」：
    /// - `host_iface` 为空时按**这个模组 USB 设备下的网卡**填上（idle 也有值）；
    /// - `host_ips` 为空时从同一张网卡的 sysfs 快照里取地址。
    ///
    /// 为什么要在 idle 时补：自动组网（`auto_wan`）早就把 `modem<id>` 建起来、
    /// 地址也拿到了，但状态机仍是 `idle`（没手动拨过号）—— 早先这两格只在
    /// `connected` 时才填，页面上「主机侧网卡」永远是 `-`，看着像没读到数据。
    pub fn dial_status(&self) -> DialState {
        let mut st = self.dial_state.lock().unwrap().clone().normalized();
        st.modem_id = self.id;

        // ① 本程序没记录过数据网卡时，按**这个模组的 USB 设备**推一块出来。
        if st.host_iface.trim().is_empty() {
            let names = usb_netifs_for_port(&self.port());
            // QMAP：地址在 mux 口（`wwan0_1`）上，优先它
            let pick = names
                .iter()
                .find(|n| crate::netprov::provision::qmap_mode(n) == 1)
                .or_else(|| names.first());
            if let Some(n) = pick {
                st.host_iface = n.clone();
            }
        }

        // ② 补地址。只查**这一块**网卡（带 5 秒缓存），
        //    不再遍历 `dial_iface_snapshot()` 的所有候选 —— 那会 N 次 fork。
        if st.host_ips.is_empty() && !st.host_iface.is_empty() {
            st.host_ips = iface_addrs_cached(&st.host_iface);
        }

        // ③ 状态机没结论（`idle` = 本程序从没下发过拨号）时，
        //    用**实际链路事实**给出结论。
        //
        //    这是用户报的「拨号链路状态读不到」的根因：这台机器上 `eth2`
        //    是 netifd 按 UCI（`modem0` proto=static）拉起来的，地址 10.42.0.16
        //    真实可用、默认路由也在走它 —— 但本程序没拨过号，状态机停在 idle，
        //    页面就显示「未拨号」。
        //
        //    判据刻意**不看** `operstate == up`：接口 UP 只说明链路层在，
        //    不代表拿到了地址（DHCP 失败、静态配置写错都会 UP 但没 IP）。
        if st.state == DIAL_IDLE {
            st.state = if st.host_iface.is_empty() {
                // 连该看哪块网卡都不知道（模组没插好 / 驱动没绑）
                DIAL_UNKNOWN.to_string()
            } else if st.host_ips.is_empty() {
                // 网卡在、但没拿到地址 → 链路没起来
                DIAL_DISCONNECTED.to_string()
            } else {
                // 有地址 ⇒ 链路真的通了。标注「不是本程序拨的」，排障时一眼能分清
                st.link_source = "netifd".to_string();
                DIAL_CONNECTED.to_string()
            };
            st.state_text = dial_state_text(&st.state);
        } else if st.link_source.is_empty() && !st.host_ips.is_empty() {
            // 本程序拨的（状态机已经给出结论），且有地址
            st.link_source = "modemd".to_string();
        }

        // ④ 主机侧地址拆成 v4 / v6。
        //
        //    页面「IPv4 / IPv6 本机地址」读的是 `ipv4` / `ipv6` 两个字段，
        //    而这条链路是 netifd 拉起来的、`dial_state` 里这两个字段没人填
        //    （它们原本只在本程序自己拨号时写入）—— 于是页面明明有地址却显示
        //    「未分配」。`host_ips` 里是网卡上的真实地址（含 /前缀长度），
        //    在这里拆开填进去，前端一行不用改。
        if st.host_ips.iter().any(|a| !a.trim().is_empty()) {
            let take = |pred: &dyn Fn(&str) -> bool, cur: &mut String| {
                if !cur.is_empty() {
                    return;
                }
                if let Some(a) = st.host_ips.iter().find(|a| pred(a)) {
                    *cur = a.split('/').next().unwrap_or(a).trim().to_string();
                }
            };
            take(&|a| !a.contains(':'), &mut st.ipv4);
            take(&|a| a.contains(':'), &mut st.ipv6);
        }

        // ⑤ 在线时长：**跟随 netifd 接口时间**（`ifstatus modem<id>` 的 `uptime`）。
        //
        //    这是用户报「连接时间显示不正常」的根因：早先时长有两个来源，都与
        //    「接口时间」对不上 ——
        //      · 模组会话 `connected_at`：重拨 / 接口重建后，会话起点和接口真实
        //        在线时间不是一回事（接口可能比拨号后的会话更早或更晚起来）；
        //      · 内存观测起点 `link_since`：modemd 一重启就归零，只是「本程序
        //        观察到它在线至今」，不是接口在线时长。
        //    netifd 的接口 uptime 才是权威的「这条链路起来了多久」，且跨 modemd
        //    重启保持正确。拿不到（接口非本程序所建 / ifstatus 失败）才回退。
        if st.state == DIAL_CONNECTED {
            let iface = crate::netprov::iface_name(self.id);
            if let Some(up) = netifd_iface_uptime_cached(&iface) {
                st.elapsed_sec = up as i64;
            } else if st.connected_at.is_empty() {
                // 没有 netifd 接口可查（用户手工搭的链路）⇒ 退回观测起点
                let mut g = self.link_since.lock().unwrap_or_else(|e| e.into_inner());
                let since = *g.get_or_insert_with(Instant::now);
                st.elapsed_sec = since.elapsed().as_secs() as i64;
            }
            // else：connected_at 有值但取不到接口 uptime ⇒ 保留 normalized() 已算的会话时长
        } else {
            // 断开就清基线：下次连上重新起算，避免把断线期间也算进去
            *self.link_since.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }

        // ⑥ 驱动/通信类型（RNDIS / QMI / MBIM…）。
        //
        //    同样只有「本程序自己拨号」时才填过，netifd 拉起的链路是空的。
        //    走 `usb_type(false)`：命中 30 秒缓存就立即返回，未命中才探测一次
        //    （探测内部用的 `usbnet()` 也有 30 秒缓存，不会每次轮询都发 AT）。
        if st.kind.is_empty() {
            let t = self.usb_type(false);
            if t.kind != "unknown" && !t.kind.is_empty() {
                st.type_name = if t.name.is_empty() {
                    t.kind.to_ascii_uppercase()
                } else {
                    t.name.clone()
                };
                st.kind = t.kind.clone();
            } else {
                // 明确说「识别不出来」，比让页面显示 `-` 有用
                st.type_name = "未能识别（设备未上报）".to_string();
            }
        }

        // ⑦ 链路阶段：`state` 之外再给一层「网络真的通了吗」。
        //
        //    只在连接态算（其它态没有意义）；默认路由读 /proc/net/route，
        //    不 fork —— dial_status 是所有出口的必经之路（HTTP/WS/snapshot），
        //    每次都 fork 一次 `ip route` 不可接受。
        st.phase = match st.state.as_str() {
            s if s == DIAL_CONNECTED => {
                // 逐族检查：v4 与 v6 的路由表独立，双栈下「v4 路由没了、v6 还在」
                // 是实测出现过的（eth2 删掉 v4 默认路由后其 v6 默认路由不受影响）。
                // 混在一起判会把「v4 已经出不去」掩盖成 online。
                let iface = st.host_iface.trim();
                let has_v4 = st.host_ips.iter().any(|a| !a.contains(':'));
                let has_v6 = st.host_ips.iter().any(|a| a.contains(':'));
                let route_ok = (!has_v4
                    || crate::netprov::route::has_default_via_family(iface, 4))
                    && (!has_v6 || crate::netprov::route::has_default_via_family(iface, 6));
                phase_for_connected(!iface.is_empty(), !st.host_ips.is_empty(), route_ok)
                    .to_string()
            }
            s if s == DIAL_DIALING => PHASE_DIALING.to_string(),
            s if s == DIAL_FAILED => PHASE_FAILED.to_string(),
            _ => PHASE_IDLE.to_string(),
        };
        st.phase_text = dial_phase_text(&st.phase);
        st
    }

    /// 模组上报的本机 IPv4（**不发 AT**：拨号状态 → 网络查询缓存）。
    pub fn dial_state_ipv4(&self) -> String {
        let st = self.dial_state.lock().unwrap().ipv4.trim().to_string();
        if !st.is_empty() {
            return st;
        }
        self.own_ipv4_cache
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
    }

    /// 补一条 `AT+CGPADDR` 读本机 IPv4（**会占串口**，只在状态里没有值时才该调用）。
    ///
    /// cid 用拨号状态里那条，没拨过号就退回 [`DEFAULT_DATA_CID`]。
    pub fn query_own_ipv4(&self) -> Result<String> {
        let cid = self.dial_state.lock().unwrap().cid;
        let cid = if cid > 0 { cid } else { DEFAULT_DATA_CID };
        let (ipv4, _) = self.query_pdp_addrs(cid)?;
        if !ipv4.trim().is_empty() {
            *self.own_ipv4_cache.lock().unwrap() = Some(ipv4.trim().to_string());
        }
        Ok(ipv4)
    }

    /// 模组上报的本机 IPv6（**不发 AT**：拨号状态 → 网络查询缓存）。
    ///
    /// 注意：`AT+CGPADDR` 的 v6 位在只拿到 IPv4 时也会回一个「只有接口标识、
    /// 前缀全零」的残值（实机 `::18df:9a01:f982:815a`）。这里**不**做过滤：
    /// 调用方（`netprov::v6::usable_static_v6`）才知道那份残值能不能用。
    pub fn dial_state_ipv6(&self) -> String {
        let st = self.dial_state.lock().unwrap().ipv6.trim().to_string();
        if !st.is_empty() {
            return st;
        }
        self.own_ipv6_cache
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
    }

    /// 补一条 `AT+CGPADDR` 读本机 IPv6（**会占串口**，只在状态里没有值时才该调用）。
    pub fn query_own_ipv6(&self) -> Result<String> {
        let cid = self.dial_state.lock().unwrap().cid;
        let cid = if cid > 0 { cid } else { DEFAULT_DATA_CID };
        let (_, ipv6) = self.query_pdp_addrs(cid)?;
        if !ipv6.trim().is_empty() {
            *self.own_ipv6_cache.lock().unwrap() = Some(ipv6.trim().to_string());
        }
        Ok(ipv6)
    }

    /// 用一次网络查询的结果校准拨号状态（**不额外发 AT**）。
    ///
    /// 拨号卡片要显示的「在用 APN / 本机地址 / 主机网卡」与「网络」卡片本来就是同一批数据，
    /// 周期推送里那段查询顺手同步过来就行，页面不用为了这几个字段再占一次串口。
    /// 只在已拨号（拨号中/已连接）时同步；没拨过号就不去动状态。
    pub fn sync_dial_state(&self, st: &super::network::NetworkStatus) {
        // 模组上报的本机地址与"我们有没有拨过号"无关：设备可能是在开机前就由
        // 别的路径（厂商脚本 / 上次会话）激活了 PDP。组网（`netprov`）正是靠这份
        // 地址给 350 系列配 static 接口 —— 早先整段被 `state != CONNECTED` 挡住，
        // 结果 idle 时永远拿不到地址，接口只能干等 DHCP（实测 FM350 不给）。
        if let Some(v4) = st.ipv4.as_deref() {
            if !v4.is_empty() {
                *self.own_ipv4_cache.lock().unwrap() = Some(v4.to_string());
            }
        }
        if let Some(v6) = st.ipv6.as_deref() {
            if !v6.is_empty() {
                *self.own_ipv6_cache.lock().unwrap() = Some(v6.to_string());
            }
        }
        self.dial_update(|s| {
            if let Some(apn) = st.apn.as_deref() {
                if !apn.is_empty() && apn != s.apn {
                    s.apn = apn.to_string();
                }
            }
            if let Some(v4) = st.ipv4.as_deref() {
                if !v4.is_empty() && v4 != s.ipv4 {
                    s.ipv4 = v4.to_string();
                }
            }
            if let Some(v6) = st.ipv6.as_deref() {
                if !v6.is_empty() && v6 != s.ipv6 {
                    s.ipv6 = v6.to_string();
                }
            }
            // 主机侧网卡只做「归属校验」：记录里的网卡必须是**这个模组 USB
            // 设备下的**，不是就换成 USB 枚举里的（优先 QMAP mux 口）。
            //
            //    早先是 `pick_dial_iface(猜名快照)`—— 实机把 SoC 有线口 eth1
            //    覆盖进了 host_iface，页面带着主路由的地址冒充模组链路。
            //    另外这里也顺带把「空的就补上」覆盖了（主机侧拨号器起来后
            //    网卡才出现；idle 时不进来，避免板载口混入）。
            if s.state != DIAL_CONNECTED && s.state != DIAL_DIALING {
                return;
            }
            let names = usb_netifs_for_port(&self.port());
            if names.is_empty() {
                return; // 模组 USB 下没有任何网卡：维持现状，不猜
            }
            let belongs = names.iter().any(|n| *n == s.host_iface);
            if !belongs || s.host_iface.trim().is_empty() {
                let pick = names
                    .iter()
                    .find(|n| crate::netprov::provision::qmap_mode(n) == 1)
                    .unwrap_or(&names[0]);
                s.host_iface = pick.clone();
                s.host_ips = iface_addrs_cached(pick);
            } else if s.host_ips.is_empty() {
                s.host_ips = iface_addrs_cached(&s.host_iface);
            }
        });
    }

    /// 拨号状态里记着的 USB 类型（不发 AT，只读缓存）。没拨过号就是空串。
    ///
    /// `pub(crate)`：拨号器托管靠它判断「这个模组要不要跑拨号器」，
    /// 而且**不能**为了这个去发 AT —— 组网循环的边界是「不额外发 AT」。
    pub(crate) fn cached_dial_kind(&self) -> String {
        self.dial_state.lock().unwrap().kind.trim().to_string()
    }

    /// 在锁内改状态。
    fn dial_update(&self, f: impl FnOnce(&mut DialState)) {
        let mut st = self.dial_state.lock().unwrap();
        f(&mut st);
    }

    /// 追加一步过程记录（页面按顺序显示，失败那一步一眼能看到）。
    fn dial_step(&self, name: &str, ok: bool, info: &str) {
        self.dial_update(|s| {
            s.steps.push(DialStep {
                name: name.to_string(),
                ok,
                info: info.to_string(),
                at: rfc3339(unix_now()),
            });
        });
    }

    /// 记下失败原因、把状态置成 failed。
    fn dial_fail(&self, err: &str) -> DialState {
        self.dial_update(|s| {
            s.state = DIAL_FAILED.to_string();
            s.state_text = dial_state_text(DIAL_FAILED);
            s.last_error = err.to_string();
        });
        self.dial_status()
    }

    /// 最近一次**从模组读出来**的 APN（拨号时记下的）。内部专用 APN 不算。
    ///
    /// `pub(crate)`：拨号器托管要用它给 MBIM 补 `-s`（没 APN 拨号器直接退出）。
    pub(crate) fn last_dial_apn(&self) -> String {
        let apn = self.dial_state.lock().unwrap().apn.trim().to_string();
        if apn.is_empty() || is_internal_apn(&apn) {
            return String::new();
        }
        apn
    }

    /// 读指定 cid 的本机地址（`AT+CGPADDR`）。
    ///
    /// 两条候选：不带 cid 的标准写法先试；被拒（实测 FM350-GL 的 `AT+CGPADDR`
    /// 直接回 ERROR，而 `AT+CGPADDR=2` 正常）再试带 cid 的写法。
    fn query_pdp_addrs(&self, cid: i32) -> Result<(String, String)> {
        let mut resp = self.at_cmd_prio(Prio::Data, "AT+CGPADDR");
        if !is_ok(&resp) {
            resp = self.at_cmd_prio(Prio::Data, &format!("AT+CGPADDR={cid}"));
            if !is_ok(&resp) {
                return Err(msg(format!(
                    "模组拒绝 AT+CGPADDR（{}）",
                    resp.trim().lines().next().unwrap_or("")
                )));
            }
        }
        let (ipv4, ipv6) = parse_cgpaddr(&resp, cid);
        if ipv4.is_empty() && ipv6.is_empty() {
            return Err(msg(
                "模组未上报本机地址（链路可能还在协商，稍后点「刷新状态」再看）",
            ));
        }
        Ok((ipv4, ipv6))
    }

    /// 拨号。**按 USB 类型只走一种方式**：
    ///
    ///   QMI / MBIM → 数据呼叫归主机侧厂商拨号器（modemd 托管，见 `runtime/dialer/`）：
    ///                 这里只把 APN 落到状态里，由下一轮组网循环（≤15s）拉起拨号器；
    ///                 **不发** `AT+CGACT=1`（会把拨号器的唯一数据承载占掉）；
    ///   其它        → 写 APN（可选）→ 激活 PDP → 回读确认 → 读本机地址 → 找主机侧网卡。
    ///
    /// 同时只允许一个拨号流程在跑（拨号要连发几条 AT，两次并发会互相干扰串口应答）。
    /// 失败也把状态带回去（页面显示「卡在哪一步」），错误信息走返回值。
    pub fn dial(
        &self,
        req: &DialRequest,
        dial_enabled: bool,
        activate_ims: bool,
    ) -> Result<DialState> {
        // 拨号总开关（页面「拨号上网 → 拨号开关」）关掉时，手动拨号也一律拒
        if !dial_enabled {
            return Err(msg(self
                .dial_fail("拨号总开关已关闭：打开「拨号上网 → 拨号开关」后才会下发拨号指令")
                .last_error));
        }
        if self
            .dial_busy
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(msg("上一次拨号还在进行中，请稍候再试"));
        }
        let result = self.dial_inner(req, activate_ims);
        self.dial_busy
            .store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    /// 拨号主流程（busy 锁由 [`Self::dial`] 持有）。
    fn dial_inner(&self, req: &DialRequest, activate_ims: bool) -> Result<DialState> {
        let mut cid = req.cid;
        if cid <= 0 {
            cid = DEFAULT_DATA_CID;
        }
        if !valid_cid(cid) {
            return Err(msg(self
                .dial_fail(&format!("cid 必须在 0~42 之间，当前 {cid}"))
                .last_error));
        }
        let apn = req.apn.trim();
        let pdp_type = req.pdp_type.trim().to_uppercase();
        if !apn.is_empty() && !valid_apn(apn) {
            return Err(msg(self
                .dial_fail(&format!("APN 只能包含字母、数字与 . - _（当前 {apn:?}）"))
                .last_error));
        }
        if !pdp_type.is_empty() && !valid_pdp_type(&pdp_type) {
            return Err(msg(self
                .dial_fail(&format!(
                    "PDP 类型只支持 IP / IPV6 / IPV4V6（当前 {pdp_type:?}）"
                ))
                .last_error));
        }

        // 先认总线 + 类型：它决定「主机侧谁来拨」，也写进状态里给页面看
        let info = self.usb_type(req.force);
        let host_setup = info
            .dial
            .as_ref()
            .map(|d| d.host_setup.clone())
            .unwrap_or_default();
        self.dial_update(|s| {
            s.state = DIAL_DIALING.to_string();
            s.state_text = dial_state_text(DIAL_DIALING);
            s.cid = cid;
            s.apn = apn.to_string();
            s.pdp_type = pdp_type.clone();
            s.kind = info.kind.clone();
            s.type_name = info.name.clone();
            s.host_setup = host_setup.clone();
            s.ipv4 = String::new();
            s.ipv6 = String::new();
            s.host_iface = String::new();
            s.host_ips = Vec::new();
            s.connected_at = String::new();
            s.last_error = String::new();
            s.steps = Vec::new();
            s.manual_stop = false; // 手动/自动拨号都会走到这里：用户以前的「断开」到此作废
        });

        // ① 写入 APN（可选：留空表示沿用模组里已有的配置）
        if !apn.is_empty() {
            let pt = if pdp_type.is_empty() {
                "IPV4V6"
            } else {
                &pdp_type
            };
            match self.set_apn(cid, pt, apn, false) {
                Ok(_) => self.dial_step(
                    "写入 APN（AT+CGDCONT）",
                    true,
                    &format!("cid={cid} {pt} {apn}"),
                ),
                Err(e) => {
                    // 不中断：上下文里可能已有可用 APN，先激活看看；失败原因留在步骤里
                    self.dial_step("写入 APN（AT+CGDCONT）", false, &e.to_string());
                }
            }
        } else {
            self.dial_step("写入 APN（AT+CGDCONT）", true, "沿用模组已有配置");
        }

        // ② 按 USB 类型分派 —— 一台模组只走**一种**拨号方式。
        //
        // QMI/MBIM：数据呼叫归主机侧厂商拨号器（现在由 modemd 托管，见
        // `runtime/dialer`）。这里绝不能发 AT+CGACT=1：模组只有一个数据承载，
        // 模组侧先激活会把拨号器的请求顶掉（实测 RM502Q-GL：requestSetupDataCall
        // 被拒 QMUXError 0xe、wwan0 link_state 0）。
        //
        // 那这一步做什么？把**用户刚填的 APN** 落到状态里（上面 dial_update 已经
        // 做了），让下一轮组网循环的拨号器托管按它重新起拨号器（`-s <apn>`）。
        // 进程起来到拿到地址要几秒，所以这一步报的是「已交办」而不是「已连上」——
        // 页面随后由 dial 推送刷新。
        if wan_host_dial_by_daemon(&info.kind) {
            // MBIM 没 APN 拨号器会直接退出：先拦，别记一个「已交办」再报错
            // （那样 steps 里留着一条 ok=true 的「交办」，跟 failed 状态自相矛盾）
            if info.kind.eq_ignore_ascii_case("mbim") && apn.is_empty() {
                let last = self.last_dial_apn();
                if last.is_empty() {
                    return Err(msg(self
                        .dial_fail(
                            "MBIM 模式必须填 APN（拨号器没有 -s 会直接退出）。\
                             请在「拨号上网」里填上运营商给的 APN 再拨号。",
                        )
                        .last_error));
                }
            }
            self.dial_step(
                "交办给主机侧拨号器（托管中）",
                true,
                &format!(
                    "{} 类型：数据呼叫由 modemd 内置拨号器建立；APN {}；下一轮组网循环（15s）内拉起",
                    dial_kind_label(&info.kind),
                    if apn.is_empty() {
                        "沿用模组配置".to_string()
                    } else {
                        format!("= {apn}")
                    }
                ),
            );
            let st = self.dial_status();
            log::info!(
                "模组 id={}（{}）：数据呼叫交办给主机侧拨号器，APN={}",
                self.id,
                info.kind,
                if apn.is_empty() {
                    "（沿用模组配置）"
                } else {
                    apn
                }
            );
            return Ok(st);
        }

        // ② 激活 PDP（cid 校验 / 先断后连 / 回读确认都在 set_pdp 里）
        let mut st_err = self.set_pdp(cid, true, req.reconnect).err();
        let mut st = self.params_refresh();
        if st_err.is_some() && apn.is_empty() {
            // ⚠️ 实测 FM350-GL：AT+CGACT=0,<cid> 之后该 cid 会从 AT+CGDCONT? 里消失，
            // 于是「断开 → 拨号（APN 留空）」会报错。这时拿**上次用过（本来就从模组读出来）
            // 的 APN** 补写一次配置再激活，让「断开再拨」能正常来回。
            let last = self.last_dial_apn();
            if !last.is_empty() {
                let pt = if pdp_type.is_empty() {
                    "IPV4V6"
                } else {
                    &pdp_type
                };
                if self.set_apn(cid, pt, &last, false).is_ok() {
                    self.dial_step(
                        "补写 APN（该 cid 停用后会从配置列表里消失）",
                        true,
                        &format!("cid={cid} {pt} {last}"),
                    );
                    st_err = self.set_pdp(cid, true, req.reconnect).err();
                    st = self.params_refresh();
                }
            }
        }
        if let Some(e) = st_err {
            self.dial_step("激活 PDP（AT+CGACT）", false, &e.to_string());
            // 失败时补一次前置体检：把「SIM 未就绪 / 未在线 / 未注册」写进错误，
            // 否则界面上只看到一句「激活失败」，排障时完全无从下手。
            let rd = self.dial_readiness();
            let why = rd.reason();
            let detail = if why.is_empty() {
                e.to_string()
            } else {
                format!("{e}（{why}）")
            };
            self.dial_step("拨号前置体检", false, &detail);
            return Err(msg(self.dial_fail(&detail).last_error));
        }
        let mut apn_in_use = apn.to_string();
        let mut type_in_use = pdp_type.clone();
        if let Some(c) = st.contexts.iter().find(|c| c.cid == cid) {
            if !c.apn.is_empty() {
                apn_in_use = c.apn.clone();
            }
            if !c.pdp_type.is_empty() {
                type_in_use = c.pdp_type.clone();
            }
        }
        self.dial_step("激活 PDP（AT+CGACT）", true, &format!("cid={cid} 已激活"));
        self.dial_update(|s| {
            s.apn = apn_in_use;
            s.pdp_type = type_in_use;
        });

        // ②b 顺手把 IMS 上下文也激活（VoLTE / IMS 短信的前提）。
        // ⚠️ 失败只记一条步骤：有些固件就是不许手动激活 IMS，绝不能因此把拨号判成失败。
        if activate_ims {
            let res = self.activate_ims(&st.contexts);
            self.dial_step(
                "激活 IMS 上下文（AT+CGACT）",
                res.status != "fail",
                &res.note,
            );
        }

        // ③ 读本机地址（读不到不算致命：分配可能还在协商，页面可稍后刷新）
        match self.query_pdp_addrs(cid) {
            Ok((ipv4, ipv6)) => {
                self.dial_step(
                    "读取本机地址（AT+CGPADDR）",
                    true,
                    format!("{ipv4} {ipv6}").trim(),
                );
                self.dial_update(|s| {
                    s.ipv4 = ipv4;
                    s.ipv6 = ipv6;
                });
            }
            Err(e) => self.dial_step("读取本机地址（AT+CGPADDR）", false, &e.to_string()),
        }

        // ④ 找主机侧网卡：**只认这个模组 USB 设备下的网卡**，绝不按名字猜。
        //
        //    早先走 `pick_dial_iface(&dial_iface_snapshot())` 的名字匹配，
        //    实机翻车：RNDIS 模组的数据口是 eth2（挂在模组 USB 接口上），
        //    而名字匹配把 SoC 有线口 eth1 认了进去 —— 页面「主机网卡接口 /
        //    已分配 IPv4」显示的是主路由 LAN 的 eth1 + 192.168.88.x，
        //    与模组毫无关系。数据口一定挂在模组自己的 USB 接口上，
        //    `usb_netifs_for_port` 按 sysfs 归属枚举，天然排除了板载口。
        let usb_names = usb_netifs_for_port(&self.port());
        let usb_pick = usb_names
            .iter()
            .find(|n| crate::netprov::provision::qmap_mode(n) == 1)
            .or_else(|| usb_names.first());
        if let Some(n) = usb_pick {
            let mut note = n.clone();
            // 少数机型（FM350 系列）模组侧 DHCP 很慢/不稳：可能要手工配一次网卡 IP
            if let Some(quirk) = host_nic_quirk(self.model_raw().unwrap_or_default().as_str()) {
                note += &format!("；⚠️ {quirk}");
            }
            let addrs = iface_addrs_cached(n);
            self.dial_step("主机侧网卡（模组 USB 设备下）", true, &note);
            self.dial_update(|s| {
                s.host_iface = n.clone();
                s.host_ips = addrs;
            });
        } else {
            self.dial_step(
                "主机侧网卡候选",
                false,
                "本机没找到像模组数据口的网卡（可能主机侧拨号器还没拉起来）",
            );
        }

        // ④b 链路校验：只凭「PDP 激活 + 模组给了地址」就宣布成功是骗人 ——
        //     主机网卡可能还没起来（FM350 实测：模组侧都好，eth2 挂 169.254、
        //     无默认路由）。这里把「网卡 / 路由」两层也查一遍，写进 steps；
        //     结论由 dial_status 的 phase 对外（state 契约不动）。
        let st_now = self.dial_status();
        let iface_up = !st_now.host_iface.trim().is_empty() && !st_now.host_ips.is_empty();
        if iface_up {
            let route_ok = crate::netprov::route::has_default_via(&st_now.host_iface);
            self.dial_step(
                "主机网卡就绪（有地址）",
                true,
                &format!("{} {}", st_now.host_iface, st_now.host_ips.join(" ")),
            );
            self.dial_step(
                "默认路由经该网口",
                route_ok,
                if route_ok {
                    ""
                } else {
                    "没有经它的默认路由：地址配了也出不去（查 netifd 配置 / metric）"
                },
            );
        } else {
            self.dial_step(
                "主机网卡就绪（有地址）",
                false,
                &if st_now.host_iface.trim().is_empty() {
                    "还没找到模组对应的数据网卡".to_string()
                } else {
                    format!("{} 还没拿到地址（DHCP 未回 / 静态未配）", st_now.host_iface)
                },
            );
        }

        self.dial_update(|s| {
            s.state = DIAL_CONNECTED.to_string();
            s.state_text = dial_state_text(DIAL_CONNECTED);
            s.connected_at = rfc3339(unix_now());
            s.last_error = String::new();
        });
        Ok(self.dial_status())
    }

    /// 断开数据连接。
    ///
    ///   QMI / MBIM → 拨号器由 handler 在调本函数**之前**就停掉（顺序要紧，
    ///                 见 `http/handlers/dial_disconnect.rs`），这里只重置状态；
    ///   其它        → 发 `AT+CGACT=0,<cid>` + 回读确认。
    ///
    /// cid <= 0 时用当前拨号状态里的 cid，再不行用数据默认 cid。
    /// ⚠️ 内部上下文（IMS / sos）有的固件不许手动停用，会回 ERROR —— 那时如实报错、不改状态。
    pub fn hangup(&self, cid: i32) -> Result<DialState> {
        let mut cid = cid;
        if cid <= 0 {
            cid = self.dial_state.lock().unwrap().cid;
        }
        if cid <= 0 {
            cid = DEFAULT_DATA_CID;
        }
        if !valid_cid(cid) {
            return Err(msg(format!("cid 必须在 0~42 之间，当前 {cid}")));
        }

        // QMI/MBIM：数据呼叫归主机侧拨号器，模组侧本来就没有我们激活的上下文
        if wan_host_dial_by_daemon(&self.cached_dial_kind()) {
            self.dial_update(|s| {
                s.state = DIAL_IDLE.to_string();
                s.state_text = dial_state_text(DIAL_IDLE);
                s.cid = cid;
                s.ipv4 = String::new();
                s.ipv6 = String::new();
                s.host_iface = String::new();
                s.host_ips = Vec::new();
                s.connected_at = String::new();
                s.last_error = String::new();
                s.manual_stop = true; // 尊重「人」的断开
                s.steps = vec![DialStep {
                    name: "断开数据连接（停止主机侧拨号器）".into(),
                    ok: true,
                    // 拨号器由 handler 在调 hangup 之前就停掉了（顺序要紧，
                    // 见 handlers/dial_disconnect.rs）；这里只说结果
                    info: "已停止 modemd 托管的拨号器（连它拉起的 udhcpc 一起），并置 manual_stop；自动组网不会再把它拉回来".into(),
                    at: rfc3339(unix_now()),
                }];
            });
            return Ok(self.dial_status());
        }

        let cmd = format!("AT+CGACT=0,{cid}");
        let resp = self.at_cmd_prio(Prio::Data, &cmd);
        if !is_ok(&resp) {
            return Err(msg(format!(
                "模组拒绝 {cmd}（内部上下文 IMS/sos 不允许手动停用；或该 cid 本来就没激活）"
            )));
        }
        // 回读确认（去激活通常很快，读不到也不影响「已经下发过」这件事，所以不当失败）
        let activated = self.at_cmd_prio(Prio::Data, "AT+CGACT?");
        // 三态判据：模组一条 `+CGACT:` 行都不回时（实测广和通 MTK 平台如此），
        // 旧写法会把「没列明细」误读成「已确认去激活」， diagnostically 是两件事。
        let mut note = String::new();
        match cgact_state(&activated, cid) {
            Some(true) => note = "回读仍显示激活（可能固件延迟）".to_string(),
            Some(false) => {}
            None => note = "模组未回读激活明细，去激活结果以已下发为准".to_string(),
        }
        self.dial_update(|s| {
            s.state = DIAL_IDLE.to_string();
            s.state_text = dial_state_text(DIAL_IDLE);
            s.cid = cid;
            s.ipv4 = String::new();
            s.ipv6 = String::new();
            s.host_iface = String::new();
            s.host_ips = Vec::new();
            s.connected_at = String::new();
            s.last_error = String::new();
            s.manual_stop = true; // 是「人」断的：自动组网不要再替他把链路上拉
            s.steps = vec![DialStep {
                name: format!("断开数据连接（{cmd}）"),
                ok: true,
                info: note,
                at: rfc3339(unix_now()),
            }];
        });
        Ok(self.dial_status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 链路阶段 ----------

    #[test]
    fn phase_layers_follow_the_three_facts() {
        // ①②③ 全满足 → online
        assert_eq!(phase_for_connected(true, true, true), PHASE_ONLINE);
        // 模组侧都好、主机网卡也有地址，但默认路由走别的口 → route_pending
        assert_eq!(phase_for_connected(true, true, false), PHASE_ROUTE);
        // 网卡不在 / 没地址 → iface_pending（路由根本无从谈起）
        assert_eq!(phase_for_connected(true, false, false), PHASE_IFACE);
        assert_eq!(phase_for_connected(false, false, false), PHASE_IFACE);
        // 网卡不在但「有地址」在逻辑上不可能，这里只保证不 panic、给最保守值
        assert_eq!(phase_for_connected(false, true, true), PHASE_IFACE);
    }

    #[test]
    fn phase_text_is_human_readable() {
        assert_eq!(dial_phase_text(PHASE_ONLINE), "网络就绪（地址 + 默认路由）");
        assert_eq!(dial_phase_text(PHASE_IFACE), "数据会话已建立，主机网卡未就绪");
        assert_eq!(dial_phase_text(PHASE_ROUTE), "网卡有地址，但默认路由不经它");
        // 未认识的值原样返回，不 panic
        assert_eq!(dial_phase_text("whatever"), "whatever");
    }

    // ---------- 拨号前置体检 ----------

    #[test]
    fn readiness_ok_when_all_three_pass() {
        let rd = DialReadiness {
            sim_ready: Some(true),
            cfun: Some(1),
            reg_state: Some(1),
            raw: Vec::new(),
        };
        assert!(rd.ok());
        assert_eq!(rd.reason(), "");
        // 漫游已注册（5）同样算就绪
        let roam = DialReadiness {
            reg_state: Some(5),
            ..rd
        };
        assert!(roam.ok());
        assert_eq!(roam.reason(), "");
    }

    #[test]
    fn readiness_reports_every_missing_precondition() {
        // 三项全缺（None = 问不出来）：两条「读不到」原因都要出现
        let rd = DialReadiness::default();
        assert!(!rd.ok());
        let why = rd.reason();
        assert!(why.contains("读不到 SIM 卡状态"), "{why}");
        assert!(why.contains("读不到注册状态"), "{why}");

        // SIM 明确未就绪（Some(false)）
        let pin = DialReadiness {
            sim_ready: Some(false),
            ..DialReadiness::default()
        };
        assert!(pin.reason().contains("SIM 卡未就绪"), "{}", pin.reason());

        // 飞行模式单独拦
        let fm = DialReadiness {
            sim_ready: Some(true),
            cfun: Some(4),
            reg_state: Some(1),
            raw: Vec::new(),
        };
        assert!(!fm.ok());
        assert!(fm.reason().contains("飞行模式"), "{}", fm.reason());

        // 未注册（既不是 1 也不是 5）
        let nr = DialReadiness {
            sim_ready: Some(true),
            cfun: Some(1),
            reg_state: Some(0),
            raw: Vec::new(),
        };
        assert!(!nr.ok());
        assert!(nr.reason().contains("未注册"), "{}", nr.reason());
    }

    // ---------- 状态机与校验 ----------

    #[test]
    fn cid_range() {
        assert!(valid_cid(0));
        assert!(valid_cid(1));
        assert!(valid_cid(42));
        assert!(!valid_cid(43));
        assert!(!valid_cid(-1));
    }

    #[test]
    fn pdp_type_enum() {
        assert!(valid_pdp_type("IP"));
        assert!(valid_pdp_type("ipv6"));
        assert!(valid_pdp_type(" IPV4V6 "));
        assert!(!valid_pdp_type("PPP"));
        assert!(!valid_pdp_type(""));
    }

    #[test]
    fn state_text_mapping() {
        assert_eq!(dial_state_text(DIAL_IDLE), "未拨号");
        assert_eq!(dial_state_text(DIAL_DIALING), "拨号中");
        assert_eq!(dial_state_text(DIAL_CONNECTED), "已连接");
        // 用户报的「拨号链路状态读不到」修复时统一了措辞：
        // failed 从「拨号失败」改成更准确的「连接异常」（链路已建立后掉线也算它）
        assert_eq!(dial_state_text(DIAL_FAILED), "连接异常");
        // 新增的三个状态：必须能和「未拨号」区分开，否则界面上看不出是哪种毛病
        assert_eq!(dial_state_text(DIAL_DISCONNECTED), "未连接");
        assert_eq!(dial_state_text(DIAL_DISCONNECTING), "正在断开");
        assert_eq!(dial_state_text(DIAL_UNKNOWN), "状态未知");
        assert_eq!(dial_state_text("别的"), "未拨号");
    }

    #[test]
    fn parse_iface_uptime_reads_number_and_rejects_zero() {
        assert_eq!(parse_iface_uptime(r#"{"up":true,"uptime":1388}"#), Some(1388));
        assert_eq!(parse_iface_uptime(r#"{"up":true,"uptime":"42"}"#), Some(42));
        assert_eq!(parse_iface_uptime(r#"{"up":true,"uptime":0}"#), None);
        assert_eq!(parse_iface_uptime(r#"{"up":false}"#), None);
        assert_eq!(parse_iface_uptime("not json"), None);
    }

    #[test]
    fn iface_addr_parser_reads_inet_line() {
        // 解析 `ip -4 -o addr show` 的一行：别把 brd / scope 后面的东西当地址
        let line = "2: eth2    inet 10.42.0.16/24 brd 10.42.0.255 scope global eth2";
        let pos = line.find(" inet ").unwrap();
        let addr = line[pos + 6..].split_whitespace().next().unwrap();
        assert_eq!(addr, "10.42.0.16/24");
        // 没有 inet 的行（纯 link 信息）不该产出地址
        assert!(
            !"2: eth2    link/ether 00:00:11:12:13:14 brd ff:ff:ff:ff:ff:ff".contains(" inet ")
        );
    }

    #[test]
    fn zero_state_normalizes_to_idle() {
        let st = DialState::default().normalized();
        assert_eq!(st.state, DIAL_IDLE);
        assert_eq!(st.state_text, "未拨号");
    }

    // ---------- 类型表 ----------

    #[test]
    fn dial_types_table() {
        let t = dial_types();
        assert_eq!(t.len(), 5);
        assert_eq!(
            t.iter().map(|x| x.value.as_str()).collect::<Vec<_>>(),
            ["0", "1", "2", "3", "5"]
        );
        // 只有 MBIM 的模组侧 AT 拨号不够用
        assert!(!t.iter().find(|x| x.kind == "mbim").unwrap().at_dial);
        assert!(t.iter().filter(|x| x.at_dial).count() == 4);
    }

    #[test]
    fn kind_from_value_and_name() {
        assert_eq!(dial_kind_of("0"), "qmi");
        assert_eq!(dial_kind_of("5"), "ncm");
        assert_eq!(dial_kind_of("4"), "unknown");
        assert_eq!(dial_kind_of(""), "unknown");
        // ⚠️ rndis 必须先于 ndis 判定："RNDIS" 里就含 "ndis"
        assert_eq!(dial_kind_from_name("RNDIS"), "rndis");
        assert_eq!(dial_kind_from_name("QMI/NDIS"), "qmi");
        assert_eq!(dial_kind_from_name("mbim"), "mbim");
        assert_eq!(dial_kind_from_name("ECM"), "ecm");
        assert_eq!(dial_kind_from_name("NCM"), "ncm");
        assert_eq!(dial_kind_from_name("RMNET"), "");
        assert_eq!(dial_kind_from_name(""), "");
    }

    #[test]
    fn kind_lookup_and_label() {
        assert_eq!(dial_kind_label("qmi"), "QMI");
        assert_eq!(dial_kind_label("weird"), "weird");
        assert!(dial_type_by_value("1").is_some());
        assert!(dial_type_by_value("4").is_none());
        assert!(dial_type_by_kind("qmi").is_some());
        assert!(dial_type_by_kind("").is_none());
        assert!(dial_type_by_kind("unknown").is_none());
    }

    #[test]
    fn host_dial_dispatch() {
        assert!(wan_host_dial_by_daemon("qmi"));
        assert!(wan_host_dial_by_daemon("MBIM"));
        assert!(!wan_host_dial_by_daemon("ecm"));
        assert!(!wan_host_dial_by_daemon("ncm"));
        assert!(!wan_host_dial_by_daemon("rndis"));
        assert!(!wan_host_dial_by_daemon("unknown"));
    }

    #[test]
    fn usbnet_mode_names() {
        assert_eq!(usbnet_mode_name("0"), "QMI/NDIS");
        assert_eq!(usbnet_mode_name("1"), "ECM");
        assert_eq!(usbnet_mode_name("2"), "MBIM");
        assert_eq!(usbnet_mode_name("3"), "RNDIS");
        assert_eq!(usbnet_mode_name("5"), "NCM");
        assert_eq!(usbnet_mode_name("4"), "");
    }

    // ---------- 类码解析 ----------

    #[test]
    fn split_class_code_formats() {
        assert_eq!(
            split_usb_class_code("02/06/00"),
            ("02".into(), "06".into(), "00".into(), true)
        );
        assert_eq!(
            split_usb_class_code("02-0e-00"),
            ("02".into(), "0e".into(), "00".into(), true)
        );
        assert_eq!(
            split_usb_class_code("USB\\Class_02&SubClass_0e&Prot_00"),
            ("02".into(), "0e".into(), "00".into(), true)
        );
        // 只有类也能用（有些固件不填后两位）
        assert_eq!(
            split_usb_class_code("ff"),
            ("ff".into(), "".into(), "".into(), true)
        );
        // 一位十六进制补前导零
        assert_eq!(
            split_usb_class_code("2/6/0"),
            ("02".into(), "06".into(), "00".into(), true)
        );
        assert_eq!(
            split_usb_class_code("zz"),
            ("".into(), "".into(), "".into(), false)
        );
        assert_eq!(
            split_usb_class_code(""),
            ("".into(), "".into(), "".into(), false)
        );
    }

    #[test]
    fn classify_classes_priority() {
        // 控制口优先：02/0e → MBIM 即使同时存在 0a 数据口
        assert_eq!(
            classify_usb_classes(&["0a/00/00".into(), "02/0e/00".into()]),
            Some("2")
        );
        assert_eq!(classify_usb_classes(&["02/0d/00".into()]), Some("5"));
        assert_eq!(classify_usb_classes(&["ef/04/01".into()]), Some("3"));
        assert_eq!(
            classify_usb_classes(&["0a/00/00".into(), "02/06/00".into()]),
            Some("1")
        );
        // ⚠️ 单独一个 CDC Data（0a）**定不了类型**：ECM/NCM/MBIM 里都有它。
        // 实测 FM350-GL 就带 `0a/00/00`，老规则会把它判成 NCM（实际是 RNDIS）。
        assert_eq!(classify_usb_classes(&["0a/00/00".into()]), None);
        // FM350-GL 的真实接口组合：ACM + Data + 厂商私有 RNDIS
        // 没有标准数据类 ⇒ 描述符这一层认不出，交给主机侧驱动/机型兜底
        assert_eq!(
            classify_usb_classes(&[
                "02/02/ff".into(),
                "0a/00/00".into(),
                "ff/00/00".into(),
                "ff/42/01".into(),
            ]),
            None
        );
        // 只有厂商自定义接口 → 认不出
        assert_eq!(classify_usb_classes(&["ff/ff/ff".into()]), None);
    }

    #[test]
    fn rndis_driver_wins_over_cdc_data_class() {
        // 实机 FM350-GL：描述符只有 0a/00/00（无标准 RNDIS 类码），
        // 但主机侧驱动是 rndis_host ⇒ 判定依据应该落在驱动上，值必须是 3。
        assert_eq!(usbnet_from_host_driver("rndis_host"), "3");
        assert_eq!(usbnet_mode_name("3"), "RNDIS");
        // 常见驱动的映射不能变（ECM/NCM/QMI/MBIM）
        assert_eq!(usbnet_from_host_driver("cdc_ether"), "1");
        assert_eq!(usbnet_from_host_driver("cdc_ncm"), "5");
        assert_eq!(usbnet_from_host_driver("qmi_wwan"), "0");
        assert_eq!(usbnet_from_host_driver("cdc_mbim"), "2");
    }

    #[test]
    fn vendor_interface_detection() {
        assert!(has_vendor_interface(&[
            "02/02/01".into(),
            "ff/00/00".into()
        ]));
        assert!(!has_vendor_interface(&[
            "02/06/00".into(),
            "0a/00/00".into()
        ]));
    }

    #[test]
    fn class_summary_labels() {
        assert_eq!(class_summary("02/0d/00"), "02/0d/00 CDC-NCM 控制接口");
        assert_eq!(class_summary("02/06/00"), "02/06/00 CDC-ECM 控制接口");
        assert_eq!(
            class_summary("0a/00/00"),
            "0a/00/00 CDC 数据接口（ECM / NCM / MBIM 共用）"
        );
        assert_eq!(class_summary("ef/04/01"), "ef/04/01 RNDIS");
        // 认不出的原样回传，不猜
        assert_eq!(class_summary("99/99/99"), "99/99/99");
        let list = class_summary_list(&["02/06/00".into(), "02/06/00".into(), "0a/00/00".into()]);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn reduce_codes_drops_shorthand() {
        // "02" 是 "02/06" 的简写（同一类里别人更具体）→ 去掉
        let out = reduce_usb_codes(&["02".into(), "02/06".into(), "0a".into()]);
        assert_eq!(out, vec!["02/06".to_string(), "0a".to_string()]);
        // 去重 + 排序
        let out = reduce_usb_codes(&["0a/00/00".into(), "02/0e/00".into(), "0a/00/00".into()]);
        assert_eq!(out, vec!["02/0e/00".to_string(), "0a/00/00".to_string()]);
    }

    // ---------- 主机侧网卡匹配 ----------

    #[test]
    fn iface_matching() {
        assert_eq!(match_dial_iface("wwan0"), (true, true));
        assert_eq!(match_dial_iface("usb0"), (true, true));
        assert_eq!(match_dial_iface("rmnet_data0"), (true, true));
        assert_eq!(match_dial_iface("eth0"), (true, false));
        assert_eq!(match_dial_iface("以太网"), (true, false));
        assert_eq!(match_dial_iface("docker0"), (false, false));
        assert_eq!(match_dial_iface("br-lan"), (false, false));
        assert_eq!(match_dial_iface("wlan0"), (false, false));
        assert_eq!(match_dial_iface("phy0-ap0"), (false, false));
        assert_eq!(match_dial_iface(""), (false, false));
    }

    #[test]
    fn pick_iface_ranking() {
        let list = vec![
            IfaceInfo {
                name: "eth0".into(),
                up: true,
                strong: false,
                addr: vec![],
            },
            IfaceInfo {
                name: "wwan0".into(),
                up: false,
                strong: true,
                addr: vec![],
            },
            IfaceInfo {
                name: "usb0".into(),
                up: true,
                strong: true,
                addr: vec![],
            },
        ];
        let best = pick_dial_iface(&list).unwrap();
        assert_eq!(best.name, "usb0");
        assert!(pick_dial_iface(&[]).is_none());
    }

    #[test]
    fn host_driver_to_usbnet() {
        assert_eq!(usbnet_from_host_driver("qmi_wwan"), "0");
        assert_eq!(usbnet_from_host_driver("cdc_mbim"), "2");
        assert_eq!(usbnet_from_host_driver("cdc_ether"), "1");
        assert_eq!(usbnet_from_host_driver("rndis_host"), "3");
        assert_eq!(usbnet_from_host_driver("cdc_ncm"), "5");
        // 定制固件的后缀变体
        assert_eq!(usbnet_from_host_driver("qmi_wwan_q"), "0");
        assert_eq!(usbnet_from_host_driver("cdc_mbim_qcom"), "2");
        // 认不出就不猜
        assert_eq!(usbnet_from_host_driver("e1000e"), "");
        assert_eq!(usbnet_from_host_driver(""), "");
    }

    // ---------- CGPADDR 解析 ----------

    #[test]
    fn looks_like_ip_cases() {
        assert!(looks_like_ip("10.42.1.67"));
        assert!(looks_like_ip("255.255.255.255"));
        assert!(!looks_like_ip("0.0.0.0"), "全零 = 未分配");
        assert!(!looks_like_ip("10.0.0"), "段数不够");
        assert!(!looks_like_ip("10.0.0.256"), "超范围");
        assert!(!looks_like_ip("a.b.c.d"));
    }

    #[test]
    fn split_addrs_standard_two_fields() {
        // 标准写法：两个引号字段
        let (v4, v6) = split_addrs("\"10.42.1.67\",\"2001:db8:1:2::8\"");
        assert_eq!(v4, "10.42.1.67");
        assert_eq!(v6, "2001:db8:1:2::8");
    }

    #[test]
    fn split_addrs_sixteen_decimal_bytes() {
        // 移远某些固件的怪写法：16 个十进制字节 = IPv6
        // 36=0x24 14=0x0e 4=0x04 0=0x00 | 88=0x58 40=0x28 24=0x18 86=0x56 |
        // 29=0x1d 217=0xd9 193=0xc1 130=0x82 | 175=0xaf 200=0xc8 182=0xb6 58=0x3a
        let (v4, v6) = split_addrs("32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8");
        assert_eq!(v4, "");
        assert_eq!(v6, "2001:db8:1:2::8");
    }

    #[test]
    fn split_addrs_rejects_zero_prefix() {
        // FM350-GL：只报接口标识（前 8 字节全 0）→ 不显示不可信的地址
        let (v4, v6) =
            split_addrs("\"10.42.2.236\",\"0.0.0.0.0.0.0.0.24.217.217.24.12.29.50.199\"");
        assert_eq!(v4, "10.42.2.236");
        assert_eq!(v6, "");
    }

    #[test]
    fn split_addrs_ipv6_only() {
        let (v4, v6) = split_addrs("\"2001:db8:1:2::8\"");
        assert_eq!(v4, "");
        assert_eq!(v6, "2001:db8:1:2::8");
    }

    #[test]
    fn parse_cgpaddr_prefers_cid() {
        let resp = "+CGPADDR: 1,\"10.0.0.1\",\"2001:db8:1:2:3:4:5:6\"\r\n\
                    +CGPADDR: 2,\"10.0.0.2\"\r\nOK\r\n";
        assert_eq!(
            parse_cgpaddr(resp, 1),
            ("10.0.0.1".into(), "2001:db8:1:2:3:4:5:6".into())
        );
        assert_eq!(parse_cgpaddr(resp, 2), ("10.0.0.2".into(), "".into()));
        // 偏好 cid 不在列表里 → 取第一个非空
        assert_eq!(
            parse_cgpaddr(resp, 9),
            ("10.0.0.1".into(), "2001:db8:1:2:3:4:5:6".into())
        );
        // 什么都没报
        assert_eq!(
            parse_cgpaddr("+CGPADDR: 1,\"0.0.0.0\"\r\nOK\r\n", 1),
            ("".into(), "".into())
        );
        assert_eq!(parse_cgpaddr("OK\r\n", 1), ("".into(), "".into()));
    }

    #[test]
    fn split_addrs_rejects_malformed_colon_form() {
        // 结构不合格的冒号写法必须拦下，不能原样写进接口配置
        assert_eq!(split_addrs("\"gg::1\""), ("".into(), "".into()));
        assert_eq!(split_addrs("\"2001::8::1\""), ("".into(), "".into())); // 两个 ::
        assert_eq!(
            split_addrs("\"2001:1:2:3:4:5:6:7:8\""),
            ("".into(), "".into())
        ); // 9 组
           // 占位地址
        assert_eq!(split_addrs("\"::\""), ("".into(), "".into()));
        assert_eq!(split_addrs("\"::1\""), ("".into(), "".into()));
        // zone id 与方括号展示形态
        assert_eq!(
            split_addrs("\"fe80::1%eth2\""),
            ("".into(), "fe80::1".into())
        );
        assert_eq!(
            split_addrs("\"[2001:db8::8]\""),
            ("".into(), "2001:db8::8".into())
        );
        // 内嵌 IPv4 仍合法
        assert_eq!(
            split_addrs("\"::ffff:10.0.0.1\""),
            ("".into(), "::ffff:10.0.0.1".into())
        );
    }

    #[test]
    fn split_addrs_rejects_dotted_placeholder() {
        // 广和通 MTK 平台 IPv6 未分配时回 16 段点分的 ::1 占位
        assert_eq!(
            split_addrs("\"10.42.4.240\",\"0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1\""),
            ("10.42.4.240".into(), "".into())
        );
        // 实机真实地址（有前缀）照常解出
        assert_eq!(
            split_addrs("\"32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8\""),
            ("".into(), "2001:db8:1:2::8".into())
        );
    }

    // ---------- IMS 上下文 ----------

    #[test]
    fn ims_apn_detection() {
        assert!(ims_apn_name("ims"));
        assert!(ims_apn_name("IMS"));
        assert!(ims_apn_name("ims.apn"));
        assert!(ims_apn_name("ims-xxx"));
        assert!(!ims_apn_name("imsfoo"), "ims 后面紧跟字母数字的不算");
        assert!(!ims_apn_name("3gnet"));
        assert!(!ims_apn_name(""));
    }

    #[test]
    fn ims_context_selection() {
        let ctx = |cid: i32, apn: &str, internal: bool| Context {
            cid,
            pdp_type: "IP".into(),
            apn: apn.into(),
            active: false,
            internal,
        };
        // ① APN 就是 ims；② cid==1 且空 APN 且内部；别的空 APN 上下文不算
        let list = vec![
            ctx(1, "", true),
            ctx(2, "3GNET", false),
            ctx(3, "", false),
            ctx(5, "ims", false),
            ctx(5, "ims", false), // 去重
        ];
        assert_eq!(ims_context_cids(&list), vec![1, 5]);
        assert!(ims_context_cids(&[ctx(2, "3GNET", false), ctx(3, "", false)]).is_empty());
    }

    // ---------- 拨号请求 ----------

    #[test]
    fn dial_request_defaults() {
        let r = DialRequest::default();
        assert_eq!(r.cid, 0);
        assert!(r.apn.is_empty());
        assert!(!r.reconnect);
        assert!(!r.force);
    }
}
