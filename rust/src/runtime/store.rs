//! 运行期台账落盘（全部与配置文件同目录）：
//! `quota.json` / `imei.json` / `atport.json` / `atport_bind.json` / `schedule.json`。
//!
//! ## 为什么要单独落盘
//!
//! 配额的「本周期已用」、IMEI 原值备份、手动指定的 AT 口、定时任务「哪些槽位
//! 已经跑过」都是**跨重启**的账 —— 重启一次就清零，等于这些功能形同虚设。
//! config.toml 是 conffile（升级不覆盖、也不该被程序改写），所以运行期数据另放。
//!
//! ## 改任何一个 store 都要遵守的约定
//!
//! · **失败降级**：文件不存在 / 损坏 / 读不了都按「空台账」起步并记一条 warn，
//!   绝不阻断启动；写失败同样只 warn（配额不是核心链路，落不了盘也要让页面出数）；
//! · **并发**：每个 store 一把 `Mutex` 护住内存表，落盘时**不持锁做 IO** ——
//!   写入路径一律是「加锁改内存 → drop 锁 → `flush()` 重新加锁拷快照」；
//! · **键的类型**：`quota` / `imei` 用模组 id（`u32`，落盘转字符串，读回解析失败就跳过）；
//!   `atport` 用**模组身份**（`imei:<imei>` / `model:<model>` / 未识别时的 `ports:<当前口>`）；
//!   `atport_bind` 用 USB 设备身份（`vid:pid` 或 `vid:pid@<总线路径>`，见
//!   [`at_port_bind_key_scoped`]）；**都不用端口号当键** —— 端口号正是要换的那个东西。
//!
//! ## 原子性（现状如此，不是目标态）
//!
//! `SchedStore` 与 `HostNicStore`（`runtime/hostnic.rs`）走 `.tmp` + `rename`；
//! `quota` / `imei` / `atport` / `atport_bind` 是**直接覆盖写** —— 写一半掉电会留下
//! 损坏文件，靠上面的「损坏按空台账」兜住（代价是丢一次账，不是起不来服务）。
//! 统一成原子写属实机写盘行为变更，未做，见 docs/UPSTREAM_AUDIT.md 的取舍说明。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::modem::model::quota::QuotaSettings;

/// 读一个 JSON 台账文件（约定见模块头的「失败降级」）。
///
/// 路径为空 / 文件不存在 / 解析失败一律 `None`：前两者是常态（没配路径、
/// 首次启动），不记日志；解析失败与读取失败各记一条 warn。
/// 键的解析与清洗**不在这里做** —— 每个 store 的键类型（u32 / 字符串）、
/// 清洗口径（去空白 / 跳过坏项）不同，那部分留在调用方。
/// 原子写一个 JSON 台账：先写 `<path>.tmp` 再 rename。
///
/// 写一半掉电只会留下 `.tmp`（下次被覆盖），正式文件始终完整 —— 代价是
/// 断电丢最近一次改动，换来的是不留下解析不了的坏文件。
/// 返回值 = rename 是否成功：调用方据此决定要不要清脏标记（不清则下次重试）。
/// 失败只 warn，不向上抛：落不了盘也要让服务继续跑（模块头的降级约定）。
pub fn write_json_atomic(path: &str, text: &str) -> bool {
    let tmp = format!("{path}.tmp");
    if let Err(e) = std::fs::write(&tmp, text.as_bytes()) {
        log::warn!("写 {tmp} 失败: {e}");
        return false;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        log::warn!("改名 {tmp} → {path} 失败: {e}");
        return false;
    }
    true
}

pub fn load_json<T: serde::de::DeserializeOwned>(path: &str) -> Option<T> {
    if path.trim().is_empty() {
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<T>(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("解析 {path} 失败，按空台账继续: {e}");
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("读取 {path} 失败，按空台账继续: {e}");
            None
        }
    }
}

/// 一个模组的配额台账。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuotaRecord {
    /// 用户在页面上保存过的设置（None = 用 config.toml 的默认值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<QuotaSettings>,
    /// 本周期起点 / 止点（Unix 秒；止点 0 = 不自动重置）
    pub period_start: i64,
    pub period_end: i64,
    /// 本周期累计上行 / 下行（字节）
    pub used_up: u64,
    pub used_down: u64,
    /// 上次采样时模组的累计计数器（计数式差分用）
    pub last_tx: u64,
    pub last_rx: u64,
    /// 上次采样时的瞬时速率与时刻（速率式积分用）
    pub last_rate_up: u64,
    pub last_rate_down: u64,
    pub last_sample_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_at: Option<String>,
    /// 上一次真正下发断网动作的时刻（按 `block_interval` 反复压制「模组自己又拨上了」）
    pub last_block_at: i64,
    /// 本周期已解除限制
    pub suspended: bool,
    /// 当前是否处于「已断网」状态
    pub blocked: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct QuotaFile {
    modems: HashMap<String, QuotaRecord>,
}

