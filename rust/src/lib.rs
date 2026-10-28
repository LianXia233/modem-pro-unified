//! modemd 库根。
//!
//! 各功能域拆成独立模块，`main.rs` 只负责调用 [`run`]。
//! 拆分口径：
//!   · `cli`      —— 命令行参数
//!   · `config`   —— 配置分节（server/serial/sms/sim/net/dial/modem/ui/wechat）
//!   · `logging`  —— 日志器与时间工具
//!   · `netprov`  —— 模组上线自动组网（纯策略 + UCI 薄 IO + 编排）
//!   · `ubus`     —— 与 rpcd 的会话交互（transport/rpc/session 分离）
//!   · `modem`    —— AT 收发 / 发现 / 模型 / 厂商 / 清单
//!   · `http`     —— 服务器 / 路由 / 请求 / 响应 / 鉴权 / handlers
//!   · `runtime`  —— PID / 信号 / 拨号器回收 / 后台巡检
//!   · `wechat`   —— 企业微信推送（纯逻辑 + curl 传输 + 运行期状态）

pub mod cli;
pub mod config;
pub mod error;
pub mod http;
pub mod logging;
pub mod modem;
pub mod netprov;
pub mod runtime;
pub mod ubus;
pub mod version;
pub mod wechat;

use std::sync::Arc;

/// 装配并阻塞运行守护进程。
///
/// 顺序：读配置 → 起日志 → 写 PID → 首次发现模组 → 起后台巡检 → 装退出钩子 →
/// 起 HTTP 服务（阻塞直到进程退出）。
pub fn run() -> error::Result<()> {
    let path = cli::config_path();
    // 配置读不了也照常起服务（兜底链见 config/loader.rs）：界面全丢比配置回默认值糟得多
    let cfg = config::load(&path);
    logging::init(&cfg.server.log_file, &cfg.server.log_level);
    log::info!(
        "modemd (rust) {} 启动，配置 {path}",
        version::build_version()
    );

    runtime::pid::write(&cfg.server.pid_file);

    let atport = Arc::new(runtime::store::AtPortStore::new(
        runtime::store::atport_file(&path),
    ));
    let atport_bind = Arc::new(runtime::store::AtPortBindStore::new(
        runtime::store::atport_bind_file(&path),
    ));
    let mgr = Arc::new(modem::ModemManager::new(
        cfg.clone(),
        atport.clone(),
        atport_bind.clone(),
    ));
    // 首次发现放到**后台**：HTTP 必须先起来。
    //
    // 原来是同步 `discover(true)` 挡在 `http::serve` 前面。实机（FM350-GL）上
    // 这台机器有 7 个 `/dev/ttyUSB*`，只有 ttyUSB4 是 AT 口，其余每个都要等
    // 探测超时 —— 启动到 HTTP 可用整整 **40 秒**，这 40 秒里 LuCI 所有页面都是
    // 「modemd 无响应」。放到后台后页面秒开，模组列表几秒后自己填上
    //（页面本来就在轮询 `/api/snapshot`，`modems` 会随之更新）。
    //
    // `discover` 内部有互斥锁，与后面的周期巡检、页面的 `?discover=1` 并发安全。
    {
        let mgr = mgr.clone();
        std::thread::spawn(move || {
            mgr.discover(true);
            log::info!("启动发现完成：模组 {} 个", mgr.count());
        });
    }

    runtime::scheduler::spawn_scan(mgr.clone(), cfg.server.scan_interval);

    let pid_on_exit = cfg.server.pid_file.clone();
    runtime::signals::install(move || {
        log::info!("收到退出信号，落盘台账、停止主机侧拨号器并退出");
        runtime::store::flush_all();
        runtime::dialer::cleanup_all();
        runtime::pid::remove(&pid_on_exit);
        std::process::exit(0);
    });

    http::serve(mgr, Arc::new(cfg), &path, atport, atport_bind)
}
