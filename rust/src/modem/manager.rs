//! 模组管理器：发现 / 列表 / 查询。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use crate::config::Config;
use crate::runtime::store::{AtPortBindStore, AtPortStore};

use super::discovery::discover_new;
use super::model::{Modem, ModemView};
use super::vendor::Vendor;

pub struct ModemManager {
    cfg: Config,
    /// 手动指定的 AT 命令口（atport.json；见 runtime/store.rs）
    atport: Arc<AtPortStore>,
    /// 按 USB 设备身份（vid:pid）指定的命令口（atport_bind.json；优先于 atport）
    atport_bind: Arc<AtPortBindStore>,
    modems: RwLock<BTreeMap<u32, Arc<Modem>>>,
    next_id: RwLock<u32>,
    /// 本轮巡检回收掉的模组 id（等 `http::serve` 侧的循环取走清缓存）。
    reaped: std::sync::Mutex<Vec<u32>>,
    /// 发现流程的互斥锁：**一次只允许一个 discover 在跑**。
    ///
    /// 存在的理由：启动发现（后台线程）、周期巡检、页面的
    /// `GET /api/modems?discover=1` 三方可能同时进来；而 `discover` 内部是
    /// 「读一次 known 快照 → 逐个探测 → 插入」的非原子流程，两个线程会各自
    /// 看到对方还没插入的口，于是**同一台设备被登记两次**（id 不同、抢同一个
    /// tty）—— 表现为列表里冒出幽灵模组、AT 应答互相串扰。
    discovering: std::sync::Mutex<()>,
    /// 补 option 驱动尝试记录（`bus_path` → 上次尝试时刻）。
    ///
    /// 有些设备（例如只有 MBIM 形态、`option_driver=1` 但固件不出 AT 口）
    /// 补绑后依然没有 tty。不节流的话每个巡检周期都会写一次 `new_id` 并刷日志；
    /// 这里按 5 分钟冷却，仍然不会永久放弃（固件升级/换模式后还能再试）。
    bind_tried: std::sync::Mutex<BTreeMap<String, std::time::Instant>>,
    /// 「一台模组都没有」从什么时候开始（有模组就清空）。
    no_at_since: std::sync::Mutex<Option<std::time::Instant>>,
    /// 上次救援时刻 / 本次故障期内的救援次数（见到模组即清零）。
    last_rescue: std::sync::Mutex<Option<std::time::Instant>>,
    rescue_count: std::sync::Mutex<u32>,
}

/// 「一台都认不出」时的救援判定（纯函数，便于测试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rescue {
    /// 还在等（<150s）
    Wait,
    /// 开关没开
    Disabled,
    /// 距上次救援不足 5 分钟
    Cooldown,
    /// 本轮故障已救援 3 次
    Budget,
    /// 该动手了
    Go,
}

/// 上游 Go 版参数：150s 无 AT 才动手、冷却 5 分钟、一个周期最多 3 次
/// （`module/kernel/modem_serial.go:352-428`）。
pub(crate) fn rescue_decision(
    enabled: bool,
    idle: std::time::Duration,
    since_last: Option<std::time::Duration>,
    count: u32,
) -> Rescue {
    if idle < std::time::Duration::from_secs(150) {
        return Rescue::Wait;
    }
    if !enabled {
        return Rescue::Disabled;
    }
    if count >= 3 {
        return Rescue::Budget;
    }
    if since_last.is_some_and(|d| d < std::time::Duration::from_secs(300)) {
        return Rescue::Cooldown;
    }
    Rescue::Go
}

impl ModemManager {
    pub fn new(cfg: Config, atport: Arc<AtPortStore>, atport_bind: Arc<AtPortBindStore>) -> Self {
        Self {
            cfg,
            atport,
            atport_bind,
            modems: RwLock::new(BTreeMap::new()),
            next_id: RwLock::new(0),
            reaped: std::sync::Mutex::new(Vec::new()),
            discovering: std::sync::Mutex::new(()),
            bind_tried: std::sync::Mutex::new(BTreeMap::new()),
            no_at_since: std::sync::Mutex::new(None),
            last_rescue: std::sync::Mutex::new(None),
            rescue_count: std::sync::Mutex::new(0),
        }
    }

