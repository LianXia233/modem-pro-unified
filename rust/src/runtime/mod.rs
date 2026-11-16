//! 运行时支撑：PID 文件 / 信号 / 拨号器回收 / 后台巡检。

pub mod dialer;
pub mod hostnic;
pub mod ifstat;
pub mod ntp;
pub mod pid;
pub mod scheduler;
pub mod signals;
pub mod store;
pub mod tz;

use std::time::Duration;

/// 崩溃重启前的退避：正常循环不会退出，`Ok` 只可能是外部要求停止。
const RESTART_BACKOFF: Duration = Duration::from_secs(5);

/// 起一个「崩了也能重来」的后台常驻线程。
///
/// 循环里任何一次 panic（驱动返回意外数据、下标越界、`unwrap` 落空）默认都会让
/// 线程静默消失：采样 / 巡检 / 定时任务从此停摆，只留一行 panic 回溯。这里用
/// `catch_unwind` 兜住，记 error 后退避重启；`body` 正常返回才退出线程。
///
/// 重启不会重置 `body` 捕获的本地缓存（`FnMut` 原样保留），因此游标类状态
/// 最好放在共享结构里；闭包应保证「单次迭代可重入」。
pub fn supervise(name: &'static str, body: impl FnMut() + Send + 'static) {
    supervise_with_backoff(name, RESTART_BACKOFF, body);
}

/// [`supervise`] 的可调退避版本（单测用短退避，生产用 [`RESTART_BACKOFF`]）。
pub fn supervise_with_backoff(
    name: &'static str,
    backoff: Duration,
    mut body: impl FnMut() + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || loop {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body)) {
                Ok(()) => return,
                Err(_) => {
                    log::error!("后台循环 {name} 崩溃，{} 秒后重启", backoff.as_secs_f32());
                    std::thread::sleep(backoff);
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("启动后台循环 {name} 失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::supervise_with_backoff;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn supervise_restarts_after_panic_and_stops_on_return() {
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        supervise_with_backoff("test-supervise", Duration::from_millis(10), move || {
            let n = r.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                panic!("boom {n}");
            }
            // 第三次正常返回：线程应结束，不再有第四次
        });
        // 两次 panic + 一次正常返回 = 3；退避 10ms 足够
        for _ in 0..100 {
            if runs.load(Ordering::SeqCst) >= 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(runs.load(Ordering::SeqCst), 3, "正常返回后不应再重启");
    }
}
