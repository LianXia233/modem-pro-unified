//! AT 口打开失败的冷却闸。
//!
//! ## 为什么需要
//!
//! `with_at_prio` 是惰性打开：谁先来谁负责 `AtPort::open`。串口打不开的原因
//! 往往是**持续几秒到几十秒**的（模组正在重启、USB 重新枚举、驱动还没 bind、
//! 8765 端口的 AT 通道在建），而调用方却有 6~8 个后台循环 + 页面轮询同时在问：
//!
//! ```text
//! 信号循环  ─┐
//! 网络循环  ─┤
//! 流量积分  ─┼─→ 每 200ms 就有一个线程跑去 open("/dev/ttyUSB4") 然后失败
//! SIM 巡检  ─┤
//! 页面轮询  ─┘
//! ```
//!
//! 后果不是「打不开」这么简单：
//!
//! - **日志被刷爆**：几百行 `打开串口失败`，把真正有用的信息埋掉；
//! - **USB 子系统压力**：每次 open 都要走一遍驱动 probe，模组正在重启时反复
//!   open 会拖慢它恢复（实机见过「模组重启后要 40 秒才认到」被这件事拉到 90 秒）；
//! - **CPU 空转**：每次 open 内部有 100ms 级超时，多个线程堆起来就是持续占用。
//!
//! ## 做法
//!
//! 失败后进冷却（默认 5 秒，与上游 `luci-app-fm350` 的
//! `OPEN_RETRY_COOLDOWN` 一致）。冷却期内**直接返回错误**，不去碰串口。
//!
//! ## 为什么不假装成功
//!
//! 冷却期内返回的是**带倒计时的错误**，不是空数据：
//! `AT 口打开失败，4 秒后重试`。页面据此能显示「模组未就绪」而不是
//! 「流量 0 / 信号 0」—— 后者会让用户以为模组坏了或卡死了。
//! 这符合项目红线：**不靠默认值掩盖故障**。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 打开失败后的默认冷却时长。
///
/// 5 秒的依据：模组重启（`AT+CFUN=1,1`）到 AT 口回来通常 20~40 秒，
/// 取 5 秒意味着最多重试 8 次就能重新打开，又不会在失败瞬间狂试。
/// 与上游 `luci-app-fm350` 的 `OPEN_RETRY_COOLDOWN` 一致。
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(5);

/// 打开失败冷却闸。
pub struct OpenCooldown {
    /// 冷却截止时刻；`None` = 未处于冷却。
    until: Mutex<Option<Instant>>,
    cooldown: Duration,
    /// 累计失败次数（排障：`/api/at/sched` 会展示）。
    fails: AtomicU64,
    /// 累计「因冷却被直接拒掉」的次数。
    skipped: AtomicU64,
}

impl Default for OpenCooldown {
    fn default() -> Self {
        Self::new(DEFAULT_COOLDOWN)
    }
}

impl OpenCooldown {
    pub fn new(cooldown: Duration) -> Self {
        Self {
            until: Mutex::new(None),
            cooldown,
            fails: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
        }
    }

    /// 还差多久才能再试？`None` = 现在就可以试。
    ///
    /// 调用方拿到 `Some(left)` 时应当**直接返回错误**（带 `left` 的秒数），
    /// 不要再去 open —— 那正是本模块存在的意义。
    pub fn blocked_for(&self) -> Option<Duration> {
        let g = self.until.lock().unwrap_or_else(|e| e.into_inner());
        let until = (*g)?;
        let now = Instant::now();
        if now >= until {
            return None;
        }
        self.skipped.fetch_add(1, Ordering::Relaxed);
        Some(until - now)
    }

    /// 打开失败：进冷却。
    pub fn on_fail(&self) {
        self.fails.fetch_add(1, Ordering::Relaxed);
        *self.until.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Instant::now() + self.cooldown);
    }

    /// 打开成功：清冷却。
    pub fn on_ok(&self) {
        *self.until.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// 手动清冷却（用户点了「重试」/ 换了一个口时用）。
    pub fn reset(&self) {
        self.on_ok();
    }

    /// 冷却时长。
    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }

    /// `(累计失败次数, 累计被冷却跳过的次数)`。
    pub fn stats(&self) -> (u64, u64) {
        (
            self.fails.load(Ordering::Relaxed),
            self.skipped.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_open() {
        let c = OpenCooldown::default();
        assert!(c.blocked_for().is_none(), "没失败过就该允许尝试");
        assert_eq!(c.stats(), (0, 0));
    }

    #[test]
    fn fail_blocks_for_cooldown() {
        let c = OpenCooldown::new(Duration::from_millis(80));
        c.on_fail();
        let left = c.blocked_for().expect("失败后应处于冷却");
        assert!(
            left <= Duration::from_millis(80),
            "剩余 {left:?} 超了冷却时长"
        );
        assert!(c.blocked_for().is_some(), "冷却期内再来还是被挡");
        assert_eq!(c.stats(), (1, 2), "失败 1 次、被挡 2 次");
    }

    #[test]
    fn cooldown_expires() {
        let c = OpenCooldown::new(Duration::from_millis(30));
        c.on_fail();
        assert!(c.blocked_for().is_some());
        std::thread::sleep(Duration::from_millis(50));
        assert!(c.blocked_for().is_none(), "冷却到点后必须放行");
    }

    #[test]
    fn success_clears_cooldown() {
        // 场景：失败一次 → 冷却 → 时间到了试成功了 → 立刻又要用，不该再等
        let c = OpenCooldown::new(Duration::from_secs(10));
        c.on_fail();
        assert!(c.blocked_for().is_some());
        c.on_ok();
        assert!(c.blocked_for().is_none(), "成功后必须清掉冷却");
    }

    #[test]
    fn reset_clears_cooldown() {
        let c = OpenCooldown::new(Duration::from_secs(10));
        c.on_fail();
        c.reset();
        assert!(c.blocked_for().is_none());
    }

    #[test]
    fn fails_accumulate() {
        let c = OpenCooldown::new(Duration::from_millis(10));
        c.on_fail();
        std::thread::sleep(Duration::from_millis(20));
        c.on_fail();
        assert_eq!(c.stats().0, 2, "失败次数应累计");
    }
}
