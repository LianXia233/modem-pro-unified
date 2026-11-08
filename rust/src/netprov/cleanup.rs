//! 拔卡/掉线后清理残留接口（`modem<id>` / `modem<id>_6`）。
//!
//! ## 为什么不能照抄上游的无条件删除
//!
//! 上游 `module/kernel/netprov_sync.go:189-227`（`sweepStaleAutoWAN`）与
//! `:229-262`（`cleanupAutoWANForModem`）的做法是「名字是 `modem<N>` + 这台模组
//! 不在了 + 绑定的设备也不在了 ⇒ `uci -q delete network.<name>`」。两道保险都对，
//! 但它默认「名字归我们」就等于「配置归我们」—— 用户完全可能在 LuCI 里给这张
//! 接口加过 `mtu` / `ip6ifaceid` / 自己改过 `metric`，删掉就等于把用户的改动一起
//! 抹了，而且**删完无法恢复**（没有留档）。
//!
//! 本模块在上游两道保险之上再加三道：
//!
//! 1. **生成记录比对**（[`IfaceRecord`]）：只有「本程序最后一次写下的选项集合」
//!    与 UCI 现状**逐键一致**时才认作「本程序生成、用户没动过」；多出来的键、
//!    被改过的值、被删掉的键，一律判定为用户改动 ⇒ **保留并记日志**，交由用户
//!    自己在 LuCI 里决定。比对范围是[`MANAGED_KEYS`]（本程序会写的那些键），
//!    用户自己加的键不在记录里 ⇒ 一定表现为「多出来的键」。
//! 2. **删除前留档**（[`archive`]）：被删接口的完整选项先追加到
//!    `netprov_cleanup.json`，同时在日志里打一段**可直接粘贴执行的 `uci` 片段**。
//! 3. **抖动稳定判据**（[`stable_gone`]）：设备不在要**连续** ≥ [`MISS_ROUNDS`]
//!    轮、且距第一次确认已过 [`MISS_MIN_WAIT`] 才动手。USB 复位 / 驱动重载 /
//!    主机侧拨号器重建会话都会让网卡短暂消失（上游注释里实测 40~90 秒），
//!    一轮就删会把正在恢复的接口拆掉。
//!
//! ## 分层
//!
//! | 部分 | 职责 | 单测 |
//! | --- | --- | --- |
//! | [`parse_sections`] | 把 `uci show network` 原文解析成「section 全量选项」 | 可 |
//! | [`classify`] / [`record_from`] | 纯策略：该不该删、记录该写什么 | 可 |
//! | [`stable_gone`] | 纯策略：抖动窗口到了没 | 可 |
//! | [`IfaceLedger`] | 生成记录落盘（`netprov_ifaces.json`） | 可（临时目录） |
//! | [`sweep`] | 编排：读记录 → 判定 → 留档 → 下发删除 → 清记录 | 需 `uci` |

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::uci;
use super::{
    is_managed_proto, is_our_iface_name, ExistingIface, PlannedIface, UciOp, PROTO_NONE,
    PROTO_STATIC,
};

/// 连续确认「模组不在、设备也不在」的轮数下限（组网循环 ≤15 秒一轮 ⇒ 约 60 秒）。
pub const MISS_ROUNDS: u32 = 4;

/// 距第一次确认的等待窗口下限（秒）。
///
/// 90 秒的依据：主机侧拨号器的自愈复位 `AT+CFUN=1,1` 会让模组重新枚举，
/// 上游注释记录 40~90 秒才回来（`module/kernel/netifd` 与 `hostdial` 一带）；
/// USB 抖动 / 驱动重载同理。窗口内设备回来 ⇒ 计数清零，不删。
pub const MISS_MIN_WAIT: Duration = Duration::from_secs(90);

/// 只看这些键判断「用户动过没有」。
///
/// 与 [`super::diff_plan`] 写的键一一对应：范围外的键（`mtu` / `ip6ifaceid` /
/// `metric` 以外的用户自有项）不属于本程序，出现即视为用户改动。
pub const MANAGED_KEYS: [&str; 12] = [
    "proto",
    "device",
    "metric",
    "auto",
    "force_link",
    "mp_no_v6",
    "ipaddr",
    "netmask",
    "gateway",
    "dns",
    "extendprefix",
    "ip6addr",
];

/// `uci show network` 里的一个 interface section（**全量选项**，不只是已知键）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RawSection {
    pub name: String,
    pub options: BTreeMap<String, String>,
}

