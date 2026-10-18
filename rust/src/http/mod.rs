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
mod smsauth;
mod static_ui;
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
    /// 短信访问密码的令牌表与失败节流（见 http/smsauth.rs）
    pub(crate) sms_auth: smsauth::SmsAuth,
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
        sms_auth: smsauth::SmsAuth::new(),
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
