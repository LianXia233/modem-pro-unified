//! HTTP 服务与接口。
//!
//! 拆分口径：
//!   · `gateway`    —— 同端口网关：WebSocket 升级识别 + HTTP 透传
//!   · `server`     —— 监听循环的对外入口
//!   · `ws`         —— WebSocket 握手/帧编解码与推送中枢
//!   · `push`       —— 各面板的广播（顺手记录波动，不额外发 AT）
//!   · `loops`      —— 后台采样循环（信号/网络/流量/配额/日志）
//!   · `logtail`    —— 运行日志的增量读取器
//!   · `router`     —— 路径分发表
//!   · `request`    —— query / 请求体解析
//!   · `response`   —— 统一 JSON 响应 `{"code":..,"data":..,"msg":..}`
//!   · `auth`       —— 「跟随 LuCI 登录」闸门
//!   · `static_ui`  —— 编译期嵌入的调试台页面
//!   · `handlers/*` —— 每个接口一个文件
//!
//! 接口契约见 docs/API.md，迁移清单见 docs/PORTING.md。

mod auth;
pub(crate) mod cache;
mod gateway;
mod handlers;
pub(crate) mod hostnic;
mod logtail;
mod loops;
mod push;
mod quota;
mod request;
mod response;
mod router;
pub(crate) mod schedule;
mod server;
mod static_ui;
pub(crate) mod tasks;
mod ws;

use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::error::Result;
use crate::modem::ModemManager;
use crate::ubus::UbusClient;

/// 所有请求共享的运行时状态。
pub struct AppState {
    pub(crate) mgr: Arc<ModemManager>,
    pub(crate) cfg: Arc<Config>,
    pub(crate) ubus: UbusClient,
    pub(crate) ws: Arc<ws::WsHub>,
    /// 波动记录与信号采样历史（见 modem/metrics.rs）
    pub(crate) metrics: crate::modem::metrics::Metrics,
    /// 流量配额台账（quota.json，见 runtime/store.rs 与 http/quota.rs）
    pub(crate) quota: Arc<crate::runtime::store::QuotaStore>,
    /// IMEI 原始值备份台账（imei.json，改写前自动存一份，见 runtime/store.rs）
    pub(crate) imei: Arc<crate::runtime::store::ImeiStore>,
    /// 手动指定的 AT 命令口（atport.json，见 runtime/store.rs 与 handlers/at_port.rs）
    pub(crate) atport: Arc<crate::runtime::store::AtPortStore>,
    /// 按 USB 设备身份（vid:pid）绑定的命令口（atport_bind.json，见 handlers/atport_bind.rs）
    pub(crate) atport_bind: Arc<crate::runtime::store::AtPortBindStore>,
    /// 定时任务台账（schedule.json，见 runtime/store.rs 与 http/schedule.rs）
    pub(crate) sched: Arc<crate::runtime::store::SchedStore>,
    /// 主机网卡配置台账（hostnic.json，见 runtime/hostnic.rs 与 http/hostnic.rs）
    pub(crate) hostnic: Arc<crate::runtime::hostnic::HostNicStore>,
    /// 配置文件路径（写回 `config.toml` 用，见 config/patch.rs）
    pub(crate) config_path: String,
    /// 运行期改过的「允许数据漫游」（写回 `config.toml` 后即时生效）
    pub(crate) net_allow_roaming: Mutex<Option<bool>>,
    /// 运行期改过的拨号总开关（`[net] dial_enabled`，见 handlers/dial_enable.rs）
    pub(crate) net_dial_enabled: Mutex<Option<bool>>,
    /// 运行期改过的模组侧 IPv6 开关（`[net] wan_ipv6`，见 handlers/dial_ipv6.rs）
    pub(crate) net_wan_ipv6: Mutex<Option<bool>>,
    /// 运行期改过的接口模式（`[net] wan_mode`，见 handlers/dial_mode.rs）
    pub(crate) net_wan_mode: Mutex<Option<String>>,
    /// 各模组最近一次自动组网的结果（见 netprov/provision.rs，`/api/dial` 读它）
    pub(crate) netprov: crate::netprov::provision::ReportStore,
    /// 企业微信推送客户端（见 wechat/，`/api/wechat/*` 与 `+CMTI` 路径共用）
    pub(crate) wechat: crate::wechat::client::WeChatClient,
    /// 只读状态缓存：页面首屏用「旧值 + 年龄」，不再每条都发 AT（见 http/cache.rs）
    pub(crate) cache: Arc<cache::StateCache>,
    /// 异步任务注册表：慢操作提交 → 轮询，不占 HTTP 线程与串口队列（见 http/tasks.rs）
    pub(crate) tasks: Arc<tasks::Registry>,
    /// 基于 sysfs 的链路流量采样器（1 秒一拍，**独立于前端**）。
    ///
    /// 与 `modem/traffic.rs`（AT 计数器）的分工见 `runtime/ifstat.rs` 的模块注释。
    /// 这是页面速率/累计/曲线的主数据源：FM350-GL 的 `AT+GTSTATIS` 实测回 ERROR，
    /// 只有 sysfs 的 rx/tx 是准的。
    pub(crate) ifstat: Arc<crate::runtime::ifstat::IfStat>,
    /// 最近一次「有人在通过 HTTP 看」的时刻（Unix 秒）；0 = 从没有过。
    ///
    /// ## 为什么需要它
    ///
    /// LuCI 前端走的是 **HTTP 轮询**（`modemd.store` 每 3 秒拉一次 `/api/snapshot`），
    /// 它不连 WebSocket —— 跨端口跨源，而 ucode 代理是「一次请求一次 `wget`」，
    /// 天生转不了长连接。可后台采样循环原本只用 `ws.active_count()` 判断「有没有人在看」，
    /// 于是 LuCI 用户**一次采样都触发不了**：快照里的 `blocks` 永远是空的，
    /// `ages` 永远 `null`、`stale` 永远 `true`，页面只能一片 `-`。
    ///
    /// 把「刚有人拉过快照」也算作「有人在看」，两类前端的口径就统一了，
    /// 「没人在看就别发 AT」的初衷也仍然成立（关掉页面 N 秒后自动停）。
    pub(crate) last_http_seen: std::sync::atomic::AtomicI64,
}