/// 把 `uci show network` 的原文解析成 interface section 列表。
///
/// 认两种行：
///
/// ```text
/// network.modem1=interface          ← section 诞生（只收 interface）
/// network.modem1.dns='1.1.1.1'      ← 选项；同一键重复出现 = UCI 列表
/// ```
///
/// 列表值（如 `dns`）在 `uci show` 里是**多行同键**，这里用空格拼成一个值 ——
/// 与 `diff_plan` 写 `dns` 时的空格分隔口径一致，比对不会误判。
/// 匿名 section（`network.@...`）与其它类型（`route` / `rule` / `switch`）一律跳过。
pub fn parse_sections(net: &str) -> Vec<RawSection> {
    let mut order: Vec<String> = Vec::new();
    let mut by_name: HashMap<String, RawSection> = HashMap::new();
    for line in net.lines() {
        let line = line.trim();
        let Some(eq) = line.find('=') else { continue };
        let Some(lhs) = line[..eq].strip_prefix("network.") else {
            continue;
        };
        if lhs.is_empty() || lhs.starts_with('@') {
            continue;
        }
        let value = uci::unquote(&line[eq + 1..]);
        match lhs.find('.') {
            // section 诞生行
            None => {
                if value != "interface" {
                    continue;
                }
                if !by_name.contains_key(lhs) {
                    order.push(lhs.to_string());
                    by_name.insert(
                        lhs.to_string(),
                        RawSection {
                            name: lhs.to_string(),
                            options: BTreeMap::new(),
                        },
                    );
                }
            }
            // 选项行（section 名里不可能再有点，这里在第一个点处切即可）
            Some(dot) => {
                let (name, key) = (&lhs[..dot], &lhs[dot + 1..]);
                if key.is_empty() || key.contains('.') {
                    continue;
                }
                let Some(sec) = by_name.get_mut(name) else {
                    continue; // 没有诞生行（被删了一半的畸形配置）⇒ 不猜
                };
                match sec.options.get_mut(key) {
                    Some(old) => {
                        // UCI 列表：`uci show` 每个成员一行，拼成空格分隔
                        old.push(' ');
                        old.push_str(&value);
                    }
                    None => {
                        sec.options.insert(key.to_string(), value);
                    }
                }
            }
        }
    }
    order
        .into_iter()
        .filter_map(|n| by_name.remove(&n))
        .collect()
}

/// 本程序下发过的「生成记录」（按接口名一条）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IfaceRecord {
    /// 生成时这台模组的 id（模组拔了以后用它判断「该不该清」）。
    pub modem_id: u32,
    /// 生成时本程序写下的选项（**只含 [`MANAGED_KEYS`]**）。
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    /// 记录被更新的时刻（RFC3339；排障用）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recorded_at: String,
}

/// 由「UCI 现状」+「本轮计划」算出应当记录下来的生成记录（纯函数）。
///
/// 基数是现状里的[`MANAGED_KEYS`]（保住 `force_link` 这类「只写不删」的键 ——
/// `diff_plan` 有意不清它，记录里也不该假装它不在），再用计划覆盖：
///
/// - 计划里有值的键 ⇒ 记计划值；
/// - 计划里为空、且 `diff_plan` 会清掉的那些（`mp_no_v6` / `ipaddr` 一组 /
///   `extendprefix` / `ip6addr` / `dns`）⇒ 从记录里移除；
/// - `auto` 恒写；`metric` 只在 `>0` 时写；`force_link` 只在为真时写、为假时保留现状。
///
/// ⚠️ 记录**永远不含**用户自己加的键（`mtu` / `ip6ifaceid` …）：它们是
/// [`classify`] 判定「用户改动」的依据。
pub fn record_from(
    current: Option<&BTreeMap<String, String>>,
    iface: &PlannedIface,
    modem_id: u32,
) -> IfaceRecord {
    let mut options: BTreeMap<String, String> = BTreeMap::new();
    if let Some(cur) = current {
        for k in MANAGED_KEYS {
            if let Some(v) = cur.get(k) {
                options.insert(k.to_string(), v.clone());
            }
        }
    }
    options.insert("proto".into(), iface.proto.to_string());
    if iface.device.trim().is_empty() {
        options.remove("device");
    } else {
        options.insert("device".into(), iface.device.trim().to_string());
    }
    if iface.metric > 0 {
        options.insert("metric".into(), iface.metric.to_string());
    } else {
        options.remove("metric");
    }
    options.insert("auto".into(), if iface.auto { "1" } else { "0" }.into());
    if iface.force_link {
        options.insert("force_link".into(), "1".into());
    }
    if iface.mp_no_v6 {
        options.insert("mp_no_v6".into(), "1".into());
    } else {
        options.remove("mp_no_v6");
    }
    if iface.ipaddr.trim().is_empty() {
        for k in ["ipaddr", "netmask", "gateway"] {
            options.remove(k);
        }
    } else {
        options.insert("ipaddr".into(), iface.ipaddr.trim().to_string());
        if !iface.netmask.trim().is_empty() {
            options.insert("netmask".into(), iface.netmask.trim().to_string());
        }
        if !iface.gateway.trim().is_empty() {
            options.insert("gateway".into(), iface.gateway.trim().to_string());
        }
    }
    if iface.extendprefix {
        options.insert("extendprefix".into(), "1".into());
    } else {
        options.remove("extendprefix");
    }
    if iface.ip6addr.trim().is_empty() {
        options.remove("ip6addr");
    } else {
        options.insert("ip6addr".into(), iface.ip6addr.trim().to_string());
    }
    if iface.dns.trim().is_empty() {
        options.remove("dns");
    } else {
        options.insert("dns".into(), iface.dns.trim().to_string());
    }
    IfaceRecord {
        modem_id,
        options,
        recorded_at: crate::logging::rfc3339(crate::logging::unix_now()),
    }
}

