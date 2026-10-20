//! 模组领域对象与厂商/型号识别。
//!
//! 简化模型：一个 AT 口 = 一个模组。同一模组的多个 ttyUSB 合并、数据网卡识别等
//! 留待后续按迁移清单补上。

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::Result;

use super::super::at::{
    error_line, extract_field, AtPort, Prio, SchedStats, Scheduler, UrcKind, UrcLine,
};
use super::super::vendor::Vendor;
use super::cells::CellScanResult;
use super::dial::{DialState, DialTypeInfo};
use super::lock::{DialectInfo, LockStatus};
use super::sim::SimIdentity;
use super::sms::{self, ConcatInfo, Sms, SmsStorage};
use super::traffic::{RateTotal, TrafficPrev};
use super::ussd::UssdResult;
use super::view::ModemView;

/// 一个已发现的模组。
pub struct Modem {
    pub id: u32,
    /// AT 口路径（/dev/ttyUSB0 或 COM3）；切口时整口换掉（见 `repoint`），故加锁保护
    port: Mutex<String>,
    /// 这台物理设备上的全部**可作 AT 口**的 tty（首选项在前）。
    ///
    /// 以前 `ModemView::tty_list` 只回当前口自己 —— 页面看不出「同一个模组还有哪些
    /// 口可以切」。发现流程现在按 USB 设备分组（见 `discovery/plan.rs`），
    /// 组内的首选 + 回退口就记在这里；切口（`repoint`）会把它同步成新的顺序。
    ttys: Mutex<Vec<String>>,
    pub baud: u32,
    pub first_seen: Instant,
    /// 单条 AT 命令超时
    pub(super) cmd_timeout: Duration,
    vendor: Mutex<Vendor>,
    model: Mutex<Option<String>>,
    /// 识别时的原始应答（ATI 原文；排查问题用）
    model_raw: Mutex<Option<String>>,
    /// 最近一次识别的完整结论（证据 / 候选 / 冲突 / USB 身份）。
    ///
    /// 与 `vendor` / `model` 分开存：那两个是「结论」，这里是「为什么」。
    /// 页面要显示识别依据、排障要看冲突原因，都得从这里取。
    ident: Mutex<Option<crate::modem::detect::Identify>>,
    /// 这台模组的 AT 命令口是用户手动指定的（界面标「手动」；见 http/at_port.rs）
    at_port_manual: AtomicBool,
    /// 惰性打开的 AT 口（首次查询时打开）
    pub(super) at: Mutex<Option<AtPort>>,
    /// 打开失败的冷却闸（见 `at/cooldown.rs`）。
    ///
    /// 串口打不开往往持续几十秒（模组重启、USB 重新枚举），而 6~8 个后台
    /// 循环会同时来问。没有这层冷却就是「每 200ms 一次 open 全失败」——
    /// 日志刷屏、USB 子系统被拖慢、CPU 空转。
    pub(super) at_cooldown: super::super::at::OpenCooldown,
    /// 串口访问调度（优先级排队 + 限流，见 `at/sched.rs`）。
    ///
    /// **必须先于 `at` 拿**：顺序见 [`Self::with_at_prio`]。
    /// 它是「谁能用串口」的唯一裁决者，`at` 那把锁只负责保护 `Option<AtPort>` 本身。
    pub(super) at_sched: Scheduler,
    /// 最近一次 USSD 应答（`+CUSD` 异步到达，见 model/ussd.rs）
    ussd: Mutex<Option<UssdResult>>,
    /// 最近一次基站扫描缓存（见 model/cells.rs）
    cells: Mutex<Option<CellScanResult>>,
    /// 锁网方言探测缓存（见 model/lock.rs）
    pub(super) lock_dialect_slot: Mutex<Option<DialectInfo>>,
    /// 锁网状态缓存（见 model/lock.rs）
    pub(super) lock_status_slot: Mutex<Option<LockStatus>>,
    /// 最近一次读到的短信与未读统计（见 model/sms.rs）
    sms: Mutex<Option<SmsCache>>,
    /// PDP / 选网参数的 TTL 缓存（15 秒，见 model/params.rs）。
    ///
    /// 进一次设置页要问三条命令，而 APN 只有被人改时才变 —— 冷门查询
    /// 就该缓存，别把串口占满（见 `at/cache.rs` 的三条纪律）。
    pub(super) params_cache: super::super::at::Ttl<super::params::ParamsStatus>,
    /// USB 网卡模式的 TTL 缓存（30 秒，见 model/usbnet.rs）。
    pub(super) usbnet_cache: super::super::at::Ttl<super::usbnet::UsbNetMode>,
    /// 组网模式偏好的 TTL 缓存（30 秒，见 model/mode.rs）。
    pub(super) mode_cache: super::super::at::Ttl<super::mode::ModePref>,
    /// `[sms] sms_memory` 的持久化偏好（空 = 不干预）。
    pub(super) sms_mem_pref: Mutex<String>,
    /// 本会话已经切到的存储区（避免每次读短信都发 `AT+CPMS`）。
    pub(super) sms_mem_active: Mutex<Option<String>>,
    /// 上一次流量采样（速率是差分出来的，见 model/traffic.rs）
    pub(super) traffic_prev: Mutex<Option<TrafficPrev>>,
    /// 速率式模组的**本地积分**累计量（见 model/traffic.rs）。
    ///
    /// 广和通只回瞬时速率（`AT+GTSTATIS`），没有累计计数器；而页面的
    /// 「累计已发送 / 已接收」不能永远是 0 —— 这里按「速率 × 采样间隔」积分。
    pub(super) rate_total: Mutex<Option<RateTotal>>,
    /// 正在进行的主动扫网的取消标志（见 model/netscan.rs）
    pub(super) netscan_stop: Mutex<Option<Arc<AtomicBool>>>,
    /// 「本程序第一次观察到链路在线」的时刻（见 model/dial.rs）。
    ///
    /// **仅作兜底**：正常路径下「连接时间」跟随 netifd 接口 uptime（`ifstatus`，
    /// 见 `dial_status()` 第 ⑤ 步）。只有取不到接口 uptime、且 `connected_at`
    /// 也为空（用户手工搭的 netifd 链路、本程序没拨过号）时，才退回到这个内存
    /// 观测起点 —— 它跨不过 modemd 重启，重启即归零。
    pub(super) link_since: Mutex<Option<Instant>>,
    /// 拨号状态机（见 model/dial.rs；零值 = 没拨过号）
    pub(super) dial_state: Mutex<DialState>,
    /// 拨号流程进行中（拨号要连发几条 AT，不允许并发）
    pub(super) dial_busy: AtomicBool,
    /// USB 类型识别缓存（30 秒 TTL，见 model/dial.rs 的 `usb_type`）
    pub(super) dial_type: Mutex<Option<(DialTypeInfo, Instant)>>,
    /// IMEI 缓存（`imei()` 每次发 AT；定时器目标解析会反复问，见 model/schedule.rs）
    pub(super) imei_cache: Mutex<Option<String>>,
    /// 组网模式指令可用性的 TTL 缓存（30 秒，见 model/schedule.rs 的预检）
    pub(super) mode_supported_slot: Mutex<Option<(bool, Instant)>>,
    /// 关射频能力（AT+CFUN?）的 TTL 缓存（30 秒，见 model/schedule.rs 的预检）
    pub(super) airplane_supported_slot: Mutex<Option<(bool, Instant)>>,
    /// 模组上报的本机 IPv4 缓存（网络查询 / `AT+CGPADDR` 顺手填，见 model/dial.rs）
    pub(super) own_ipv4_cache: Mutex<Option<String>>,
    /// 模组上报的本机 IPv6 缓存（同上；静态 IPv6 模式与组网要用）
    pub(super) own_ipv6_cache: Mutex<Option<String>>,
    /// SIM 身份缓存（在不在 / 状态文本 / ICCID / IMSI，见 model/sim.rs）
    pub(super) sim_ident: Mutex<SimIdentity>,
    /// 是否已经建过基线身份（第一次只建基线、不报事件）
    pub(super) sim_known: std::sync::atomic::AtomicBool,
    /// 收到过与已知状态不同的 `+CPIN:` URC ⇒ 该立刻去核实
    pub(super) sim_dirty: std::sync::atomic::AtomicBool,
    /// 最近一次热插拔事件（removed/inserted/swapped）与发生时刻（unix 秒，0 = 没有过）
    pub(super) sim_last_event: Mutex<String>,
    pub(super) sim_last_event_at: Mutex<i64>,
    /// 最近一次核实身份的时刻（unix 秒，给接口展示；轮询节流另用单调时钟）
    pub(super) sim_checked_at_secs: std::sync::atomic::AtomicI64,
    /// 最近一次核实身份的时刻（单调时钟，轮询节流用）
    pub(super) sim_checked_at: Mutex<Option<Instant>>,
    /// 动作冷却截止时刻（反复插拔不会把模组反复复位）
    pub(super) sim_cooldown_until: Mutex<Option<Instant>>,
    /// 收到过注册态 URC ⇒ 该重查网络状态（`/api/network` 的缓存过期了）
    pub(super) net_dirty: std::sync::atomic::AtomicBool,
}