/// 配额台账的读写（进程内缓存 + 每次改动落盘）。
pub struct QuotaStore {
    path: String,
    inner: Mutex<HashMap<u32, QuotaRecord>>,
}

impl QuotaStore {
    /// 打开（文件不存在 / 损坏都按空台账起步，并记一条 warn）。
    pub fn new(path: String) -> Self {
        let mut inner: HashMap<u32, QuotaRecord> = HashMap::new();
        if let Some(f) = load_json::<QuotaFile>(&path) {
            for (k, v) in f.modems {
                if let Ok(id) = k.parse::<u32>() {
                    inner.insert(id, v);
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

    /// 取一个模组的台账（没有就给一份空的）。
    pub fn get(&self, modem_id: u32) -> QuotaRecord {
        self.inner
            .lock()
            .unwrap()
            .get(&modem_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 写回一个模组的台账（立即落盘）。
    pub fn put(&self, modem_id: u32, rec: QuotaRecord) {
        {
            let mut guard = self.inner.lock().unwrap();
            guard.insert(modem_id, rec);
        }
        self.flush();
    }

    fn flush(&self) {
        if self.path.trim().is_empty() {
            return;
        }
        let mut modems = HashMap::new();
        for (id, rec) in self.inner.lock().unwrap().iter() {
            modems.insert(id.to_string(), rec.clone());
        }
        match serde_json::to_string_pretty(&QuotaFile { modems }) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.path, text) {
                    log::warn!("写 {path} 失败: {e}", path = self.path);
                }
            }
            Err(e) => log::warn!("序列化配额台账失败: {e}"),
        }
    }
}

/// IMEI 原始值备份（改 IMEI 前自动存一份，之后才能一键恢复）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImeiBackup {
    /// 第一次改写前读到的值（它就是原始值；空串 = 模组出厂 / 维修状态即空 IMEI）
    pub original: String,
    /// 非空即「已备份过」—— 判定用这个，**不能**用 `original` 非空
    /// （空 IMEI 模组的原值就是空串）
    pub saved_at: String,
    /// 原值是用哪条只读命令读到的（`AT+EGMREXT=0,7` / `AT+CGSN`），恢复时好回溯
    #[serde(default)]
    pub source: String,
    /// 本程序改写过几次
    pub writes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_at: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ImeiFile {
    modems: HashMap<String, ImeiBackup>,
}

/// IMEI 备份台账（`imei.json`，与配置文件同目录）。
pub struct ImeiStore {
    path: String,
    inner: Mutex<HashMap<u32, ImeiBackup>>,
}

impl ImeiStore {
    pub fn new(path: String) -> Self {
        let mut inner: HashMap<u32, ImeiBackup> = HashMap::new();
        if let Some(f) = load_json::<ImeiFile>(&path) {
            for (k, v) in f.modems {
                if let Ok(id) = k.parse::<u32>() {
                    inner.insert(id, v);
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

    pub fn get(&self, modem_id: u32) -> Option<ImeiBackup> {
        self.inner.lock().unwrap().get(&modem_id).cloned()
    }

    pub fn put(&self, modem_id: u32, bak: ImeiBackup) {
        {
            self.inner.lock().unwrap().insert(modem_id, bak);
        }
        self.flush();
    }

    fn flush(&self) {
        if self.path.trim().is_empty() {
            return;
        }
        let mut modems = HashMap::new();
        for (id, rec) in self.inner.lock().unwrap().iter() {
            modems.insert(id.to_string(), rec.clone());
        }
        match serde_json::to_string_pretty(&ImeiFile { modems }) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.path, text) {
                    log::warn!("写 {path} 失败: {e}", path = self.path);
                }
            }
            Err(e) => log::warn!("序列化 IMEI 备份失败: {e}"),
        }
    }
}

/// 由配置文件路径推出 `quota.json` 的位置（与配置文件同目录）。
pub fn quota_file(config_path: &str) -> String {
    sibling_file(config_path, "quota.json")
}

/// 由配置文件路径推出 `imei.json` 的位置（与配置文件同目录）。
pub fn imei_file(config_path: &str) -> String {
    sibling_file(config_path, "imei.json")
}

/// 由配置文件路径推出 `atport.json` 的位置（与配置文件同目录）。
pub fn atport_file(config_path: &str) -> String {
    sibling_file(config_path, "atport.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AtPortFile {
    modems: HashMap<String, String>,
}

/// 手动指定的 AT 命令口（跨重启记住；`/api/at/port` 用）。
///
/// 键是「模组身份」（`imei:<imei>` / `model:<model>` / 未识别时的 `ports:<当前口>`），
/// **不是端口号** —— 端口号正是要换的那个东西；值是手动指定的命令口，
/// 空串 = 用自动探测（见 `modem::discovery`）。
///
/// 与「按 USB 设备身份」的 [`AtPortBindStore`]（`atport_bind.json`）是两套键：
/// 这台模组**身份读不出来时**（AT 口挑错 → IMEI/型号都读不到）只有后者能用，
/// 所以发现流程里绑定优先于本 store（见 `modem::manager::apply_saved_at_port`）。
pub struct AtPortStore {
    path: String,
    inner: Mutex<HashMap<String, String>>,
}

impl AtPortStore {
    /// 打开（文件不存在 / 损坏都按「没指定过」起步，并记一条 warn）。
    pub fn new(path: String) -> Self {
        let mut inner: HashMap<String, String> = HashMap::new();
        if let Some(f) = load_json::<AtPortFile>(&path) {
            for (k, v) in f.modems {
                let k = k.trim();
                if !k.is_empty() {
                    inner.insert(k.to_string(), v.trim().to_string());
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

    /// 这台模组手动指定的命令口（没指定过 / 指定为空 ⇒ 空串 = 自动探测）。
    pub fn get(&self, key: &str) -> String {
        let key = key.trim();
        if key.is_empty() {
            return String::new();
        }
        self.inner
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// 记下这台模组的手动指定（空串 = 恢复自动探测）；返回是否与原来不同。
    pub fn set(&self, key: &str, port: &str) -> bool {
        let key = key.trim();
        if key.is_empty() {
            return false;
        }
        let port = port.trim();
        let changed = {
            let mut guard = self.inner.lock().unwrap();
            match guard.get(key) {
                Some(old) if old == port => false,
                _ => {
                    guard.insert(key.to_string(), port.to_string());
                    true
                }
            }
        };
        if changed {
            self.flush();
        }
        changed
    }

    /// 把「值等于这个端口」的记录全部清掉（返回清了几条）。
    ///
    /// 用途：身份读不出来时（AT 口挑错了 IMEI 也读不到）界面拿不到键，
    /// 靠端口名反查着清记录 —— 保证用户永远能回到「自动探测」。
    pub fn clear_by_port(&self, port: &str) -> usize {
        let port = port.trim();
        if port.is_empty() {
            return 0;
        }
        let n = {
            let mut guard = self.inner.lock().unwrap();
            let before = guard.len();
            guard.retain(|_, v| v != port);
            before - guard.len()
        };
        if n > 0 {
            self.flush();
        }
        n
    }

    fn flush(&self) {
        if self.path.trim().is_empty() {
            return;
        }
        let mut modems = HashMap::new();
        for (k, v) in self.inner.lock().unwrap().iter() {
            modems.insert(k.clone(), v.clone());
        }
        match serde_json::to_string_pretty(&AtPortFile { modems }) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.path, text) {
                    log::warn!("写 {path} 失败: {e}", path = self.path);
                }
            }
            Err(e) => log::warn!("序列化 AT 口指定失败: {e}"),
        }
    }
}

/// 由配置文件路径推出 `atport_bind.json` 的位置（与配置文件同目录）。
pub fn atport_bind_file(config_path: &str) -> String {
    sibling_file(config_path, "atport_bind.json")
}

/// 归一「按 USB 设备身份（`vid:pid`）」绑定的键（小写四位十六进制；非法返回 None）。
///
/// 允许只给 `"vid:pid"`（界面手工粘进来的情况）；每段都要求**正好 4 位十六进制**，
/// 卡死它才能挡住手抖（`"abc"`、`"0e8d1"` 一律当没填）。
pub fn at_port_bind_key(vid: &str, pid: &str) -> Option<String> {
    let (mut v, mut p) = (vid.trim().to_string(), pid.trim().to_string());
    if p.is_empty() && v.contains(':') {
        let (nv, np) = crate::modem::usbdev::split_vid_pid(&v);
        v = nv;
        p = np;
    }
    let norm = |s: &str| -> Option<String> {
        let t = s.trim().to_lowercase();
        let t = match t.strip_prefix("0x") {
            Some(rest) => rest.to_string(),
            None => t,
        };
        if t.len() != 4 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(t)
    };
    Some(format!("{}:{}", norm(&v)?, norm(&p)?))
}

/// 带**物理路径**的绑定键：`vid:pid@1-1`。
///
/// 为什么需要：同一型号的两台模组 `vid:pid` 完全相同，只按 `vid:pid` 绑定会让
/// 第二台也被切到第一台的串口（等于把两台设备的命令口串起来）。USB 拓扑
/// 路径（`1-1` / `2-3.4`）在**同一台机器内唯一**，加进键里就能区分。
///
/// 兼容：老键（不带 `@`）仍然有效；发现流程先查带路径的，再退回老键。
pub fn at_port_bind_key_scoped(vid: &str, pid: &str, bus_path: &str) -> Option<String> {
    let base = at_port_bind_key(vid, pid)?;
    let bus = bus_path.trim();
    if bus.is_empty() {
        return Some(base);
    }
    if !bus
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
    {
        return None;
    }
    Some(format!("{base}@{bus}"))
}

#[derive(Debug, Serialize, Deserialize)]
struct AtPortBindFile {
    #[serde(default)]
    version: i64,
    #[serde(default)]
    saved_at: String,
    #[serde(default)]
    binds: HashMap<String, String>,
}

/// 按 USB 设备身份（`vid:pid`）绑定的命令口（`atport_bind.json`；`/api/atport/bind` 用）。
///
/// 与「按模组身份」的 [`AtPortStore`]（`atport.json`，`/api/at/port` 用）分工不同：
/// 键是 USB 设备身份，**不认识模组也能用** —— 自动探测挑错口、模组身份读不出来时，
/// 这是唯一的补救路径。绑定优先于按模组身份的指定。
pub struct AtPortBindStore {
    path: String,
    inner: Mutex<HashMap<String, String>>,
}

impl AtPortBindStore {
    /// 打开（文件不存在 / 损坏都按「没绑过」起步，并记一条 warn）。
    pub fn new(path: String) -> Self {
        let mut data: HashMap<String, String> = HashMap::new();
        if let Some(f) = load_json::<AtPortBindFile>(&path) {
            for (k, v) in f.binds {
                let key = k.trim();
                let port = v.trim();
                if !key.is_empty() && !port.is_empty() {
                    data.insert(key.to_string(), port.to_string());
                }
            }
        }
        Self {
            path,
            inner: Mutex::new(data),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// 这台 USB 设备（`vid:pid`）绑的命令口（没绑 / 空串 ⇒ 空 = 自动探测）。
    pub fn get(&self, key: &str) -> String {
        let key = key.trim();
        if key.is_empty() {
            return String::new();
        }
        self.inner
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// 全部绑定（按键排序，界面直接用）。
    pub fn list(&self) -> Vec<(String, String)> {
        let guard = self.inner.lock().unwrap();
        let mut out: Vec<(String, String)> =
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 绑定 / 解绑（port 为空 = 删掉这条），返回是否与原来不同。
    pub fn set(&self, key: &str, port: &str) -> bool {
        let key = key.trim();
        if key.is_empty() {
            return false;
        }
        let port = port.trim();
        let changed = {
            let mut guard = self.inner.lock().unwrap();
            match guard.get(key) {
                Some(old) if old == port => false,
                _ => {
                    if port.is_empty() {
                        guard.remove(key);
                    } else {
                        guard.insert(key.to_string(), port.to_string());
                    }
                    true
                }
            }
        };
        if changed {
            self.flush();
        }
        changed
    }

    fn flush(&self) {
        if self.path.trim().is_empty() {
            return;
        }
        let binds: HashMap<String, String> = self.inner.lock().unwrap().clone();
        let payload = AtPortBindFile {
            version: 1,
            saved_at: crate::logging::timestamp(),
            binds,
        };
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.path, text) {
                    log::warn!("写 {} 失败: {e}", self.path);
                }
            }
            Err(e) => log::warn!("序列化命令口绑定失败: {e}"),
        }
    }
}

/// 退出前要落盘的台账（信号处理里统一调）。
///
/// 大部分台账是「写一次就原子落盘」，只有定时任务台账按 5 秒节流合并写；
/// 进程收到 SIGTERM 时如果正好在节流窗口里，最后这次编辑会丢。注册一个
/// 刷盘回调，退出钩子按注册顺序调一遍，代价只有一次文件写。
type FlushFn = Box<dyn Fn() + Send + Sync + 'static>;
static FLUSHERS: Mutex<Vec<FlushFn>> = Mutex::new(Vec::new());

/// 注册一个退出前要刷盘的台账（重复注册同一对象也安全：flush 是幂等的）。
pub fn register_flush<F: Fn() + Send + Sync + 'static>(f: F) {
    FLUSHERS.lock().unwrap().push(Box::new(f));
}

/// 进程退出前调用：把注册过的台账全部刷一遍。
pub fn flush_all() {
    for f in FLUSHERS.lock().unwrap().iter() {
        f();
    }
}

/// 取配置文件同目录下的 `name`。
fn sibling_file(config_path: &str, name: &str) -> String {
    let p = std::path::Path::new(config_path);
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => dir.join(name).to_string_lossy().to_string(),
        None => name.to_string(),
    }
}

/// 由配置文件路径推出 `schedule.json` 的位置（与配置文件同目录）。
pub fn schedule_file(config_path: &str) -> String {
    sibling_file(config_path, "schedule.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SchedFile {
    version: i64,
    #[serde(default)]
    saved_at: String,
    #[serde(default)]
    tasks: Vec<crate::modem::model::schedule::Task>,
}

/// 定时任务台账（内存 + `schedule.json` 原子落盘，写盘用临时文件 + 改名保证原子性）。
///
/// 为什么单独落盘：定时任务「哪些槽位已经跑过」（`slots`）是**跨重启**的账 ——
/// 重启丢了它，进程一起来就会把 15 分钟补做窗口内的槽位再跑一遍。
/// 写失败只 warn 不阻断：定时器不是核心链路，落不了盘也要让页面照常出数据。
pub struct SchedStore {
    path: String,
    inner: Mutex<SchedInner>,
}

struct SchedInner {
    tasks: Vec<crate::modem::model::schedule::Task>,
    dirty: bool,
    /// 上次落盘时刻（Unix 秒；`record` 的落盘节流用）
    saved_at: i64,
    /// 每个任务一条失败日志的节流（坏任务别每 10 秒刷屏）
    last_log: HashMap<String, i64>,
}

impl SchedStore {
    /// 打开并载入（文件不存在 / 损坏都按空台账起步；损坏记 warn）。
    pub fn new(path: String) -> Self {
        let mut tasks: Vec<crate::modem::model::schedule::Task> = Vec::new();
        if let Some(f) = load_json::<SchedFile>(&path) {
            for mut t in f.tasks {
                if t.id.trim().is_empty() {
                    continue;
                }
                // 与 API 写入同一套校验：坏任务宁可跳过也不带病运行
                if let Err(e) = crate::modem::model::schedule::validate_task(&mut t) {
                    log::warn!("定时任务 {} 不合法，跳过: {e}", t.id);
                    continue;
                }
                tasks.push(t);
            }
        }
        let on = tasks.iter().filter(|t| t.enabled).count();
        log::info!("定时任务台账：{path}（{} 条，启用 {on}）", tasks.len());
        Self {
            path,
            inner: Mutex::new(SchedInner {
                tasks,
                dirty: false,
                saved_at: 0,
                last_log: HashMap::new(),
            }),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// 任务快照（加锁拷贝，供 API / 循环使用）。
    pub fn snapshot(&self) -> Vec<crate::modem::model::schedule::Task> {
        self.inner.lock().unwrap().tasks.clone()
    }

    /// 新增 / 更新一条任务（整体覆盖；校验失败返回 Err 不落盘）。
    ///
    /// 更新时保留不该被前端覆盖的字段：创建时间、槽位去重记录、上次执行结果。
    pub fn upsert(&self, t: &mut crate::modem::model::schedule::Task) -> crate::error::Result<()> {
        crate::modem::model::schedule::validate_task(t)?;
        let mut guard = self.inner.lock().unwrap();
        let now = crate::logging::rfc3339(crate::logging::unix_now());
        if let Some(old) = guard.tasks.iter_mut().find(|old| old.id == t.id) {
            t.created_at = old.created_at.clone();
            t.slots = old.slots.clone();
            t.last = old.last.clone();
            t.updated_at = now;
            *old = t.clone();
            guard.dirty = true;
            drop(guard);
            self.flush();
            return Ok(());
        }
        if guard.tasks.len() >= crate::modem::model::schedule::MAX_TASKS {
            return Err(crate::error::msg(format!(
                "任务数已达上限（{} 条）",
                crate::modem::model::schedule::MAX_TASKS
            )));
        }
        if t.created_at.is_empty() {
            t.created_at = now.clone();
        }
        t.updated_at = now;
        guard.tasks.push(t.clone());
        guard.dirty = true;
        drop(guard);
        self.flush();
        Ok(())
    }

    /// 删除任务（返回是否存在）。
    pub fn delete(&self, id: &str) -> bool {
        let found = {
            let mut guard = self.inner.lock().unwrap();
            let before = guard.tasks.len();
            guard.tasks.retain(|t| t.id != id);
            let found = guard.tasks.len() != before;
            guard.dirty |= found;
            found
        };
        if found {
            self.flush();
        }
        found
    }

    /// 改任务的模组键（换 USB 口后按唯一 IMEI 迁移用）；顺带落盘。
    pub fn retarget(&self, id: &str, key: &str) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(t) = guard.tasks.iter_mut().find(|t| t.id == id) {
            t.modem_key = key.to_string();
            t.updated_at = crate::logging::rfc3339(crate::logging::unix_now());
            guard.dirty = true;
            drop(guard);
            self.flush();
        }
    }

    /// 记录一次执行结果（并推进槽位去重标记；`manual` 不推进 —— 手动执行不改时间表）。
    pub fn record(&self, id: &str, run: crate::modem::model::schedule::Run) {
        let should_save = {
            let mut guard = self.inner.lock().unwrap();
            let Some(t) = guard.tasks.iter_mut().find(|t| t.id == id) else {
                return;
            };
            if run.slot != crate::modem::model::schedule::PHASE_MANUAL {
                t.slots.insert(run.phase.clone(), run.slot.clone());
            }
            t.last = Some(run);
            guard.dirty = true;
            // 落盘节流：SCHED_SAVE_EVERY 秒内只记内存（进程退出 flush 兜底）
            let now = crate::logging::unix_now();
            if now - guard.saved_at >= crate::modem::model::schedule::SCHED_SAVE_EVERY {
                guard.saved_at = now;
                true
            } else {
                false
            }
        };
        if should_save {
            self.flush();
        }
    }

    /// 日志节流（同一任务 [`LOG_EVERY`] 内只记一条；返回是否允许记）。
    pub fn log_allowed(&self, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap();
        let now = crate::logging::unix_now();
        if let Some(&at) = guard.last_log.get(id) {
            if now - at < crate::modem::model::schedule::LOG_EVERY {
                return false;
            }
        }
        guard.last_log.insert(id.to_string(), now);
        true
    }

    /// 强制落盘（进程退出前 / 状态变化后调用）。
    ///
    /// 锁内写盘。写失败不丢脏标记，
    /// 下一次 flush 会重试（宁可可重入，也别静默丢账）。
    pub fn flush(&self) {
        let mut guard = self.inner.lock().unwrap();
        if !guard.dirty {
            return;
        }
        if self.path.trim().is_empty() {
            return;
        }
        let payload = SchedFile {
            version: 1,
            saved_at: crate::logging::timestamp(),
            tasks: guard.tasks.clone(),
        };
        let Ok(text) = serde_json::to_string_pretty(&payload) else {
            log::warn!("序列化定时任务台账失败");
            return;
        };
        // 原子写：rename 没成才保留脏标记，下一次 flush 重试
        if write_json_atomic(&self.path, &text) {
            guard.dirty = false;
            guard.saved_at = crate::logging::unix_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        at_port_bind_key, at_port_bind_key_scoped, atport_bind_file, atport_file, imei_file,
        quota_file, schedule_file, AtPortBindStore, AtPortStore, ImeiBackup, ImeiStore,
        QuotaRecord, QuotaStore, SchedStore,
    };
    use crate::modem::model::schedule::{Action, Run, Task, KIND_BAND_LOCK};

    #[test]
    fn ledger_files_sit_next_to_config() {
        // 用 Path::join 造期望值，避免 Windows 分隔符差异
        let dir = std::path::Path::new("/etc/modemd");
        let cfg = dir.join("config.toml").to_string_lossy().to_string();
        assert_eq!(quota_file(&cfg), dir.join("quota.json").to_string_lossy());
        assert_eq!(imei_file(&cfg), dir.join("imei.json").to_string_lossy());
        assert_eq!(atport_file(&cfg), dir.join("atport.json").to_string_lossy());
        assert_eq!(
            atport_bind_file(&cfg),
            dir.join("atport_bind.json").to_string_lossy()
        );
        assert_eq!(quota_file("config.toml"), "quota.json");
    }

    #[test]
    fn at_port_bind_key_normalizes() {
        assert_eq!(
            at_port_bind_key("0E8D", "7126").as_deref(),
            Some("0e8d:7126")
        );
        assert_eq!(
            at_port_bind_key("0x2c7c", "0x0800").as_deref(),
            Some("2c7c:0800")
        );
        // 只给 "vid:pid"
        assert_eq!(
            at_port_bind_key("2C7C:0900", "").as_deref(),
            Some("2c7c:0900")
        );
        // 非法（长度不是 4 / 非十六进制）
        assert!(at_port_bind_key("abc", "7126").is_none());
        assert!(at_port_bind_key("0e8d1", "7126").is_none());
        assert!(at_port_bind_key("zzzz", "7126").is_none());
    }

    #[test]
    fn scoped_bind_key_separates_identical_models() {
        assert_eq!(
            at_port_bind_key_scoped("2c7c", "0801", "1-1").as_deref(),
            Some("2c7c:0801@1-1")
        );
        assert_eq!(
            at_port_bind_key_scoped("2c7c", "0801", "").as_deref(),
            Some("2c7c:0801"),
            "没给路径时退回老键"
        );
        assert!(
            at_port_bind_key_scoped("2c7c", "0801", "../evil").is_none(),
            "路径里有穿越字符必须拒绝"
        );
        assert!(at_port_bind_key_scoped("bad", "0801", "1-1").is_none());
    }

    #[test]
    fn at_port_bind_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("modemd-atbind-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("atport_bind.json").to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let store = AtPortBindStore::new(path.clone());
        assert!(store.get("0e8d:7126").is_empty());
        assert!(store.set("0e8d:7126", "/dev/ttyUSB2"));
        // 同值不算变化
        assert!(!store.set("0e8d:7126", "/dev/ttyUSB2"));

        let reloaded = AtPortBindStore::new(path.clone());
        assert_eq!(reloaded.get("0e8d:7126"), "/dev/ttyUSB2");
        assert_eq!(reloaded.list().len(), 1);

        // 解绑（空串）清掉记录
        assert!(reloaded.set("0e8d:7126", ""));
        assert!(AtPortBindStore::new(path.clone())
            .get("0e8d:7126")
            .is_empty());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn imei_backup_roundtrip() {
        let dir = std::env::temp_dir().join(format!("modemd-imei-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("imei.json").to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let store = ImeiStore::new(path.clone());
        assert!(store.get(0).is_none());
        store.put(
            1,
            ImeiBackup {
                original: "867698040012349".to_string(),
                saved_at: "2026-10-18T00:00:00Z".to_string(),
                source: "AT+EGMREXT=0,7".to_string(),
                writes: 2,
                restored_at: None,
            },
        );
        let got = ImeiStore::new(path.clone()).get(1).expect("应读回备份");
        assert_eq!(got.original, "867698040012349");
        assert_eq!(got.source, "AT+EGMREXT=0,7");
        assert_eq!(got.writes, 2);

        // 旧版 imei.json 没有 source 字段：按空串读入，不让老台账失效
        std::fs::write(
            &path,
            r#"{"modems":{"2":{"original":"867698040012349","saved_at":"2026-10-01T00:00:00Z","writes":1}}}"#,
        )
        .expect("写旧版台账");
        let legacy = ImeiStore::new(path.clone()).get(2).expect("旧台账应能读");
        assert_eq!(legacy.original, "867698040012349");
        assert!(legacy.source.is_empty());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_through_disk() {
        let dir = std::env::temp_dir().join(format!("modemd-quota-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("quota.json").to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let store = QuotaStore::new(path.clone());
        // 空台账：读不到就给默认值
        assert!(store.get(0).settings.is_none());

        store.put(
            2,
            QuotaRecord {
                used_up: 1234,
                used_down: 5678,
                period_start: 1_700_000_000,
                blocked: true,
                ..Default::default()
            },
        );

        let reloaded = QuotaStore::new(path.clone());
        let got = reloaded.get(2);
        assert_eq!((got.used_up, got.used_down), (1234, 5678));
        assert_eq!(got.period_start, 1_700_000_000);
        assert!(got.blocked);
        // 另一个模组不受影响
        assert_eq!(reloaded.get(3).used_up, 0);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_file_does_not_panic() {
        let dir = std::env::temp_dir().join(format!("modemd-quota-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("quota.json").to_string_lossy().to_string();
        let _ = std::fs::write(&path, "{ 这不是 json");
        let store = QuotaStore::new(path.clone());
        assert!(store.get(0).settings.is_none());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atport_roundtrip_and_clear() {
        let dir = std::env::temp_dir().join(format!("modemd-atport-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("atport.json").to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let store = AtPortStore::new(path.clone());
        assert_eq!(store.get("imei:123"), "");
        // 空键不记（宁可不记，也别把两台模组串在一起）
        assert!(!store.set("  ", "/dev/ttyUSB9"));
        assert!(store.set("imei:123", "/dev/ttyUSB2"));
        assert!(store.get("imei:123") == "/dev/ttyUSB2");
        // 原样再设一次不算改动
        assert!(!store.set("imei:123", "/dev/ttyUSB2"));
        // 空串 = 恢复自动探测
        assert!(store.set("imei:123", ""));
        assert_eq!(store.get("imei:123"), "");

        // 跨重启读回
        assert!(store.set("imei:123", "/dev/ttyUSB2"));
        let reloaded = AtPortStore::new(path.clone());
        assert_eq!(reloaded.get("imei:123"), "/dev/ttyUSB2");
        // 按端口反查清记录
        assert_eq!(reloaded.clear_by_port("/dev/ttyUSB2"), 1);
        assert_eq!(reloaded.clear_by_port("/dev/ttyUSB2"), 0);
        assert_eq!(AtPortStore::new(path.clone()).get("imei:123"), "");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn task(id: &str) -> Task {
        Task {
            id: id.into(),
            modem_key: "port:/dev/ttyUSB0".into(),
            enabled: true,
            start_time: "08:00".into(),
            start_actions: vec![Action {
                kind: KIND_BAND_LOCK.into(),
                tech: "NR".into(),
                bands: "78".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn schedule_file_sits_next_to_config() {
        let dir = std::path::Path::new("/etc/modemd");
        let cfg = dir.join("config.toml").to_string_lossy().to_string();
        assert_eq!(
            schedule_file(&cfg),
            dir.join("schedule.json").to_string_lossy()
        );
        assert_eq!(schedule_file(""), "schedule.json");
    }

    #[test]
    fn sched_store_upsert_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!("modemd-sched-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("schedule.json").to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        let store = SchedStore::new(path.clone());

        // 新增：created_at / updated_at 自动补
        let mut t = task("t1");
        assert!(store.upsert(&mut t).is_ok());
        assert!(!t.created_at.is_empty());
        assert_eq!(store.snapshot().len(), 1);

        // 先记录一次执行（推进槽位与上次结果），再 upsert 编辑版任务：
        // created_at / slots / last 必须保留，不能被前端提交体覆盖
        let mut t2 = task("t1");
        t2.name = "改过".into();
        store.record(
            "t1",
            Run {
                phase: "start".into(),
                slot: "2026-09-30 08:00".into(),
                at: String::new(),
                ok: false,
                msg: "失败".into(),
            },
        );
        assert!(store.upsert(&mut t2).is_ok());
        let snap = store.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0].slots.get("start").map(String::as_str),
            Some("2026-09-30 08:00")
        );
        assert!(!snap[0].last.as_ref().unwrap().ok);
        assert_eq!(snap[0].created_at, t.created_at);
        assert_eq!(snap[0].name, "改过");
        // 校验失败不写入
        let mut bad = task("t2");
        bad.start_time = "99:99".into();
        assert!(store.upsert(&mut bad).is_err());
        assert_eq!(store.snapshot().len(), 1);

        // 跨重启读回，再删除
        let reloaded = SchedStore::new(path.clone());
        assert_eq!(reloaded.snapshot().len(), 1);
        assert_eq!(reloaded.snapshot()[0].id, "t1");
        assert!(reloaded.delete("t1"));
        assert!(!reloaded.delete("t1"));
        assert_eq!(reloaded.snapshot().len(), 0);

        // 坏台账：无 id 的跳过、校验不过的跳过（宁可跳过也不带病运行）
        std::fs::write(&path, r#"{"version":1,"tasks":[{"id":"","name":"无 id"},{"id":"t3","modem_key":"port:x","enabled":true,"start_time":"not-a-time","start_actions":[]}]}"#).unwrap();
        let reloaded = SchedStore::new(path.clone());
        assert_eq!(reloaded.snapshot().len(), 0);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sched_store_record_retarget_log() {
        let dir = std::env::temp_dir().join(format!("modemd-sched2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("schedule.json").to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        let store = SchedStore::new(path.clone());
        let mut t = task("t1");
        store.upsert(&mut t).unwrap();

        // manual 不推进槽位
        store.record(
            "t1",
            Run {
                phase: "manual".into(),
                slot: "manual".into(),
                at: "x".into(),
                ok: true,
                msg: "已执行".into(),
            },
        );
        assert!(store.snapshot()[0].slots.is_empty());
        // 非 manual 推进槽位并记结果
        store.record(
            "t1",
            Run {
                phase: "start".into(),
                slot: "2026-09-30 08:00".into(),
                at: "x".into(),
                ok: false,
                msg: "失败".into(),
            },
        );
        let snap = store.snapshot();
        assert_eq!(
            snap[0].slots.get("start").map(String::as_str),
            Some("2026-09-30 08:00")
        );
        assert!(!snap[0].last.as_ref().unwrap().ok);

        // 迁移目标键
        store.retarget("t1", "port:COM32");
        assert_eq!(store.snapshot()[0].modem_key, "port:COM32");
        store.retarget("no-such", "port:x"); // 不存在的任务安静忽略

        // 日志节流：窗口内第二次被拦
        assert!(store.log_allowed("t1"));
        assert!(!store.log_allowed("t1"));
        assert!(store.log_allowed("t2"));

        // record 后（节流窗口外首次）落盘可读回
        let reloaded = SchedStore::new(path.clone());
        assert_eq!(reloaded.snapshot()[0].modem_key, "port:COM32");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