/// 「该不该删」的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 名字/协议都不归本程序管（用户手工建的接口）。
    NotOurs,
    /// 是本程序的命名，但**没有生成记录**（老版本建的 / 记录文件丢了）。
    ///
    /// 保守起见一律保留：没有记录就没有「用户动过没有」的基线，
    /// 宁可留一张没人用的空接口，也不删掉用户可能改过的配置。
    NoRecord,
    /// 用户动过：列出来的是「与记录不一致」的键（排障与日志用）。
    UserEdited { keys: Vec<String> },
    /// 本程序生成、用户没动过 ⇒ 可以删。
    Cleanable,
}

/// 判定一张残留接口能不能删（纯函数）。
///
/// 顺序（先便宜的、先保守的）：
/// 1. 名字必须是 `modem<N>` / `modem<N>_6`；
/// 2. proto 必须落在本程序会写的集合里（`static` / `none` 另带附加判据，
///    与 [`super::is_ours`] 同口径，避免把用户的 `static` 接口认成我们的）；
/// 3. 没有记录 ⇒ [`Verdict::NoRecord`]（保留）；
/// 4. 记录里的每个键都要能对上（缺了 / 值变了 ⇒ 用户改动）；
///    UCI 里多出来的键也视为用户改动（记录里永远不含用户自有键）。
pub fn classify(section: &RawSection, rec: Option<&IfaceRecord>) -> Verdict {
    if !is_our_iface_name(&section.name) {
        return Verdict::NotOurs;
    }
    let proto = section
        .options
        .get("proto")
        .map(String::as_str)
        .unwrap_or("");
    let proto_ours = if is_managed_proto(proto) {
        true
    } else {
        match proto {
            // 350 系列主接口写成 static；v6 兄弟接口可能是 static / none
            PROTO_STATIC => {
                !section
                    .options
                    .get("ipaddr")
                    .map(String::as_str)
                    .unwrap_or("")
                    .is_empty()
                    || !section
                        .options
                        .get("ip6addr")
                        .map(String::as_str)
                        .unwrap_or("")
                        .is_empty()
            }
            PROTO_NONE => section.name.ends_with("_6"),
            _ => false,
        }
    };
    if !proto_ours {
        return Verdict::NotOurs;
    }
    let Some(rec) = rec else {
        return Verdict::NoRecord;
    };
    let mut keys: Vec<String> = Vec::new();
    for (k, v) in &rec.options {
        match section.options.get(k) {
            Some(cur) if cur == v => {}
            _ => keys.push(k.clone()),
        }
    }
    for k in section.options.keys() {
        if !rec.options.contains_key(k) {
            keys.push(k.clone());
        }
    }
    if keys.is_empty() {
        Verdict::Cleanable
    } else {
        keys.sort();
        keys.dedup();
        Verdict::UserEdited { keys }
    }
}

/// 抖动窗口是否已过（纯函数，便于单测）。
///
/// **两个条件都要满足**：连续确认轮数 ≥ [`MISS_ROUNDS`]、且距第一次确认
/// ≥ [`MISS_MIN_WAIT`]。只有一个条件时不动手 —— 掉电重启后第一轮可能
/// 一上来就看到「模组不在、设备不在」，轮数到了但时间没到，那一刻最可能是
/// 开机时 USB 还没枚举完。
pub fn stable_gone(rounds: u32, waited: Duration) -> bool {
    rounds >= MISS_ROUNDS && waited >= MISS_MIN_WAIT
}

