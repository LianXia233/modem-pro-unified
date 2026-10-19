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
        for path in discover_new(&known, baud, self.cfg.serial.probe_timeout()) {
            let id = self.alloc_id();
            let modem = Arc::new(Modem::new(
                id,
                path.clone(),
                baud,
                self.cfg.serial.command_timeout(),
                Vendor::Unknown,
            ));
            modem.set_sms_memory_pref(&self.cfg.sms.sms_memory);
            let _ = modem.identify();
            self.apply_saved_at_port(&modem);
            log::info!(
                "发现模组 id={id} at={} vendor={}",
                modem.port(),
                modem.vendor().as_str()
            );
            self.modems.write().unwrap().insert(id, modem);
            added += 1;
        }
        added
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
        // ① 按 USB 设备身份（vid:pid）绑定的命令口优先
        if let Some(desc) = crate::modem::model::dial::usb_descriptor_for_port(&modem.port()) {
            if let Some(key) = crate::runtime::store::at_port_bind_key(&desc.vid, &desc.pid) {
                let bound = self.atport_bind.get(&key);
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