/// 短信缓存：列表 + 未读数。
struct SmsCache {
    list: Vec<Sms>,
    unread: usize,
}

impl Modem {
    pub fn new(id: u32, port: String, baud: u32, cmd_timeout: Duration, vendor: Vendor) -> Self {
        Self {
            id,
            port: Mutex::new(port.clone()),
            ttys: Mutex::new(vec![port]),
            baud,
            first_seen: Instant::now(),
            cmd_timeout,
            vendor: Mutex::new(vendor),
            model: Mutex::new(None),
            model_raw: Mutex::new(None),
            ident: Mutex::new(None),
            at_port_manual: AtomicBool::new(false),
            at: Mutex::new(None),
            at_cooldown: super::super::at::OpenCooldown::default(),
            at_sched: Scheduler::default(),
            ussd: Mutex::new(None),
            cells: Mutex::new(None),
            lock_dialect_slot: Mutex::new(None),
            lock_status_slot: Mutex::new(None),
            sms: Mutex::new(None),
            params_cache: super::super::at::Ttl::new(Duration::from_secs(15)),
            usbnet_cache: super::super::at::Ttl::new(Duration::from_secs(30)),
            mode_cache: super::super::at::Ttl::new(Duration::from_secs(30)),
            sms_mem_pref: Mutex::new(String::new()),
            sms_mem_active: Mutex::new(None),
            traffic_prev: Mutex::new(None),
            rate_total: Mutex::new(None),
            netscan_stop: Mutex::new(None),
            link_since: Mutex::new(None),
            dial_state: Mutex::new(DialState::default()),
            dial_busy: AtomicBool::new(false),
            dial_type: Mutex::new(None),
            imei_cache: Mutex::new(None),
            mode_supported_slot: Mutex::new(None),
            airplane_supported_slot: Mutex::new(None),
            own_ipv4_cache: Mutex::new(None),
            own_ipv6_cache: Mutex::new(None),
            sim_ident: Mutex::new(SimIdentity::default()),
            sim_known: std::sync::atomic::AtomicBool::new(false),
            sim_dirty: std::sync::atomic::AtomicBool::new(false),
            sim_last_event: Mutex::new(String::new()),
            sim_last_event_at: Mutex::new(0),
            sim_checked_at_secs: std::sync::atomic::AtomicI64::new(0),
            sim_checked_at: Mutex::new(None),
            sim_cooldown_until: Mutex::new(None),
            net_dirty: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 最近一次 USSD 应答。
    pub fn ussd_last(&self) -> Option<UssdResult> {
        self.ussd.lock().unwrap().clone()
    }

    /// 记录一次 USSD 应答。
    pub fn set_ussd(&self, result: UssdResult) {
        *self.ussd.lock().unwrap() = Some(result);
    }

    /// 清掉上一次 USSD 应答（下发前调：免得页面把旧应答当成这次的）。
    pub fn clear_ussd(&self) {
        *self.ussd.lock().unwrap() = None;
    }

    /// 读取基站扫描缓存。
    pub fn cells_cache(&self) -> Option<CellScanResult> {
        self.cells.lock().unwrap().clone()
    }

    /// 写入基站扫描缓存。
    pub fn set_cells_cache(&self, result: CellScanResult) {
        *self.cells.lock().unwrap() = Some(result);
    }

    /// 执行一条 AT 命令并返回原始应答（失败返回空串）。
    ///
    /// **必须显式给优先级**：这里刻意不提供「默认级别」的重载 ——
    /// 新建调用点时不写优先级就编译不过，逼着作者想一秒「这条命令急不急」。
    pub(super) fn at_cmd_prio(&self, prio: Prio, cmd: &str) -> String {
        self.with_at_prio(prio, |p| p.command(cmd, self.cmd_timeout))
            .unwrap_or_default()
    }

    /// 执行一条可被叫停的 AT 命令（用于主动扫网这类长时间占用的操作）。
    ///
    /// 返回 `(应答, 是否被叫停)`；被叫停时应答可能是半截的，由调用方判断能不能用。
    pub(super) fn at_cmd_cancel(
        &self,
        prio: Prio,
        cmd: &str,
        timeout: Duration,
        cancel: &dyn Fn() -> bool,
    ) -> (String, bool) {
        self.with_at_prio(prio, |p| p.command_cancellable(cmd, timeout, cancel))
            .unwrap_or_else(|_| (String::new(), false))
    }

    /// 读取全部短信，按到达时间倒序返回，并刷新缓存。
    ///
    /// 常规固件走文本模式：正文由模组给，但文本模式不暴露 UDH，故再切一次 PDU 模式
    /// 只为取分条拼接信息（失败不影响正文）。机型特例走 PDU 模式（见 [`Self::sms_reads_via_pdu`]）。
    /// 模组回 `ERROR` 时按错误处理，不能当成"卡里没短信"。
    pub fn read_sms(&self) -> Result<Vec<Sms>> {
        // 启动后第一次读短信时把 [sms] sms_memory 应用到模组（失败只 warn，沿用当前区）
        self.apply_sms_memory_pref();
        let list = if self.sms_reads_via_pdu() {
            self.read_sms_via_pdu()?
        } else {
            self.read_sms_text()?
        };
        let list = sms::sort_newest_first(list);
        *self.sms.lock().unwrap() = Some(SmsCache {
            unread: sms::count_unread(&list),
            list: list.clone(),
        });
        Ok(list)
    }

    /// 走文本模式读短信（另切一次 PDU 模式补拼接信息）。
    fn read_sms_text(&self) -> Result<Vec<Sms>> {
        let (resp, concat) = self.with_at_prio(Prio::Bulk, |p| {
            let t = self.cmd_timeout;
            // 模组默认可能是 PDU 模式，此时 AT+CMGL="ALL" 会直接报错
            p.command("AT+CMGF=1", t)?;
            let resp = p.command("AT+CMGL=\"ALL\"", t)?;
            let concat = self.read_concat_info(p);
            Ok((resp, concat))
        })?;
        if let Some(e) = error_line(&resp) {
            return Err(crate::error::msg(format!("读取短信失败: {e}")));
        }
        Ok(sms::group_sms_concat(sms::parse_sms_list(&resp), &concat))
    }

    /// 走 PDU 模式读短信：正文/号码/时间/分条全从 PDU 解出，不必再单独取拼接信息。
    ///
    /// 无论成功与否都把模式切回文本模式 —— 发短信依赖文本模式。
    fn read_sms_via_pdu(&self) -> Result<Vec<Sms>> {
        let (list, concat) = self.with_at_prio(Prio::Bulk, |p| {
            let t = self.cmd_timeout;
            p.command("AT+CMGF=0", t)?;
            let resp = p.command("AT+CMGL=4", t);
            let _ = p.command("AT+CMGF=1", t);
            let resp = resp?;
            // 一条短信都没有时 `AT+CMGL=4` 也是 `OK`：空列表不是错误，
            // 只有固件明确回 ERROR 才算读失败（以前的写法把所有空列表都当错误，
            // 页面因此显示「读取失败」，而不是「无短信」）。
            let ok = super::super::at::is_ok(&resp);
            if !ok && sms::parse_pdu_list(&resp).0.is_empty() {
                return Err(crate::error::msg(format!(
                    "PDU 模式读短信失败：{}",
                    resp.trim()
                )));
            }
            Ok(sms::parse_pdu_list(&resp))
        })?;
        Ok(sms::group_sms_concat(list, &concat))
    }

    /// 是否要走 PDU 模式读短信（机型特例：文本模式回的"正文"其实是十六进制 PDU，
    /// 按 UCS2 文本解会把 UDH 解成怪字符）。
    ///
    /// 目前只钉住实测会这样的 FM350 系列；机型特例表补齐后并入。
    fn sms_reads_via_pdu(&self) -> bool {
        self.is_fibocom_mtk()
    }

    /// 是否为 MTK 平台的广和通模组（FM350 / RW350 系列）。
    ///
    /// 该支线有自己的一套私有指令族（`AT+GTSENRDTEMP` / `AT+GTCCINFO` / `AT+EMBIND`），
    /// 与广和通高通 / 展锐平台（FM150 / FM160 / FM650）**完全不同**；
    /// 且这些指令发到其它厂商模组上只会白等一个超时，所以下发前必须过这道闸。
    pub(crate) fn is_fibocom_mtk(&self) -> bool {
        self.vendor() == Vendor::Fibocom
            && self.model().map(|m| is_fibocom_350(&m)).unwrap_or(false)
    }

    /// 切到 PDU 模式读一遍，只为取「存储位置 → 拼接信息」（无则返回空表）。
    fn read_concat_info(&self, p: &mut AtPort) -> HashMap<String, ConcatInfo> {
        let t = self.cmd_timeout;
        if p.command("AT+CMGF=0", t).is_err() {
            return HashMap::new();
        }
        // PDU 模式下 4 = 全部短信（对应文本模式的 "ALL"）
        let resp = p.command("AT+CMGL=4", t).unwrap_or_default();
        // 发短信依赖文本模式，无论成败都切回去
        let _ = p.command("AT+CMGF=1", t);
        sms::parse_pdu_concat_list(&resp)
    }

    /// 查询短信存储用量（附带 `AT+CPMS=?` 支持列表，取不到就留空）。
    pub fn query_sms_storage(&self) -> Result<SmsStorage> {
        // 冷门：只在短信页打开时问一次
        self.with_at_prio(Prio::Bulk, |p| {
            let t = self.cmd_timeout;
            let resp = p.command("AT+CPMS?", t)?;
            let mut st = sms::parse_storage(&resp)
                .ok_or_else(|| crate::error::msg("无法解析 AT+CPMS? 应答"))?;
            if let Ok(sup) = p.command("AT+CPMS=?", t) {
                if error_line(&sup).is_none() {
                    st.supported = sms::parse_cpms_supported(&sup);
                }
            }
            Ok(st)
        })
    }

    /// 短信条数（总数 / 未读；没读过就是 0/0）—— 角标推送用。
    pub fn sms_counts(&self) -> (usize, usize) {
        self.sms
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| (c.list.len(), c.unread))
            .unwrap_or((0, 0))
    }

    /// 设置 `[sms] sms_memory` 的持久化偏好（由 `ModemManager` 在发现时注入）。
    pub(crate) fn set_sms_memory_pref(&self, mem: &str) {
        *self.sms_mem_pref.lock().unwrap() = mem.trim().to_uppercase();
    }

    /// 清空短信缓存（换卡 / 拔卡时调：旧卡的短信不能还挂在页面上）。
    pub fn clear_sms_cache(&self) {
        *self.sms.lock().unwrap() = None;
        // 换卡后模组可能回到默认存储区 ⇒ 让下次读短信重新应用 [sms] sms_memory
        *self.sms_mem_active.lock().unwrap() = None;
    }

    pub fn vendor(&self) -> Vendor {
        *self.vendor.lock().unwrap()
    }

    pub fn model(&self) -> Option<String> {
        self.model.lock().unwrap().clone()
    }

    /// 识别时的原始应答（ATI 原文）。
    pub fn model_raw(&self) -> Option<String> {
        self.model_raw.lock().unwrap().clone()
    }

    /// 这台模组的 AT 命令口是用户手动指定的吗（见 http/handlers/at_port.rs）。
    pub fn at_port_manual(&self) -> bool {
        self.at_port_manual
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 记录这台物理设备的全部 AT 候选口（首选项在前）。
    ///
    /// 只影响展示与「还能切哪个口」的提示，不改变当前口 —— 当前口仍由
    /// [`Self::repoint`] / 发现流程决定。
    pub fn set_ttys(&self, list: Vec<String>) {
        if list.is_empty() {
            return;
        }
        *self.ttys.lock().unwrap() = list;
    }

    /// 当前登记的全部 AT 候选口（首选项在前）。
    pub fn ttys(&self) -> Vec<String> {
        let list = self.ttys.lock().unwrap().clone();
        if list.is_empty() {
            vec![self.port()]
        } else {
            list
        }
    }

    pub fn set_at_port_manual(&self, manual: bool) {
        self.at_port_manual
            .store(manual, std::sync::atomic::Ordering::Relaxed);
    }

    /// 型号特例表的匹配串（`ATI` 原文 + 清单型号）。
    ///
    /// 特例表用型号子串匹配（`RM520` / `FM350`），ATI 原文最完整；
    /// 清单型号是兜底（有些固件 ATI 只回一句型号，识别后 model 更规范）。
    pub(crate) fn quirk_hint(&self) -> String {
        let raw = self.model_raw().unwrap_or_default();
        let model = self.model().unwrap_or_default();
        format!("{raw} {model}")
    }

    /// 最近一次识别的完整结论（没有识别过就 `None`）。
    pub fn identify_info(&self) -> Option<crate::modem::detect::Identify> {
        self.ident.lock().unwrap().clone()
    }

    pub fn view(&self) -> ModemView {
        let port = self.port();
        ModemView {
            id: self.id,
            at_port: port.clone(),
            tty_list: {
                let list = self.ttys.lock().unwrap().clone();
                if list.is_empty() {
                    vec![port]
                } else {
                    list
                }
            },
            vendor: self.vendor().as_str(),
            vendor_name: self.vendor().label(),
            model: self.model(),
            model_raw: self.model_raw(),
            at_port_manual: self.at_port_manual(),
            running: self.at.lock().unwrap().is_some(),
            cell_scan_at: self.cell_scan_at(),
            identify: self.identify_info(),
        }
    }

    /// 当前 AT 口路径。
    pub fn port(&self) -> String {
        self.port.lock().unwrap().clone()
    }

    /// 在已打开的 AT 口上执行一段操作（没有就现开）。
    /// 按优先级拿串口并发命令（见 `at/sched.rs`）。
    ///
    /// ## 为什么要带优先级
    ///
    /// 早先所有人抢同一把 `Mutex`，谁先到谁先发。后台采样一密集，用户点「发送 AT」
    /// 就要排十几秒的队；反过来，用户翻短信列表（几十条命令）又把信号采样饿死，
    /// 页面上的信号值停住不动。现在按级别插队：
    /// `User`（手动 AT）> `Signal`（信号）> `Data`（拨号/IP/CA）> `Bulk`（冷门）。
    ///
    /// ## 锁的顺序（别改）
    ///
    /// 先拿调度票、再拿 `at` 锁。反过来会死锁：持 `at` 的人等票、持票的人等 `at`。
    ///
    /// ## 超时语义
    ///
    /// 排队本身**不超时**（理由见 `Scheduler::acquire`）。真卡住看
    /// [`Self::at_sched_stats`] 的 `waiting` / `max_wait_ms`。
    pub fn with_at_prio<T>(
        &self,
        prio: Prio,
        f: impl FnOnce(&mut AtPort) -> Result<T>,
    ) -> Result<T> {
        // 票的 drop 在 `guard` 之后（Rust 逆序析构）：
        // 先放开 `at`，再喊下一位，避免下一位醒来抢到票又卡在 `at` 上。
        let _ticket = self.at_sched.acquire(prio);
        let mut guard = self.at.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            // 冷却期内直接拒，**不去碰串口**（见 `at/cooldown.rs`）。
            // 返回的是带倒计时的错误，不是空数据 —— 页面要能区分
            // 「模组未就绪」和「流量/信号真的是 0」。
            if let Some(left) = self.at_cooldown.blocked_for() {
                return Err(crate::error::msg(format!(
                    "AT 口打开失败，{} 秒后重试",
                    left.as_secs() + 1
                )));
            }
            let port = self.port();
            match AtPort::open(&port, self.baud, self.cmd_timeout) {
                Ok(p) => {
                    *guard = Some(p);
                    self.at_cooldown.on_ok();
                }
                Err(e) => {
                    self.at_cooldown.on_fail();
                    return Err(e);
                }
            }
        }
        f(guard.as_mut().unwrap())
    }

    /// 串口调度的统计快照（`GET /api/at/sched`）。
    pub fn at_sched_stats(&self) -> SchedStats {
        self.at_sched.stats()
    }

    /// 当前限流参数（给接口展示，方便对账「这个数字为什么被限」）。
    pub fn at_sched_limits(&self) -> super::super::at::Limits {
        self.at_sched.limits()
    }

    /// 打开失败冷却状态 `(失败次数, 被冷却跳过次数, 冷却时长ms)`。
    pub fn at_open_stats(&self) -> (u64, u64, u64) {
        let (fails, skipped) = self.at_cooldown.stats();
        (
            fails,
            skipped,
            self.at_cooldown.cooldown().as_millis() as u64,
        )
    }

    /// 命令节奏统计 `(间隔ms, 被节流次数, 累计等待ms)`。
    ///
    /// 用 `try_lock`：AT 口正在发命令时立刻返回 `None`，**不让排障接口
    /// 去排队等串口**（否则页面点一下要等几百毫秒）。
    /// `None` 的语义是「此刻读不到」，不是「没有节流」。
    pub fn at_pacing_stats(&self) -> Option<(u64, u64, u64)> {
        let g = self.at.try_lock().ok()?;
        g.as_ref().map(|p| p.pacing_stats())
    }

    /// 取走并清空本模组 AT 口积压的 URC（主动上报）。
    ///
    /// 与查询路径共用同一把 AT 锁，所以**不会**和命令收发抢串口 ——
    /// 这也是过滤层放在读取路径而不是独立读线程的原因（半双工总线
    /// 多一个读者会把一问一答的时序搅乱）。
    ///
    /// 本方法只负责把 URC 交出来，**不解释语义**：事件化消费（驱动巡检
    /// 加速、状态推进）由 `http::loops::spawn_urc` 按 [`UrcKind`] 分流，
    /// 折算逻辑集中在 `model::urc`（纯函数，可单测）。
    pub fn take_urc_lines(&self) -> Vec<UrcLine> {
        self.with_at_prio(Prio::Signal, |p| Ok(p.take_urc()))
            .unwrap_or_default()
    }

    /// 只取走指定类别的 URC，其余留在队列里。
    ///
    /// 给「某条命令的同步应答正好走 URC 形态」的场景用（目前只有
    /// [`Self::send_ussd`] 收 `+CUSD`）：整队列取走会把 `+GTSIM` / 注册态
    /// URC 一起吃掉，巡检加速就失灵了。
    pub fn take_urc_of(&self, kinds: &[UrcKind]) -> Vec<UrcLine> {
        self.with_at_prio(Prio::Signal, |p| Ok(p.take_urc_of(kinds)))
            .unwrap_or_default()
    }

    /// 被洪水阈值丢弃的 URC 行数（累计）。
    ///
    /// 长期大于 0 说明串口在被垃圾行淹没，是排障时第一个该看的数。
    pub fn urc_dropped(&self) -> u64 {
        self.with_at_prio(Prio::Signal, |p| Ok(p.urc_dropped()))
            .unwrap_or_default()
    }

    /// 识别厂商与型号（`ATI`；不识别的模组回落 `AT+CGMR`/`AT+CGMM`）。
    ///
    /// ATI 明确报错（如展锐回 `+CME ERROR: 4`）时，
    /// 用 3GPP 标准指令的应答**重跑一遍**厂商识别，再取型号短名。
    pub fn identify(&self) -> Result<()> {
        let ati = self
            .with_at_prio(Prio::Data, |p| p.command("ATI", self.cmd_timeout))
            .unwrap_or_default();
        let lines = split_at_lines(&ati);
        // 识别用「设备自报 + USB 身份」两路取证，见 `modem::detect` 的优先级表。
        // 注意：这里**只发只读指令**，且失败时退回 AT+CGMR / AT+CGMM（部分展锐固件 ATI 回 ERROR）。
        let mut evidence_lines = lines.clone();
        if lines.is_empty() || at_resp_is_error(&lines) {
            for alt in ["AT+CGMM", "AT+CGMR"] {
                let resp = self
                    .with_at_prio(Prio::Data, |p| p.command(alt, self.cmd_timeout))
                    .unwrap_or_default();
                let alt_lines = split_at_lines(&resp);
                if alt_lines.is_empty() || at_resp_is_error(&alt_lines) {
                    continue;
                }
                evidence_lines.extend(alt_lines);
            }
        }

        let usb = crate::modem::usbdev::identity_for_tty(&self.port());
        let ident = crate::modem::detect::identify(usb.as_ref(), &evidence_lines);
        let vendor = Vendor::from_key(ident.vendor).unwrap_or(Vendor::Unknown);
        let raw = ident.model_raw.clone();
        let model = if ident.model.is_some() {
            ident.model.clone().unwrap_or_default()
        } else {
            // 清单里没有这个型号（或型号名与 ATI 差别大）：保留短名给界面显示，
            // 但结论仍标「未识别」——不因为猜了个名字就当成已确认。
            short_model_name(&raw)
        };
        *self.vendor.lock().unwrap() = vendor;
        *self.model_raw.lock().unwrap() = if raw.is_empty() { None } else { Some(raw) };
        if !model.is_empty() {
            *self.model.lock().unwrap() = Some(model.clone());
        }
        *self.ident.lock().unwrap() = Some(ident);
        // 厂商/型号可能变了：厂商方言相关的缓存全部作废重探
        *self.lock_dialect_slot.lock().unwrap() = None;
        *self.lock_status_slot.lock().unwrap() = None;
        *self.cells.lock().unwrap() = None;
        Ok(())
    }

    /// 把命令口切到 `port` 并重新识别（关闭旧口 → 换口 → 重新识别）。
    ///
    /// 用于 `/api/at/port`：用户手动指定的命令口与系统探到的不同时整口换掉。
    /// 厂商/型号缓存由 `identify` 重填；短信缓存是旧口读到的，一并清掉。
    pub fn repoint(&self, port: &str) -> Result<()> {
        *self.at.lock().unwrap() = None;
        *self.port.lock().unwrap() = port.to_string();
        {
            // 切过去的口排到最前，其余保留（页面「其它 tty」据此显示）
            let mut list = self.ttys.lock().unwrap();
            list.retain(|p| p != port);
            list.insert(0, port.to_string());
        }
        *self.sms.lock().unwrap() = None;
        // 手动指定可能指向另一台机器：IMEI 缓存一并作废
        *self.imei_cache.lock().unwrap() = None;
        self.identify()
    }

    /// 手动指定命令口时用的「模组身份」键：
    /// IMEI 优先，取不到就给型号；再取不到返回 None（宁可不记，也别把两台模组串在一起）。
    ///
    /// 注意这里**不**用端口号当键 —— 端口号正是要换的那个东西。
    pub fn at_port_key(&self) -> Option<String> {
        if let Some(imei) = self.imei() {
            if !imei.trim().is_empty() {
                return Some(format!("imei:{}", imei.trim()));
            }
        }
        let model = self.model().unwrap_or_default();
        if !model.trim().is_empty() {
            return Some(format!("model:{}", model.trim()));
        }
        None
    }

    // ---------------- SIM 热插拔（见 model/sim.rs） ----------------

    /// 当前记录的 SIM 身份快照（不加 AT，只读缓存）。
    pub fn sim_identity(&self) -> SimIdentity {
        self.sim_ident.lock().unwrap().clone()
    }

    /// 最近一次热插拔事件（空串 = 没有过）与发生时刻（unix 秒，0 = 没有过）。
    pub fn sim_last_event(&self) -> (String, i64) {
        (
            self.sim_last_event.lock().unwrap().clone(),
            *self.sim_last_event_at.lock().unwrap(),
        )
    }

    /// 最近一次核实 SIM 身份的时刻（unix 秒，0 = 没查过）。
    pub fn sim_checked_at(&self) -> i64 {
        self.sim_checked_at_secs
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 标记「该去核实 SIM 了」：URC 消费端（`http::loops::spawn_urc`）看到
    /// `+GTSIM:` 等 SIM 相关上报时调。
    ///
    /// `+CPIN` 的主动上报与 `AT+CPIN?` 应答同形、过滤层刻意不吞，所以它不是置脏来源；
    /// 置脏后由巡检读一次权威身份（ICCID/IMSI 没变就不会触发任何动作）。
    pub fn mark_sim_dirty(&self) {
        self.sim_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// 标记「该重查网络状态了」：URC 消费端看到注册态变化（`+CEREG:` 等）时调。
    ///
    /// 只置脏、不改缓存：单字段注册态 URC 与 `n=0` 时的查询应答同形，
    /// 不足以推定状态（见 `model/urc.rs` 的说明），权威读数仍走查询路径。
    pub fn mark_net_dirty(&self) {
        self.net_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// 取走网络置脏标记（取走即清）：供巡检循环决定本轮要不要重查网络。
    ///
    /// 取走即清是为了「同一时刻只有一个循环响应」：push 与 change 两个循环
    /// 各自持一份节流表，谁先看到脏标记谁去查，另一个不会跟着重复查。
    pub fn take_net_dirty(&self) -> bool {
        self.net_dirty
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// 读一帧 SIM 身份：便宜的在前面（`AT+CPIN?` 一条就能判卡在不在，
    /// 不在就不必再问 ICCID）；读不到 ICCID 的固件用 IMSI 当身份。
    pub fn query_sim_identity(&self) -> SimIdentity {
        let mut id = SimIdentity::default();
        let resp = self
            .with_at_prio(Prio::Signal, |p| p.command("AT+CPIN?", self.cmd_timeout))
            .unwrap_or_default();
        match extract_field(&resp, "+CPIN") {
            Some(state) => id.state = state,
            None => return id,
        }
        id.present = super::sim::cpin_present(&id.state);
        if !id.present {
            return id;
        }
        if let Some(resp) = self.query_first(&["AT+CCID", "AT+QCCID", "AT^ICCID?"]) {
            if let Some(v) = super::siminfo::parse_iccid(&resp) {
                id.iccid = v;
                return id;
            }
        }
        let resp = self
            .with_at_prio(Prio::Signal, |p| p.command("AT+CIMI", self.cmd_timeout))
            .unwrap_or_default();
        if let Some(v) = super::siminfo::parse_imsi(&resp) {
            id.imsi = v;
        }
        id
    }

    /// 读一次 SIM 身份；变了就处理（事件 + 复位决策）。
    ///
    /// 返回本次检出的事件类型（空串 = 没变化），供调用方广播 `sim` 推送。
    ///
    /// 两条「绝不误判」的规则（都是真机踩出来的）：
    ///   1. 读不到确定答复（`state` 为空）时不判任何事件 —— 模组复位 / USB 重新枚举期间
    ///      CPIN 一定读不到，把它当「拔卡」会引出无限复位循环；
    ///   2. 卡不在位时保留上次读到的 ICCID/IMSI，且第一次「确定答复」只建基线不报事件。
    pub fn check_sim_hotswap(&self, cfg_reset: bool) -> String {
        let now = Instant::now();
        let now_secs = crate::logging::unix_now();
        self.sim_checked_at.lock().unwrap().replace(now);
        self.sim_checked_at_secs
            .store(now_secs, std::sync::atomic::Ordering::Relaxed);
        self.sim_dirty
            .store(false, std::sync::atomic::Ordering::Relaxed);

        let (prev, known) = {
            let g = self.sim_ident.lock().unwrap();
            (
                g.clone(),
                self.sim_known.load(std::sync::atomic::Ordering::Relaxed),
            )
        };
        let mut cur = self.query_sim_identity();
        if cur.state.is_empty() {
            // 读失败：只更新「何时查过」，身份原样保留（连 present 都不改）
            return String::new();
        }
        let kind = if known {
            super::sim::sim_hotswap_event(&prev, &cur)
        } else {
            ""
        };
        if !cur.present {
            // 卡读不到时别把身份丢了（否则同一张卡回来会被当成新卡）
            cur.iccid = prev.iccid.clone();
            cur.imsi = prev.imsi.clone();
        }
        *self.sim_ident.lock().unwrap() = cur.clone();
        self.sim_known
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if kind.is_empty() {
            return String::new();
        }
        if super::sim::in_cooldown(*self.sim_cooldown_until.lock().unwrap(), now) {
            log::info!(
                "模组 id={} 检出 SIM {}，但仍在冷却期，只更新状态",
                self.id,
                kind
            );
            return String::new();
        }
        *self.sim_cooldown_until.lock().unwrap() = Some(now + super::sim::SIM_COOLDOWN);
        *self.sim_last_event.lock().unwrap() = kind.to_string();
        *self.sim_last_event_at.lock().unwrap() = now_secs;
        log::warn!(
            "模组 id={} SIM 热插拔：{}（在位={} 状态={} ICCID={}）",
            self.id,
            kind,
            cur.present,
            cur.state,
            cur.iccid
        );

        // 卡都换了：缓存的 ICCID/IMSI/号码整块作废，界面下次一读就是新卡
        *self.imei_cache.lock().unwrap() = None;
        *self.sms.lock().unwrap() = None;
        if kind == super::sim::SIM_REMOVED {
            // 卡拔了：别让屏幕/网页还挂着旧卡那几条
            return kind.to_string();
        }
        if super::sim::should_reset_modem(kind, cfg_reset) {
            // 有的固件这条不回 OK，超时是正常的 —— 只记日志
            if let Err(e) = self.with_at_prio(Prio::Data, |p| {
                p.command("AT+CFUN=1,1", Duration::from_secs(5))
            }) {
                log::debug!("模组 id={} 换卡后复位未拿到 OK: {e}", self.id);
            }
            log::warn!("模组 id={} 已发 AT+CFUN=1,1 让新卡生效", self.id);
        }
        kind.to_string()
    }

    /// 该不该现在去核实 SIM（供巡检循环调用；见 model/sim.rs 的判据）。
    pub fn sim_due(&self, every: std::time::Duration, now: Instant) -> bool {
        let dirty = self.sim_dirty.load(std::sync::atomic::Ordering::Relaxed);
        super::sim::should_check_sim(*self.sim_checked_at.lock().unwrap(), every, dirty, now)
    }
}

/// 把 AT 原始应答拆成行（去 \r\n、去首尾空白、丢空行）。
///
/// 与 `/api/at` 的 `resp` 同一种「应答行」。
pub fn split_at_lines(resp: &str) -> Vec<String> {
    resp.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// 应答里「只有错误行」（没有一条像样的数据）⇒ 这条指令不被支持。
///
/// 展锐 UMS9620 的 ATI 直接回 `+CME ERROR: 4`，
/// 这时要走 `AT+CGMR`/`AT+CGMM` 兜底认厂商。
pub fn at_resp_is_error(lines: &[String]) -> bool {
    let mut seen = false;
    for l in lines {
        let up = l.to_uppercase();
        if up == "OK" {
            continue;
        }
        seen = true;
        if !up.contains("ERROR") {
            return false;
        }
    }
    seen
}

/// 从 ATI 原始应答里取出**型号短名**（界面「型号」一栏用）。
///
/// 型号短名（实测过的几类写法）：
///   ① `Manufacturer: TD Tech Ltd.|Model: MT5700M-CN|…` → MT5700M-CN
///   ② `ATI|Quectel|RM520N-GL|Revision: …|OK`            → RM520N-GL
///   ③ 展锐的「标签: 值」版本块（Platform/Project/HW Version）→ 挑一个
/// 认不出时截断到 40 字符，不编造。
fn short_model_name(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    // ① 带标签的写法：取 "Model:" 后的第一个字段（分隔符可能是 ':' 或 '：'）
    if let Some(rest) = labeled_value(raw, "MODEL") {
        return rest;
    }
    // ② 逐段写法（移远）：跳过回显/结束标记/带标签段/厂商标识段，
    //    取第一个「同时含字母和数字」的段当型号
    for seg in raw.split(['|', '\r', '\n']) {
        let seg = seg.trim();
        if seg.is_empty() || seg.contains(':') || seg.contains('：') {
            continue;
        }
        match seg.to_uppercase().as_str() {
            "ATI" | "AT" | "OK" | "ERROR" | "QUECTEL" | "FIBOCOM" | "SIMCOM" | "TD TECH" => {
                continue
            }
            _ => {}
        }
        if has_letter_and_digit(seg) {
            return seg.to_string();
        }
    }
    // ③ 「标签: 值」的版本块（展锐 UMS9620 / 中兴 F50 的 AT+CGMR）
    if let Some(v) = labeled_version_value(raw) {
        return v;
    }
    if raw.chars().count() > 40 {
        return raw.chars().take(40).collect();
    }
    raw.to_string()
}

/// 取 `raw` 里 `<label>：<值>` 形式的值（标签大小写不敏感；分隔符半角/全角都认）。
fn labeled_value(raw: &str, label: &str) -> Option<String> {
    for line in raw.split(['|', '\r', '\n']) {
        let Some(i) = line.find([':', '：']) else {
            continue;
        };
        let sep_len = if line.as_bytes()[i] == b':' {
            1
        } else {
            '：'.len_utf8()
        };
        let key = line[..i].trim().to_uppercase();
        if key != label {
            continue;
        }
        let value = line[i + sep_len..].trim().trim_matches('"').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// 「标签: 值」版本块里挑一个能当型号的值：PROJECT/MODEL/PRODUCT 优先，
/// 其次 HW/HARDWARE，最后第一条。
fn labeled_version_value(raw: &str) -> Option<String> {
    let mut all: Vec<(String, String)> = Vec::new();
    for line in raw.split(['|', '\r', '\n']) {
        let Some(i) = line.find([':', '：']) else {
            continue;
        };
        let sep_len = if line.as_bytes()[i] == b':' {
            1
        } else {
            '：'.len_utf8()
        };
        let key = line[..i].trim().to_uppercase();
        let value = line[i + sep_len..]
            .trim()
            .trim_matches('"')
            .trim()
            .to_string();
        if key.is_empty() || value.is_empty() || !has_letter_and_digit(&value) {
            continue;
        }
        all.push((key, value));
    }
    let pick = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|k| {
            all.iter()
                .find(|(label, _)| label.contains(k))
                .map(|(_, v)| v.clone())
        })
    };
    pick(&["PROJECT", "MODEL", "PRODUCT"])
        .or_else(|| pick(&["HW", "HARDWARE"]))
        .or_else(|| all.first().map(|(_, v)| v.clone()))
}

fn has_letter_and_digit(s: &str) -> bool {
    let mut letter = false;
    let mut digit = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            digit = true;
        } else if c.is_ascii_alphabetic() {
            letter = true;
        }
    }
    letter && digit
}

/// 广和通 **350 系列**（FM350 / RW350）：同一套 MTK 平台行为。
///
/// 两者在主机侧完全同构：USB 网卡不发 DHCP、私有指令族是 `AT+GT*`、温度要
/// `AT+GTSENRDTEMP=0`、小区只有 `AT+GTCCINFO?`。参考实现只钉了 FM350，
/// 这里把 RW350 一起收进来 —— **判定只此一处**，不要在别处再写 `contains("FM350")`
/// （之前 `hostnic::quirk` 与 `dial::host_nic_quirk` 各抄了一份，加 RW350 时就会漏）。
pub fn is_fibocom_350(model: &str) -> bool {
    let u = model.to_ascii_uppercase();
    u.contains("FM350") || u.contains("RW350")
}

#[cfg(test)]
mod identity_tests {
    use super::{
        at_resp_is_error, has_letter_and_digit, is_fibocom_350, labeled_version_value,
        short_model_name, split_at_lines, Modem, Vendor,
    };
    use std::time::Duration;

    #[test]
    fn fibocom_350_family_covers_fm350_and_rw350() {
        for m in [
            "FM350-GL",
            "fm350",
            "RW350-CN",
            "ATI|Manufacturer: Fibocom|Model: RW350|Revision: x",
        ] {
            assert!(is_fibocom_350(m), "{m:?} 应算 350 系列");
        }
        // 高通平台与别的型号不能误伤（它们的指令族完全不同）
        for m in ["FM160-CN", "FM650-CN", "RM520N-GL", "", "RW351"] {
            assert!(!is_fibocom_350(m), "{m:?} 不该算 350 系列");
        }
    }

    #[test]
    fn split_lines_drops_empty_and_trims() {
        let got = split_at_lines("\r\nATI\r\nQuectel\r\n\r\nOK\r\n");
        assert_eq!(got, vec!["ATI", "Quectel", "OK"]);
    }

    #[test]
    fn error_only_resp() {
        assert!(at_resp_is_error(&["+CME ERROR: 4".to_string()]));
        assert!(!at_resp_is_error(&["OK".to_string()]));
        assert!(!at_resp_is_error(&["RM520N-GL".to_string()]));
        // 空应答不算「错误」——seen=false
        assert!(!at_resp_is_error(&[]));
    }

    /// 网络置脏取一次就清：push / change 两个循环都看这个标记，
    /// 不清就会变成「同一件事查两遍」，节流白做。
    #[test]
    fn net_dirty_take_clears() {
        let m = Modem::new(
            1,
            "/dev/null".to_string(),
            115200,
            Duration::from_secs(3),
            Vendor::Fibocom,
        );
        assert!(!m.take_net_dirty(), "默认不该是脏的");
        m.mark_net_dirty();
        assert!(m.take_net_dirty());
        assert!(!m.take_net_dirty(), "第二次应已清空");
    }

    /// 一台物理设备的多个 AT 口：`tty_list` 要按「当前口在前」列出全部候选，
    /// 切口后新口排到最前（页面「其它 tty」就靠这个）。
    #[test]
    fn tty_list_follows_current_port() {
        let m = Modem::new(
            2,
            "/dev/ttyUSB2".to_string(),
            115200,
            Duration::from_secs(3),
            Vendor::Quectel,
        );
        assert_eq!(m.ttys(), vec!["/dev/ttyUSB2"]);
        m.set_ttys(vec![
            "/dev/ttyUSB2".to_string(),
            "/dev/ttyUSB3".to_string(),
            "/dev/ttyUSB4".to_string(),
        ]);
        assert_eq!(m.view().tty_list.len(), 3);
        // repoint 会重开串口并识别；这里只验证列表顺序（打开失败不影响顺序）
        let _ = m.repoint("/dev/ttyUSB3");
        assert_eq!(m.ttys().first().map(String::as_str), Some("/dev/ttyUSB3"));
        assert_eq!(m.ttys().len(), 3, "其余候选口不能丢");
    }

    #[test]
    fn short_names() {
        // ① 鼎桥 / 广和通的带标签写法
        assert_eq!(
            short_model_name("Manufacturer: TD Tech Ltd.|Model: MT5700M-CN|Revision: X|OK"),
            "MT5700M-CN"
        );
        assert_eq!(
            short_model_name("Manufacturer: Fibocom Wireless Inc.|Model: FM160-EAU|OK"),
            "FM160-EAU"
        );
        // ② 移远的逐段写法
        assert_eq!(
            short_model_name("ATI|Quectel|RM520N-GL|Revision: RM520NGLAAR01A07M4G|OK"),
            "RM520N-GL"
        );
        // ③ 展锐的版本块
        assert_eq!(
            short_model_name(
                "Platform Version: MOCORTM_V2_22C_W25.39.4_Debug\nProject Version: QogirN6Pro_PS\nHW Version: ums9620_modem"
            ),
            "QogirN6Pro_PS"
        );
        // 认不出：截断到 40 字符
        let long = "x".repeat(80);
        assert_eq!(short_model_name(&long).chars().count(), 40);
        assert_eq!(short_model_name(""), "");
    }

    #[test]
    fn labeled_version_pick_order() {
        assert_eq!(
            labeled_version_value("HW Version: ums9620_modem"),
            Some("ums9620_modem".to_string())
        );
        assert_eq!(labeled_version_value("no label here"), None);
    }

    #[test]
    fn letter_and_digit() {
        assert!(has_letter_and_digit("RM520N-GL"));
        assert!(!has_letter_and_digit("Quectel"));
        assert!(!has_letter_and_digit("520"));
    }
}