    /// 手动指定台账（`/api/at/port` 与发现流程共用）。
    pub fn atport(&self) -> &Arc<AtPortStore> {
        &self.atport
    }

    /// 按 USB 设备身份绑定的台账（`/api/atport/bind` 与发现流程共用）。
    pub fn atport_bind(&self) -> &Arc<AtPortBindStore> {
        &self.atport_bind
    }

    /// 扫描串口并把新出现的 AT 口登记成模组。
    ///
    /// `force` 只影响「是否重探已知口」；骨架阶段不维护否定缓存（迁移时按
    /// `probe_warmup` / 指数退避补上，见 docs/INTEGRATION.md）。
    ///
    /// 手动指定（`/api/at/port` 存的那份）在这里生效：新模组识别出身份后，
    /// 若台账里有它的命令口且与探到的不同，整口换过去（并回收被换口上的
    /// 「幻影模组」—— 简化模型下同一物理设备的多个 tty 会各自成一个模组）。
    pub fn discover(&self, _force: bool) -> usize {
        // 串行化：见 `discovering` 字段的说明。持锁期间不要再调 `reap_gone`
        // 之外会重入的公开方法。
        let _serial = self.discovering.lock().unwrap_or_else(|e| e.into_inner());
        let baud = self.cfg.serial.baud;
        // ⓪ 先补 option 驱动：没 tty 的设备到不了后面任何一步
        let bound = self.auto_bind_option() + self.auto_bind_pcie();
        if bound > 0 {
            log::info!("本轮补绑 {bound} 台设备的驱动，重新枚举串口");
        }
        // ① 先回收消失的模组，**再**统计已知口。
        //
        // 顺序要紧：`known` 里要是还留着已拔掉设备的口，
        // `discover_new` 就会一直跳过它 —— 同一台设备重插后再也发现不了。
        self.reap_gone();
        let known: BTreeSet<String> = self
            .modems
            .read()
            .unwrap()
            .values()
            .map(|m| m.port())
            .collect();

        let mut added = 0;
        for d in discover_new(&known, baud, self.cfg.serial.probe_timeout()) {
            let id = self.alloc_id();
            let modem = Arc::new(Modem::new(
                id,
                d.path.clone(),
                baud,
                self.cfg.serial.command_timeout(),
                Vendor::Unknown,
            ));
            modem.set_sms_memory_pref(&self.cfg.sms.sms_memory);
            modem.set_ttys(d.ttys.clone());
            let _ = modem.identify();
            self.apply_saved_at_port(&modem);
            log::info!(
                "发现模组 id={id} at={} vendor={} group={} fallback={} 规则={} 证据={}",
                d.path,
                modem.vendor().as_str(),
                d.group,
                d.fallback_index,
                if d.rule_note.is_empty() {
                    "未命中"
                } else {
                    &d.rule_note
                },
                d.evidence.join(" | ")
            );
            for (port, why) in &d.skipped {
                log::debug!("发现流程跳过 {port}：{why}");
            }
            self.modems.write().unwrap().insert(id, modem);
            added += 1;
        }
        // 收尾再判断要不要救援：让本轮新发现的设备先落库，避免刚救完又救
        self.maybe_rescue();
        added
    }

    /// 发现前先把「端口规则要求 option 驱动、但还没出 tty」的设备补绑上。
    ///
    /// 顺序要紧：枚举串口前不补绑，这类设备永远看不到 tty，也就永远发现不了
    /// （QModem `modem_scand.c:570` 同样在枚举接口之前写 `option1/new_id`）。
    /// 受 `[serial] usb_autobind` 控制；失败只记日志，不影响其他设备。
    fn auto_bind_option(&self) -> usize {
        if !self.cfg.serial.usb_autobind {
            return 0;
        }
        const BIND_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(300);
        let mut n = 0;
        for dev in crate::modem::usbdev::option_bind_candidates() {
            {
                let mut tried = self.bind_tried.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(last) = tried.get(&dev.bus_path) {
                    if last.elapsed() < BIND_COOLDOWN {
                        continue;
                    }
                }
                tried.insert(dev.bus_path.clone(), std::time::Instant::now());
            }
            match crate::modem::usbdev::ensure_serial_tty(&dev) {
                Ok(note) => {
                    log::info!(
                        "已补绑 option 驱动：{}:{}（{}）→ {note}",
                        dev.vid,
                        dev.pid,
                        dev.bus_path
                    );
                    n += 1;
                }
                Err(e) => log::warn!(
                    "补绑 option 驱动失败 {}:{}（{}）：{e}",
                    dev.vid,
                    dev.pid,
                    dev.bus_path
                ),
            }
        }
        n
    }