/// 设备现在还存不存在（网卡名走 `/sys/class/net`，`/dev/...` 走文件系统）。
///
/// 读不到 = false（当成不在），但**不代表立刻删**：还要过 [`stable_gone`]。
pub fn device_still_there(dev: &str) -> bool {
    let d = dev.trim();
    if d.is_empty() {
        return false;
    }
    if d.starts_with('/') {
        return Path::new(d).exists();
    }
    // 不用 libc 的 `if_nametoindex`：sysfs 判断不依赖平台绑定，Windows 上恒 false
    //（清理只在 unix 上跑，见 `uci::available`）。
    Path::new("/sys/class/net").join(d).exists()
}

// ---------- 生成记录落盘 ----------

#[derive(Debug, Default, Serialize, Deserialize)]
struct LedgerFile {
    #[serde(default)]
    version: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    saved_at: String,
    #[serde(default)]
    ifaces: HashMap<String, IfaceRecord>,
}

/// 生成记录台账（`netprov_ifaces.json`，与 `config.toml` 同目录）。
///
/// 不放进 `AppState` 以外的全局：组网循环是唯一写入者，一个实例足够；
/// 放 `AppState` 是为了与其它台账同一套生命周期（启动载入、退出不丢）。
pub struct IfaceLedger {
    path: String,
    inner: Mutex<HashMap<String, IfaceRecord>>,
}

impl IfaceLedger {
    /// 打开并载入（文件不存在 / 损坏都按空台账起步，记 warn 但不阻断）。
    pub fn new(path: String) -> Self {
        let mut inner = HashMap::new();
        if let Some(f) = crate::runtime::store::load_json::<LedgerFile>(&path) {
            for (k, v) in f.ifaces {
                let k = k.trim();
                if !k.is_empty() && is_our_iface_name(k) {
                    inner.insert(k.to_string(), v);
                }
            }
        }
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn get(&self, name: &str) -> Option<IfaceRecord> {
        self.inner.lock().unwrap().get(name.trim()).cloned()
    }

    /// 记下（或不覆盖）一张接口的生成记录；内容没变就不写盘。
    pub fn put(&self, name: &str, rec: IfaceRecord) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let changed = {
            let mut g = self.inner.lock().unwrap();
            match g.get(name) {
                Some(old) if old.modem_id == rec.modem_id && old.options == rec.options => false,
                _ => {
                    g.insert(name.to_string(), rec);
                    true
                }
            }
        };
        if changed {
            self.flush();
        }
    }

    /// 删一条记录（接口已不存在 / 已删除）。
    pub fn remove(&self, name: &str) {
        let removed = self.inner.lock().unwrap().remove(name.trim()).is_some();
        if removed {
            self.flush();
        }
    }

    /// 全部记录（按接口名排序，便于稳定日志）。
    pub fn snapshot(&self) -> Vec<(String, IfaceRecord)> {
        let g = self.inner.lock().unwrap();
        let mut out: Vec<(String, IfaceRecord)> =
            g.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn flush(&self) {
        if self.path.trim().is_empty() {
            return;
        }
        let ifaces: HashMap<String, IfaceRecord> = self.inner.lock().unwrap().clone();
        let payload = LedgerFile {
            version: 1,
            saved_at: crate::logging::timestamp(),
            ifaces,
        };
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                crate::runtime::store::write_json_atomic(&self.path, &text);
            }
            Err(e) => log::warn!("序列化生成记录失败: {e}"),
        }
    }
}

// ---------- 删除前留档 ----------

/// 一条留档（被删接口的完整配置 + 可直接执行的恢复命令）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub at: String,
    pub iface: String,
    pub modem_id: u32,
    /// 被删时的完整选项（含用户自有键，恢复要用它）
    pub options: BTreeMap<String, String>,
    /// 可直接粘贴执行的 `uci` 片段（先 set 选项，再 set section，最后 commit）
    pub recover: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ArchiveFile {
    #[serde(default)]
    version: i64,
    #[serde(default)]
    entries: Vec<ArchiveEntry>,
}

/// 留档最多保留多少条（低频事件，20 条足够回溯，也不会把台账撑大）。
const ARCHIVE_KEEP: usize = 20;

/// 由接口现状生成一条留档（纯函数；`at` 由调用方给，便于单测）。
pub fn archive_entry(section: &RawSection, modem_id: u32, at: &str) -> ArchiveEntry {
    let mut recover = vec![format!("uci set network.{}=interface", section.name)];
    for (k, v) in &section.options {
        // 值里可能有空格（`dns` 列表）⇒ 统一加单引号，照抄即可执行
        recover.push(format!("uci set network.{}.{k}='{v}'", section.name));
    }
    recover.push(format!("uci commit network"));
    ArchiveEntry {
        at: at.to_string(),
        iface: section.name.clone(),
        modem_id,
        options: section.options.clone(),
        recover,
    }
}

