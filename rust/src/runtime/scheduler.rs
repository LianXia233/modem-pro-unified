//! 后台热插拔巡检。

use std::sync::Arc;
use std::time::Duration;

use crate::modem::ModemManager;

/// 起一个后台线程按 `interval_secs` 周期发现新模组；`interval_secs <= 0` 时不启动。
pub fn spawn_scan(mgr: Arc<ModemManager>, interval_secs: i64) {
    if interval_secs <= 0 {
        log::warn!("scan_interval<=0：不启用后台巡检，仅启动时发现一次模组");
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(interval_secs as u64));
        let added = mgr.discover(false);
        if added > 0 {
            log::info!("巡检发现新增模组 {added} 个");
        }
    });
}

/// 起一个后台线程按 `interval_secs` 周期执行 `tick`；**启动先补跑一轮**。
///
/// 定时动作（锁频段/锁基站）必须到点就执行，不看有没有客户端在线 —— 与配额同一口径。
/// 补跑让"进程重启前几分钟内该做但还没来得及做"的槽位补上（15 分钟窗口内的去重
/// 由台账 `slots` 保证，见 modem/model/schedule.rs 的 `due_phases`）。
pub fn spawn_schedule(interval_secs: i64, tick: impl Fn() + Send + Sync + 'static) {
    if interval_secs <= 0 {
        log::warn!("schedule_interval<=0：不启动定时任务循环，到点动作不会执行");
        return;
    }
    std::thread::spawn(move || {
        tick();
        loop {
            std::thread::sleep(Duration::from_secs(interval_secs as u64));
            tick();
        }
    });
}