    /// PCIe 模组没绑驱动时，按已知驱动表试一遍（MHI / T7xx）。
    ///
    /// 与 USB 补绑同一理由：没驱动就没有 AT 口，也就永远发现不了。
    /// 绑定失败只记日志（驱动没装、设备不支持都可能）。
    fn auto_bind_pcie(&self) -> usize {
        if !self.cfg.serial.pcie_autobind {
            return 0;
        }
        let mut n = 0;
        for dev in crate::modem::pcidev::list_pci_modem_devices() {
            if dev.bound || !dev.modem {
                continue;
            }
            let key = format!("pci:{}", dev.bdf);
            {
                let mut tried = self.bind_tried.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(last) = tried.get(&key) {
                    if last.elapsed() < std::time::Duration::from_secs(300) {
                        continue;
                    }
                }
                tried.insert(key, std::time::Instant::now());
            }
            for drv in crate::modem::pcidev::PCIE_MODEM_DRIVERS {
                if crate::modem::pcidev::bind_pci_driver(&dev.bdf, drv).is_ok() {
                    log::info!("PCIe 模组 {} 已绑定驱动 {drv}", dev.bdf);
                    n += 1;
                    break;
                }
            }
        }
        n
    }

    /// 一台模组都认不出时，按上游参数做一次「软重插」。
    ///
    /// 只在 `[serial] auto_rescue=true` 时动手；默认关（会短暂断开数据面）。
    fn maybe_rescue(&self) {
        let has_modem = !self.modems.read().unwrap().is_empty();
        if has_modem {
            *self.no_at_since.lock().unwrap() = None;
            *self.rescue_count.lock().unwrap() = 0;
            return;
        }
        let idle = {
            let mut since = self.no_at_since.lock().unwrap_or_else(|e| e.into_inner());
            let start = *since.get_or_insert_with(std::time::Instant::now);
            start.elapsed()
        };
        let since_last = self
            .last_rescue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|t| t.elapsed());
        let count = *self.rescue_count.lock().unwrap_or_else(|e| e.into_inner());
        let decision = rescue_decision(self.cfg.serial.auto_rescue, idle, since_last, count);
        if decision != Rescue::Go {
            if decision == Rescue::Disabled && idle.as_secs() >= 150 {
                log::warn!(
                    "已 {idle:?} 一台模组都没认出来；需要自动软重插请打开 [serial] auto_rescue"
                );
            }
            return;
        }
        {
            let mut c = self.rescue_count.lock().unwrap_or_else(|e| e.into_inner());
            *c += 1;
            log::warn!(
                "无模组已 {idle:?}，执行第 {} 次软重插（USB authorized / PCIe 重扫）",
                *c
            );
        }
        *self.last_rescue.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(std::time::Instant::now());