/// 追加一条留档到 `netprov_cleanup.json`（保留最近 [`ARCHIVE_KEEP`] 条）。
pub fn archive(path: &str, entry: ArchiveEntry) {
    if path.trim().is_empty() {
        return;
    }
    let mut file = crate::runtime::store::load_json::<ArchiveFile>(path).unwrap_or_default();
    file.version = 1;
    file.entries.push(entry);
    if file.entries.len() > ARCHIVE_KEEP {
        let cut = file.entries.len() - ARCHIVE_KEEP;
        file.entries.drain(0..cut);
    }
    match serde_json::to_string_pretty(&file) {
        Ok(text) => {
            crate::runtime::store::write_json_atomic(path, &text);
        }
        Err(e) => log::warn!("序列化清理留档失败: {e}"),
    }
}

// ---------- 抖动计数 ----------

#[derive(Debug, Default)]
struct Miss {
    first: Option<Instant>,
    rounds: u32,
}

fn miss_table() -> &'static Mutex<HashMap<u32, Miss>> {
    static TABLE: OnceLock<Mutex<HashMap<u32, Miss>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// 记一轮「这台模组不在」并返回（轮数，已等待时长）。
fn note_miss(modem_id: u32, now: Instant) -> (u32, Duration) {
    let mut g = miss_table().lock().unwrap_or_else(|e| e.into_inner());
    let e = g.entry(modem_id).or_default();
    if e.first.is_none() {
        e.first = Some(now);
    }
    e.rounds = e.rounds.saturating_add(1);
    let waited = e.first.map(|t| now.duration_since(t)).unwrap_or_default();
    (e.rounds, waited)
}

/// 模组恢复 / 接口不用删时清零（下次拔卡重新从第一轮数）。
fn clear_miss(modem_id: u32) {
    miss_table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&modem_id);
}

// ---------- 编排 ----------

/// 跑一轮「清理残留接口」（组网循环每轮调一次，`auto_wan` 开着才调）。
///
/// `live` = 当前管理里**存在的**模组 id（不看 `is_running`：模组在但不一定
/// 有人查过，接口仍然是它的）；`existing` 给防火墙 zone 反查；`sections` 是
/// 同一份 `uci show network` 的全量解析（避免为清理再 fork 一次）。
///
/// 只有「记录里存在、且对应模组不在」的接口才会进入判定；一张都没有时
/// 连 UCI 都不读（调用方已读好的除外）。
pub fn sweep(
    state: &crate::http::AppState,
    live: &HashSet<u32>,
    existing: &[ExistingIface],
    sections: &[RawSection],
    zone_sections: &HashMap<String, String>,
) {
    let records = state.iface_ledger.snapshot();
    if records.is_empty() {
        return;
    }
    let by_name: HashMap<&str, &RawSection> =
        sections.iter().map(|s| (s.name.as_str(), s)).collect();
    let zone = if state.cfg.net.wan_zone.trim().is_empty() {
        super::FIREWALL_ZONE.to_string()
    } else {
        state.cfg.net.wan_zone.trim().to_string()
    };
    let zone_section = zone_sections
        .get(&zone)
        .cloned()
        .unwrap_or_else(|| format!("firewall.{zone}"));

    let now = Instant::now();
    let mut ops: Vec<UciOp> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut removed: Vec<(String, u32)> = Vec::new();

    for (name, rec) in records {
        // ① 模组还在 ⇒ 接口就是它该有的，清计数
        if live.contains(&rec.modem_id) {
            clear_miss(rec.modem_id);
            continue;
        }
        // ② 接口已经不在了（用户自己删的 / 上一轮删掉了）⇒ 记录直接作废
        let Some(section) = by_name.get(name.as_str()) else {
            dropped.push(name);
            continue;
        };
        // ③ 设备还在 ⇒ 可能已被别的模组 / 用户接管，不动（上游 `deviceStillThere`）
        let dev = rec.options.get("device").map(String::as_str).unwrap_or("");
        if device_still_there(dev) {
            clear_miss(rec.modem_id);
            continue;
        }
        // ④ 抖动窗口：连续多轮 + 最小等待都满足才动手
        let (rounds, waited) = note_miss(rec.modem_id, now);
        if !stable_gone(rounds, waited) {
            log::debug!(
                "清理残留接口 {name}：设备 {dev:?} 不在（第 {rounds} 轮 / 已等 {}s），等待稳定",
                waited.as_secs()
            );
            continue;
        }
        // ⑤ 生成记录比对：用户动过就保留（并写清动过哪些键）
        match classify(section, Some(&rec)) {
            Verdict::Cleanable => {
                let entry = archive_entry(section, rec.modem_id, &crate::logging::timestamp());
                log::warn!(
                    "清理残留接口 {name}（模组 {} 已不在、设备 {dev:?} 不在，连续 {rounds} 轮）：\
                     已留档到 {}，需要恢复请执行：{}",
                    rec.modem_id,
                    state.iface_ledger.path(),
                    entry.recover.join(" && ")
                );
                archive(&cleanup_archive_path(state), entry);
                ops.push(UciOp::Remove {
                    section: name.clone(),
                });
                if existing
                    .iter()
                    .find(|e| e.name == name)
                    .map(|e| e.zones.iter().any(|z| z == &zone))
                    .unwrap_or(false)
                {
                    ops.push(UciOp::DelList {
                        section: zone_section.clone(),
                        key: "network".into(),
                        value: name.clone(),
                    });
                }
                removed.push((name, rec.modem_id));
            }
            Verdict::UserEdited { keys } => {
                log::info!(
                    "清理残留接口 {name}：用户在 UCI 里改过 {}，保留不动（模组 {} 已不在）",
                    keys.join("/"),
                    rec.modem_id
                );
                clear_miss(rec.modem_id);
            }
            Verdict::NoRecord => {
                // snapshot() 里的条目一定来自台账，这条分支只会在记录被并发清掉时出现，
                // 保留原接口更安全
                log::info!("清理残留接口 {name}：没有生成记录，保留不动");
            }
            Verdict::NotOurs => {
                log::info!("清理残留接口 {name}：proto 不归本程序管，保留不动");
                clear_miss(rec.modem_id);
            }
        }
    }

    for name in dropped {
        state.iface_ledger.remove(&name);
    }
    if ops.is_empty() {
        return;
    }
    match uci::apply(&ops) {
        Ok(_) => {
            for (name, modem_id) in removed {
                state.iface_ledger.remove(&name);
                clear_miss(modem_id);
                super::provision::forget_device(modem_id);
            }
            if let Err(e) = uci::reload_network() {
                log::warn!("清理残留接口：UCI 已写入，但 network reload 失败: {e}");
            }
        }
        Err(e) => log::warn!("清理残留接口失败（保留生成记录，下轮重试）: {e}"),
    }
}