impl AppState {
    /// 当前是否允许数据漫游：运行期改过的优先，否则取配置段。
    pub(crate) fn allow_roaming(&self) -> bool {
        self.net_allow_roaming
            .lock()
            .unwrap()
            .unwrap_or(self.cfg.net.allow_roaming)
    }

    /// 拨号总开关（页面「拨号上网 → 拨号开关」）：运行期改过的优先。
    pub(crate) fn dial_enabled(&self) -> bool {
        self.net_dial_enabled
            .lock()
            .unwrap()
            .unwrap_or(self.cfg.net.dial_enabled)
    }

    /// 模组侧 IPv6 开关（`[net] wan_ipv6`）：运行期改过的优先。
    pub(crate) fn wan_ipv6(&self) -> bool {
        self.net_wan_ipv6
            .lock()
            .unwrap()
            .unwrap_or(self.cfg.net.wan_ipv6)
    }

    /// 接口模式（`[net] wan_mode`）：运行期改过的优先，认不出的回落默认。
    pub(crate) fn wan_mode(&self) -> crate::config::WanMode {
        let guard = self.net_wan_mode.lock().unwrap();
        let raw = guard.as_deref().unwrap_or(self.cfg.net.wan_mode.as_str());
        crate::config::WanMode::parse(raw)
    }