        for dev in crate::modem::usbdev::list_modem_usb_devices() {
            if !dev.ttys.is_empty() {
                continue;
            }
            match crate::modem::usbdev::reauthorize(&dev.bus_path) {
                Ok(()) => log::info!("USB 软重插 {}（{}:{}）", dev.bus_path, dev.vid, dev.pid),
                Err(e) => log::warn!("USB 软重插 {} 失败：{e}", dev.bus_path),
            }
        }
        for dev in crate::modem::pcidev::list_pci_modem_devices() {
            if dev.bound || !dev.modem {
                continue;
            }
            match crate::modem::pcidev::reset_pci_device(&dev.bdf) {
                Ok(()) => log::info!("PCIe 重扫完成（{}）", dev.bdf),
                Err(e) => log::warn!("PCIe 重扫失败（{}）：{e}", dev.bdf),
            }
        }
    }

    /// 回收「设备已经不在了」的模组，返回被摘掉的 id 列表。
    ///
    /// ## 为什么必须有这一步
    ///
    /// 早先 `discover()` 只增不删，于是拔卡 / 拔模组之后：
    ///
    /// - 它仍留在列表里，`is_running()` 恒为 true，后台每个循环都要对它发一次 AT
    ///   （每次都超时，白等一个 `command_timeout`）；
    /// - 它的串口句柄从不释放（fd 泄漏，反复热插拔会攒到 ulimit）；
    /// - 最要命的是：它的口一直在 `known` 集合中，**同一台设备重插后再也发现不了**
    ///   —— 用户看到的就是「拔了再插，模组不见了」，只能重启 modemd。
    ///
    /// ## 判据为什么只看「设备文件在不在」
    ///
    /// 用「打不开串口」当判据会误杀：设备正忙（另一个进程占着）、权限临时受限、
    /// 模组正在重启，都会让 `open` 失败，但设备其实还在 —— 那样会把一台好模组
    /// 反复删掉再重建，界面上 id 一直跳。只看设备文件是否存在则没有这个歧义：
    /// 拔掉了文件才消失。
    pub fn reap_gone(&self) -> Vec<u32> {
        let gone: Vec<u32> = {
            let g = self.modems.read().unwrap();
            g.values()
                .filter(|m| !super::discovery::port_exists(&m.port()))
                .map(|m| m.id)
                .collect()
        };
        if gone.is_empty() {
            return gone;
        }
        let mut removed = Vec::new();
        {
            let mut w = self.modems.write().unwrap();
            for id in &gone {
                if let Some(m) = w.remove(id) {
                    // `Arc<Modem>` 的引用计数归零后，里面的 `AtPort` 被 drop，
                    // serialport 会替我们关掉 fd —— 不需要手动 close。
                    log::info!(
                        "模组 id={id}（at={}）的设备已消失，已回收（串口随对象释放）",
                        m.port()
                    );
                    removed.push(*id);
                }
            }
        }
        // 攒起来等调用方取走（清缓存）：`http::serve` 那侧的巡检循环会定期拿
        self.reaped.lock().unwrap().extend(removed.iter().copied());
        removed
    }

    /// 取走「本轮巡检回收掉的模组 id」（调用方据此清 `StateCache`）。
    ///
    /// 取走即清，所以同一批 id 只会被清理一次。
    pub fn take_reaped(&self) -> Vec<u32> {
        std::mem::take(&mut *self.reaped.lock().unwrap())
    }

    /// 按台账里这台模组的手动指定切口（没有指定 / 指定为空 = 自动探测，不动）。
    ///
    /// 优先「按 USB 设备身份（vid:pid）」的绑定（`atport_bind.json`）：它不认识模组也能用，
    /// 正是自动探测挑错口、身份读不出来时的补救路径；其次才是「按模组身份」的指定（`atport.json`）。
    ///
    /// 简化模型说明：一个 AT 口 = 一个模组，没有 USB 设备分组信息，所以身份键取不到时
    /// 用「当前口自己」当兜底键（不做串口分组）；切口成功后清掉兜底键，免得两份记录打架。
    pub(crate) fn apply_saved_at_port(&self, modem: &Arc<Modem>) {
        // ① 按 USB 设备身份（vid:pid，可带物理路径）绑定的命令口优先
        if let Some(desc) = crate::modem::model::dial::usb_descriptor_for_port(&modem.port()) {
            if let Some(key) = crate::runtime::store::at_port_bind_key(&desc.vid, &desc.pid) {
                // 先查「vid:pid@总线路径」：同型号多台设备时，只有它能区分
                let bus = crate::modem::usbdev::identity_for_tty(&modem.port())
                    .map(|u| u.bus_path)
                    .unwrap_or_default();
                let scoped =
                    crate::runtime::store::at_port_bind_key_scoped(&desc.vid, &desc.pid, &bus);
                let mut bound = scoped
                    .as_deref()
                    .map(|k| self.atport_bind.get(k))
                    .unwrap_or_default();
                if bound.is_empty() {
                    bound = self.atport_bind.get(&key);
                }
                if !bound.is_empty() {
                    if bound == modem.port() {
                        modem.set_at_port_manual(true);
                        return;
                    }
                    match modem.repoint(&bound) {
                        Ok(()) => {
                            modem.set_at_port_manual(true);
                            self.reclaim_ghosts(modem.id, &bound);
                            return;
                        }
                        Err(e) => log::warn!(
                            "模组 id={} 按 VID:PID 绑定（{key} → {bound}）切口失败: {e}",
                            modem.id
                        ),
                    }
                }
            }
        }

        // ② 按模组身份的手动指定
        let identity_key = modem.at_port_key();
        let fallback_key = format!("ports:{}", modem.port());
        let key = identity_key.clone().unwrap_or_else(|| fallback_key.clone());
        let saved = self.atport.get(&key);
        if saved.is_empty() {
            // 没有手动指定：确认这不是「别人手动指定的目标口」
            return;
        }
        if saved == modem.port() {
            modem.set_at_port_manual(true);
            return;
        }
        if let Err(e) = modem.repoint(&saved) {
            log::warn!(
                "模组 id={} 按手动指定切到 {saved} 失败: {e}（保持 {from}）",
                modem.id,
                from = modem.port()
            );
            return;
        }
        modem.set_at_port_manual(true);
        // 被换到的口上如果已有「幻影模组」（同一物理设备的另一个 tty），回收掉
        self.reclaim_ghosts(modem.id, &saved);
        // 身份读出来了：清掉「未识别时期」用兜底键存的记录
        if let Some(ik) = identity_key {
            if ik != key {
                self.atport.set(&ik, &saved);
                self.atport.set(&fallback_key, "");
            }
        }
    }

    /// 回收占了 `port` 的其它「幻影模组」（同一物理设备的另一个 tty 在简化模型下各自成一个模组）。
    fn reclaim_ghosts(&self, keep_id: u32, port: &str) {
        let mut modems = self.modems.write().unwrap();
        let ghosts: Vec<u32> = modems
            .values()
            .filter(|m| m.id != keep_id && m.port() == port)
            .map(|m| m.id)
            .collect();
        for gid in ghosts {
            if let Some(g) = modems.remove(&gid) {
                log::info!(
                    "回收幻影模组 id={gid} at={}（id={keep_id} 的命令口）",
                    g.port()
                );
            }
        }
    }

    pub fn list(&self) -> Vec<ModemView> {
        self.modems
            .read()
            .unwrap()
            .values()
            .map(|m| m.view())
            .collect()
    }

    pub fn get(&self, id: u32) -> Option<Arc<Modem>> {
        self.modems.read().unwrap().get(&id).cloned()
    }

    /// 全部模组（按 id 升序）。
    pub fn all(&self) -> Vec<Arc<Modem>> {
        self.modems.read().unwrap().values().cloned().collect()
    }

    pub fn count(&self) -> usize {
        self.modems.read().unwrap().len()
    }

    /// 摘掉一台模组（`/api/at/port` 换口时回收幻影模组用）。
    pub fn remove(&self, id: u32) {
        self.modems.write().unwrap().remove(&id);
    }

    fn alloc_id(&self) -> u32 {
        let mut n = self.next_id.write().unwrap();
        let id = *n;
        *n += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::{rescue_decision, Rescue};
    use std::time::Duration;

    fn d(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    /// 150s 之前一律不动手（刚开机/刚插上都不该被打断）。
    #[test]
    fn rescue_waits_before_acting() {
        assert_eq!(rescue_decision(true, d(0), None, 0), Rescue::Wait);
        assert_eq!(rescue_decision(true, d(149), None, 0), Rescue::Wait);
        assert_eq!(rescue_decision(false, d(149), None, 0), Rescue::Wait);
    }

    /// 开关关掉时只提示，不碰设备。
    #[test]
    fn rescue_respects_switch() {
        assert_eq!(rescue_decision(false, d(600), None, 0), Rescue::Disabled);
        assert_eq!(rescue_decision(true, d(150), None, 0), Rescue::Go);
    }

    /// 5 分钟冷却与「本轮最多 3 次」都必须生效，不能无限重插。
    #[test]
    fn rescue_is_rate_limited() {
        assert_eq!(
            rescue_decision(true, d(600), Some(d(10)), 0),
            Rescue::Cooldown
        );
        assert_eq!(
            rescue_decision(true, d(600), Some(d(299)), 0),
            Rescue::Cooldown
        );
        assert_eq!(rescue_decision(true, d(600), Some(d(300)), 0), Rescue::Go);
        assert_eq!(
            rescue_decision(true, d(600), Some(d(9999)), 3),
            Rescue::Budget
        );
    }
}