/// 清理留档文件的路径（与 `config.toml` 同目录）。
pub fn cleanup_archive_path(state: &crate::http::AppState) -> String {
    crate::runtime::store::netprov_cleanup_file(&state.config_path)
}

/// 生成记录文件的路径（与 `config.toml` 同目录）。
pub fn ledger_path(config_path: &str) -> String {
    crate::runtime::store::netprov_ifaces_file(config_path)
}

/// 仅供测试：确认留档与记录文件不会互相覆盖。
#[cfg(test)]
pub(crate) fn paths_distinct(config_path: &str) -> bool {
    ledger_path(config_path) != crate::runtime::store::netprov_cleanup_file(config_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WanMode;
    use crate::netprov::plan_wan;
    use crate::netprov::V6Plan;

    fn section(name: &str, pairs: &[(&str, &str)]) -> RawSection {
        let mut options = BTreeMap::new();
        for (k, v) in pairs {
            options.insert((*k).to_string(), (*v).to_string());
        }
        RawSection {
            name: name.to_string(),
            options,
        }
    }

    fn record(options: &[(&str, &str)]) -> IfaceRecord {
        let mut m = BTreeMap::new();
        for (k, v) in options {
            m.insert((*k).to_string(), (*v).to_string());
        }
        IfaceRecord {
            modem_id: 1,
            options: m,
            recorded_at: String::new(),
        }
    }

    #[test]
    fn parse_collects_options_and_joins_lists() {
        let net = "\
network.loopback=interface
network.loopback.proto='static'
network.modem1=interface
network.modem1.proto='modem_pro'
network.modem1.device='wwan0'
network.modem1.dns='1.1.1.1'
network.modem1.dns='8.8.8.8'
network.modem1.mtu='1420'
network.@route[0]=route
";
        let secs = parse_sections(net);
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].name, "loopback");
        assert_eq!(secs[1].name, "modem1");
        assert_eq!(
            secs[1].options.get("proto").map(String::as_str),
            Some("modem_pro")
        );
        // 列表值拼成空格分隔（与 diff_plan 写 dns 的口径一致）
        assert_eq!(
            secs[1].options.get("dns").map(String::as_str),
            Some("1.1.1.1 8.8.8.8")
        );
        // 用户自有键也收进来 —— 判定「用户改动」就靠它
        assert!(secs[1].options.contains_key("mtu"));
    }

    #[test]
    fn parse_ignores_non_interface_and_orphan_options() {
        let net = "\
network.globals=globals
network.modem1.proto='modem_pro'
network.@switch[0]=switch
";
        assert!(
            parse_sections(net).is_empty(),
            "没有 section 诞生行的选项行不猜"
        );
    }

    #[test]
    fn classify_rejects_foreign_names_and_protos() {
        let ours = record(&[("proto", "modem_pro"), ("device", "wwan0")]);
        let foreign = section("wan", &[("proto", "dhcp")]);
        assert_eq!(classify(&foreign, Some(&ours)), Verdict::NotOurs);
        let pppoe = section("modem1", &[("proto", "pppoe")]);
        assert_eq!(classify(&pppoe, Some(&ours)), Verdict::NotOurs);
        // 用户手起的 static 接口（没有 ipaddr / ip6addr）也不能认
        let user_static = section("modem1", &[("proto", "static")]);
        assert_eq!(classify(&user_static, Some(&ours)), Verdict::NotOurs);
        // `none` 只在 `_6` 兄弟接口上归我们
        assert_eq!(
            classify(&section("modem1", &[("proto", "none")]), Some(&ours)),
            Verdict::NotOurs
        );
        // `_6` 上 proto=none 归我们，但与记录（主接口那份）对不上 ⇒ 判为用户改动，保留
        assert!(matches!(
            classify(&section("modem1_6", &[("proto", "none")]), Some(&ours)),
            Verdict::UserEdited { .. }
        ));
    }

    #[test]
    fn classify_without_record_keeps_interface() {
        let sec = section(
            "modem1",
            &[("proto", "modem_pro"), ("device", "wwan0"), ("auto", "1")],
        );
        assert_eq!(classify(&sec, None), Verdict::NoRecord);
    }

    #[test]
    fn classify_exact_match_is_cleanable() {
        let sec = section(
            "modem1",
            &[("proto", "modem_pro"), ("device", "wwan0"), ("auto", "1")],
        );
        let rec = record(&[("proto", "modem_pro"), ("device", "wwan0"), ("auto", "1")]);
        assert_eq!(classify(&sec, Some(&rec)), Verdict::Cleanable);
    }

    #[test]
    fn classify_detects_user_extra_key() {
        let sec = section(
            "modem1",
            &[
                ("proto", "modem_pro"),
                ("device", "wwan0"),
                ("auto", "1"),
                ("mtu", "1420"),
            ],
        );
        let rec = record(&[("proto", "modem_pro"), ("device", "wwan0"), ("auto", "1")]);
        assert_eq!(
            classify(&sec, Some(&rec)),
            Verdict::UserEdited {
                keys: vec!["mtu".to_string()]
            }
        );
    }

    #[test]
    fn classify_detects_changed_and_missing_keys() {
        let rec = record(&[
            ("proto", "modem_pro"),
            ("device", "wwan0"),
            ("auto", "1"),
            ("metric", "20"),
        ]);
        let changed = section(
            "modem1",
            &[
                ("proto", "modem_pro"),
                ("device", "wwan0"),
                ("auto", "0"),
                ("metric", "20"),
            ],
        );
        assert_eq!(
            classify(&changed, Some(&rec)),
            Verdict::UserEdited {
                keys: vec!["auto".to_string()]
            }
        );
        let missing = section(
            "modem1",
            &[("proto", "modem_pro"), ("auto", "1"), ("metric", "20")],
        );
        assert_eq!(
            classify(&missing, Some(&rec)),
            Verdict::UserEdited {
                keys: vec!["device".to_string()]
            }
        );
    }

    #[test]
    fn record_from_mirrors_plan_and_keeps_force_link() {
        // QMI + 主机侧拨号：force_link 由 provision 层置位（plan_wan 本身不管这个），
        // 这里手工构造一个等价计划，确认记录里只收本程序写的键
        let mut plan = plan_wan(
            1,
            "qmi",
            WanMode::ModemPro,
            "wwan0",
            V6Plan::of(crate::netprov::v6::V6Mode::Dhcpv6, "", true),
            20,
            true,
            "",
        );
        plan.ifaces[0].force_link = true;
        plan.ifaces[0].dns = "119.29.29.29".into();
        let mut current = BTreeMap::new();
        current.insert("force_link".to_string(), "1".to_string());
        current.insert("mtu".to_string(), "1420".to_string());
        let rec = record_from(Some(&current), &plan.ifaces[0], 1);
        assert_eq!(rec.modem_id, 1);
        assert_eq!(
            rec.options.get("proto").map(String::as_str),
            Some("modem_pro")
        );
        assert_eq!(rec.options.get("device").map(String::as_str), Some("wwan0"));
        assert_eq!(rec.options.get("metric").map(String::as_str), Some("20"));
        assert_eq!(rec.options.get("force_link").map(String::as_str), Some("1"));
        assert_eq!(
            rec.options.get("dns").map(String::as_str),
            Some("119.29.29.29")
        );
        assert!(
            !rec.options.contains_key("mtu"),
            "用户自有键不能进生成记录，否则以后判不出它是不是用户加的"
        );
    }

    #[test]
    fn record_from_drops_keys_plan_no_longer_wants() {
        let mut current = BTreeMap::new();
        current.insert("ipaddr".to_string(), "10.0.0.2".to_string());
        current.insert("netmask".to_string(), "255.255.255.0".to_string());
        current.insert("gateway".to_string(), "10.0.0.1".to_string());
        current.insert("mp_no_v6".to_string(), "1".to_string());
        let plan = plan_wan(
            1,
            "ecm",
            WanMode::ModemPro,
            "eth2",
            V6Plan::off(),
            20,
            true,
            "",
        );
        let rec = record_from(Some(&current), &plan.ifaces[0], 1);
        assert!(!rec.options.contains_key("ipaddr"));
        assert!(!rec.options.contains_key("netmask"));
        assert!(!rec.options.contains_key("gateway"));
        assert!(!rec.options.contains_key("mp_no_v6"));
    }

    #[test]
    fn stable_gone_needs_both_rounds_and_wait() {
        assert!(!stable_gone(0, Duration::from_secs(600)));
        assert!(!stable_gone(MISS_ROUNDS, Duration::from_secs(1)));
        assert!(!stable_gone(1, MISS_MIN_WAIT));
        assert!(stable_gone(MISS_ROUNDS, MISS_MIN_WAIT));
        assert!(stable_gone(MISS_ROUNDS + 5, MISS_MIN_WAIT * 2));
    }

    #[test]
    fn device_still_there_handles_empty_and_netdev() {
        assert!(!device_still_there(""));
        // 测试机上一定不存在的名字
        assert!(!device_still_there("definitely-not-a-real-iface"));
        // 空设备名 ⇒ false（不拦清理；这类接口本来就绑不到东西）
    }

    #[test]
    fn archive_snippet_is_paste_ready() {
        let sec = section(
            "modem1",
            &[
                ("proto", "modem_pro"),
                ("device", "wwan0"),
                ("auto", "1"),
                ("dns", "1.1.1.1 8.8.8.8"),
            ],
        );
        let e = archive_entry(&sec, 1, "2026-11-08T00:00:00Z");
        assert_eq!(e.iface, "modem1");
        assert!(e.recover[0].contains("network.modem1=interface"));
        assert!(e
            .recover
            .iter()
            .any(|c| c.contains("network.modem1.dns='1.1.1.1 8.8.8.8'")));
        assert_eq!(
            e.recover.last().map(String::as_str),
            Some("uci commit network")
        );
    }

    #[test]
    fn archive_appends_and_caps_entries() {
        let dir = std::env::temp_dir().join(format!("modemd-clean-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir
            .join("netprov_cleanup.json")
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let sec = section("modem1", &[("proto", "modem_pro")]);
        for i in 0..(ARCHIVE_KEEP + 5) {
            archive(&path, archive_entry(&sec, 1, &format!("t{i}")));
        }
        let file = crate::runtime::store::load_json::<ArchiveFile>(&path).expect("留档应能读回");
        assert_eq!(file.entries.len(), ARCHIVE_KEEP);
        assert_eq!(
            file.entries.last().unwrap().at,
            format!("t{}", ARCHIVE_KEEP + 4)
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ledger_roundtrip_and_ignores_foreign_names() {
        let dir = std::env::temp_dir().join(format!("modemd-ledger-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir
            .join("netprov_ifaces.json")
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);

        let l = IfaceLedger::new(path.clone());
        assert!(l.get("modem1").is_none());
        l.put(
            "modem1",
            record(&[("proto", "modem_pro"), ("device", "wwan0")]),
        );
        // 同内容不重复落盘（这里只看行为：记录仍在）
        l.put(
            "modem1",
            record(&[("proto", "modem_pro"), ("device", "wwan0")]),
        );
        assert_eq!(
            IfaceLedger::new(path.clone())
                .get("modem1")
                .unwrap()
                .options
                .get("device")
                .map(String::as_str),
            Some("wwan0")
        );
        // 非本程序命名不进台账
        std::fs::write(
            &path,
            r#"{"version":1,"ifaces":{"wan":{"modem_id":0,"options":{}}}}"#,
        )
        .unwrap();
        assert!(IfaceLedger::new(path.clone()).snapshot().is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_paths_are_distinct() {
        assert!(paths_distinct("/etc/modemd/config.toml"));
    }
}