    /// 记一次「有人在通过 HTTP 看」（拉快照 / 查状态都算）。
    ///
    /// 由 `/api/snapshot` 与几个高频只读接口调用：它们是「页面还开着」的可靠信号。
    pub(crate) fn note_http_seen(&self) {
        self.last_http_seen.store(
            crate::logging::unix_now(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// 有没有「人在看」：WebSocket 主动客户端，或最近 `within` 秒内拉过 HTTP 接口。
    ///
    /// 后台循环用它决定要不要发 AT。两类前端统一在这里判，别再各自只看 WS
    /// （那样 LuCI 的 HTTP 轮询会被整轮跳过，见 [`Self::last_http_seen`]）。
    pub(crate) fn viewer_active(&self, within: i64) -> bool {
        if self.ws.active_count() > 0 {
            return true;
        }
        http_seen_recent(
            self.last_http_seen
                .load(std::sync::atomic::Ordering::Relaxed),
            crate::logging::unix_now(),
            within,
        )
    }
}

/// 「最近有没有人通过 HTTP 看过」的纯判断（抽成自由函数是为了能单测）。
///
/// `last == 0` 表示从没有人拉过接口 —— 那不算「有人在看」，
/// 否则服务刚启动（时间戳还没被写过）会被误判成「正有人盯着」。
fn http_seen_recent(last: i64, now: i64, within: i64) -> bool {
    last > 0 && now.saturating_sub(last) <= within.max(1)
}

/// 启动 HTTP 服务（阻塞）。`config_path` 只用来定位台账文件（与配置文件同目录）。
pub fn serve(
    mgr: Arc<ModemManager>,
    cfg: Arc<Config>,
    config_path: &str,
    atport: Arc<crate::runtime::store::AtPortStore>,
    atport_bind: Arc<crate::runtime::store::AtPortBindStore>,
) -> Result<()> {
    let ubus = UbusClient::from_url(&cfg.ui.ubus_http);
    // 企业微信客户端要在 cfg 移进 AppState **之前**建（它持有配置快照）
    let wechat = crate::wechat::client::WeChatClient::new(&cfg.wechat);
    let state = Arc::new(AppState {
        mgr,
        cfg,
        ubus,
        ws: Arc::new(ws::WsHub::new()),
        metrics: crate::modem::metrics::Metrics::new(),
        quota: quota::store_for(config_path),
        imei: Arc::new(crate::runtime::store::ImeiStore::new(
            crate::runtime::store::imei_file(config_path),
        )),
        atport,
        atport_bind,
        sched: Arc::new(crate::runtime::store::SchedStore::new(
            crate::runtime::store::schedule_file(config_path),
        )),
        hostnic: Arc::new(crate::runtime::hostnic::HostNicStore::new(
            crate::runtime::hostnic::hostnic_file(config_path),
        )),
        config_path: config_path.to_string(),
        net_allow_roaming: Mutex::new(None),
        net_dial_enabled: Mutex::new(None),
        net_wan_ipv6: Mutex::new(None),
        net_wan_mode: Mutex::new(None),
        netprov: crate::netprov::provision::ReportStore::new(),
        wechat,
        cache: Arc::new(cache::StateCache::new()),
        tasks: Arc::new(tasks::Registry::new()),
        last_http_seen: std::sync::atomic::AtomicI64::new(0),
        ifstat: Arc::new(crate::runtime::ifstat::IfStat::new()),
    });
    log::info!("流量配额台账：{}", state.quota.path());
    log::info!("IMEI 备份台账：{}", state.imei.path());
    log::info!("AT 命令口指定：{}", state.atport.path());
    log::info!("AT 命令口绑定（VID:PID）：{}", state.atport_bind.path());
    // 退出时把节流中的台账刷掉（定时任务 5 秒合并写，SIGTERM 可能正好落在窗口里）
    {
        let sched = state.sched.clone();
        crate::runtime::store::register_flush(move || sched.flush());
    }
    // NTP 校时（`[ntp]` 段）：先应用配置时区（后续日志/调度立刻用上），
    // 再起后台同步线程；状态变化广播成 `ntp` WS 消息（见 runtime/ntp.rs）。
    crate::runtime::ntp::spawn(state.cfg.ntp.clone(), {
        let state = state.clone();
        move |s| state.ws.broadcast("ntp", None, s.to_json())
    });
    // 定时任务循环：与配额一样不看有没有客户端在线 —— 到点动作必须执行。
    // 启动先补跑一轮，把"进程重启前几分钟内该做但还没来得及做"的槽位补上。
    crate::runtime::scheduler::spawn_schedule(cfg_schedule_interval(&state), {
        let state = state.clone();
        move || schedule::tick(&state)
    });
    // 链路流量采样（sysfs，固定 1000ms，**不看前端**）。
    //
    // pick 回调每 5 秒被调一次，返回「当前活跃模组的数据网卡」：
    // 用 `netprov::pick_device` 从模组自己的 USB 设备推出来，而不是全局扫
    // `eth0/wwan0` 这类固定名 —— 换机型、换枚举顺序、QMAP mux 都不会认错。
    {
        let stat = state.ifstat.clone();
        let mgr = state.mgr.clone();
        crate::runtime::ifstat::spawn(stat, move || {
            // ① 优先：在跑的模组 → 按**它的 USB 设备**推网卡
            //    （`pick_device` 会依次试 QMAP mux 口 / 强候选 / 同 USB 设备下的口，
            //     并排除 eth0/eth1 这类板载口，最不容易认错）
            if let Some(modem) = mgr.all().iter().find(|m| m.is_running()) {
                let dev = crate::netprov::provision::pick_device(modem, &[]);
                if !dev.is_empty() {
                    return Some(dev);
                }
            }
            // ② 回退：按**驱动**全局找数据口（rndis_host / qmi_wwan / cdc_mbim…）。
            //
            // 为什么需要这一层：模组还没登记进管理器时（服务刚起、串口正被别的
            // 进程占着探测失败），① 拿不到东西 —— 但网卡其实早就在了。
            // 此时仍要能统计，否则页面在启动头几十秒里流量全空。
            crate::runtime::hostnic::detect_data_dev(&[])
        });
    }

    // 后台采样循环：信号/网络/流量/配额/日志（见 http/loops.rs 的三类判据）
    loops::spawn_all(&state);
    log::info!("后台循环已启动：{} 个采样线程", loops::thread_count(&state));
    server::run(state)
}

/// 定时任务循环的节拍（秒；配置里 <=0 按 10 秒兜底）。
fn cfg_schedule_interval(state: &AppState) -> i64 {
    let n = state.cfg.server.schedule_interval;
    if n <= 0 {
        10
    } else {
        n
    }
}

#[cfg(test)]
mod tests {
    use super::http_seen_recent;

    #[test]
    fn http_seen_recent_boundaries() {
        // 从没人拉过接口（时间戳还是 0）= 没人在看，不能算「正有人盯着」
        assert!(!http_seen_recent(0, 1000, 60));
        // 刚拉过（差值 0）→ 在看
        assert!(http_seen_recent(1000, 1000, 60));
        // 窗口内 → 在看（含边界）
        assert!(http_seen_recent(1000, 1060, 60));
        // 超窗口 → 不在看（页面已关，停发 AT 省串口）
        assert!(!http_seen_recent(1000, 1061, 60));
    }

    #[test]
    fn http_seen_recent_survives_clock_rollback() {
        // NTP 校时把时钟往回调时，now - last 为负，saturating_sub 饱和到 0，
        // 于是判为「刚有人看过」→ 继续采样。
        //
        // 这是**有意的**降级，不是 bug：时钟异常时我们无从知道最后一次访问是多久前，
        // 此时停掉所有后台采样（页面静默失去数据）比多采一轮糟得多。
        // 用 saturating_sub 而非 `-`，也顺带杜绝了 i64 下溢。
        assert!(http_seen_recent(1000, 500, 60));
    }

    #[test]
    fn http_seen_recent_ignores_nonpositive_window() {
        // within <= 0 时兜底成 1 秒，避免「永远不在看」把后台采样彻底关死
        assert!(http_seen_recent(1000, 1000, 0));
        assert!(!http_seen_recent(1000, 1002, 0));
    }
}
