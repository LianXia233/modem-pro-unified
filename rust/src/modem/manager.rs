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
}

impl ModemManager {
    pub fn new(cfg: Config, atport: Arc<AtPortStore>, atport_bind: Arc<AtPortBindStore>) -> Self {
        Self {
            cfg,
            atport,
            atport_bind,
            modems: RwLock::new(BTreeMap::new()),
            next_id: RwLock::new(0),
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
        let baud = self.cfg.serial.baud;
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
