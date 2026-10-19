//! 命令节奏：**每一条** AT 命令之间的硬性延时。
//!
//! ## 它解决的问题
//!
//! 串口是半双工，模组固件收到 `AT+XXX\r` 后要解析、查内部状态、拼应答。
//! 一条命令发出去、应答回来的时间看似「天然有间隔」，但**应答极快的命令**
//! （`AT`、`AT+CFUN?`、`AT+CSQ` 在部分固件上 5ms 就回 `OK`）之间就没有缓冲了。
//! 更糟的是下面两种情况：
//!
//! 1. **一次逻辑操作里连发多条**。读短信要 `AT+CMGF=1` → `AT+CMGL="ALL"` →
//!    `AT+CMGF=0` → `AT+CMGL=4` → `AT+CMGF=1`；读参数要 `AT+CGDCONT?` →
//!    `AT+CGACT?` → `AT+COPS?`。这几条中间原本零间隔，等于「把所有 AT 一起打给模块」。
//! 2. **多条命令都不等应答**（`write_raw` 直写）。
//!
//! 后果是实测过的：应答被截断（只收到半个 `+CSQ: 22,99`）、偶发 `ERROR`、
//! 严重时 AT 口假死到要重启模组。
//!
//! ## 与 `at/sched.rs` 的分工（**别混**）
//!
//! | 层 | 管什么 | 粒度 |
//! | --- | --- | --- |
//! | `sched.rs` | **谁**能用串口（优先级排队）、每窗口最多几次（配额） | 一次「占用」 |
//! | 本模块 | 持串口期间，**每条命令之间**至少隔多久 | 一条「命令」 |
//!
//! 两者正交：调度器让用户手动 AT 插队，本模块保证不管谁在发，都不会连珠炮。
//! 所以 `Limits::gap` 默认是 **0** —— 间隔统一在这里管，避免两处叠加成 120ms。
//!
//! ## 取值
//!
//! [`DEFAULT_GAP`] = 30ms，与上游 `luci-app-fm350` 的 `MIN_CMD_GAP` 一致
//! （同一 FM350 平台实机验证过的值）。依据：
//!
//! - 实机 FM350-GL 上 20ms 间隔仍会零星丢 `OK`，30ms 完全稳定；
//! - 30ms × 一条命令的应答时间（通常 20~200ms）→ 实际节拍 50~230ms，
//!   一个采样循环 5 条命令约 1 秒内跑完，用户在页面上感觉不到慢；
//! - 上限侧：一次 `with_at` 内最多十几条命令（短信列表），30ms 累计 300ms，
//!   相对它本身的几百毫秒耗时可忽略。
//!
//! 所有等待累加进 [`Pacing::throttled`]，`GET /api/at/sched` 的
//! `limits.cmd_gap_ms` 会把它暴露出来，方便对账「为什么这条命令慢了 25ms」。

use std::time::{Duration, Instant};

/// 两条 AT 命令之间的默认最小间隔。
///
/// **30ms 不是随手取的**：同作者的上游项目
/// [`luci-app-fm350`](https://github.com/LianXia233/luci-app-fm350) 的
/// `rust/src/at/mod.rs` 里 `MIN_CMD_GAP = 30ms`，那是长时间实机跑出来的值
/// （20ms 会零星丢 `OK`，30ms 稳定）。本项目的 FM350-GL 是同一平台，
/// 沿用同一个数，省得再踩一遍。
///
/// 语义也照抄上游：等的是**两次「发令时刻」之差**。模组应答本身更慢时
/// （`AT+CGDCONT?` 要 120ms）自然满足，不会把本来就慢的命令拖得更慢 ——
/// 只在命令排得密（连发 3~5 条）时才真正生效。
pub const DEFAULT_GAP: Duration = Duration::from_millis(30);

/// 命令节奏控制器。每个 [`super::AtPort`] 一份（端口生命周期内有效）。
pub struct Pacing {
    gap: Duration,
    /// 上一条命令写完的时刻；`None` = 这个口还没发过命令。
    last: Option<Instant>,
    /// 累计因间隔不足而等待的次数（排障用）。
    throttled: u64,
    /// 累计等待的毫秒数（排障用：能看出「一页数据被延时拖了多久」）。
    waited_ms: u64,
}

impl Default for Pacing {
    fn default() -> Self {
        Self::new(DEFAULT_GAP)
    }
}

impl Pacing {
    pub fn new(gap: Duration) -> Self {
        Self {
            gap,
            last: None,
            throttled: 0,
            waited_ms: 0,
        }
    }

    /// 当前间隔（给接口展示）。
    pub fn gap(&self) -> Duration {
        self.gap
    }

    /// **发命令前调**：距上一条不足 `gap` 就睡够再返回。
    ///
    /// 这是本模块唯一的阻塞点，也是「不要把所有 AT 一起打给模块」的落点。
    /// 调用方不需要知道上一次是什么时候 —— 状态在这个结构里。
    pub fn wait_gap(&mut self) {
        let Some(last) = self.last else {
            return;
        };
        let elapsed = last.elapsed();
        if elapsed >= self.gap {
            return;
        }
        let need = self.gap - elapsed;
        std::thread::sleep(need);
        self.throttled += 1;
        self.waited_ms = self.waited_ms.saturating_add(need.as_millis() as u64);
    }

    /// **命令写完字节之后调**：记录本次发送时刻，供下一条算间隔。
    ///
    /// 刻意在 `write` **之后** mark：串口写本身也要时间（115200 波特率下
    /// 一条 40 字节的命令约 3.5ms），从「写完」算间隔，模组实际收到的
    /// 两条命令间隔才真的是 `gap`，而不是 `gap - 写耗时`。
    pub fn mark_sent(&mut self) {
        self.last = Some(Instant::now());
    }

    /// 累计因间隔不足而等待的次数。
    pub fn throttled(&self) -> u64 {
        self.throttled
    }

    /// 累计等待毫秒数。
    pub fn waited_ms(&self) -> u64 {
        self.waited_ms
    }

    /// 距上一条命令过了多久（毫秒）；没发过就是 `None`。
    pub fn since_last_ms(&self) -> Option<u64> {
        self.last.map(|t| t.elapsed().as_millis() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_command_never_waits() {
        let mut p = Pacing::new(Duration::from_millis(30));
        let t = Instant::now();
        p.wait_gap();
        assert!(t.elapsed() < Duration::from_millis(10), "第一条命令不该等");
        assert_eq!(p.throttled(), 0);
        assert_eq!(p.since_last_ms(), None);
    }

    #[test]
    fn second_command_waits_full_gap() {
        let mut p = Pacing::new(Duration::from_millis(30));
        p.mark_sent();
        let t = Instant::now();
        p.wait_gap();
        assert!(
            t.elapsed() >= Duration::from_millis(28),
            "应至少等 30ms，实际 {:?}",
            t.elapsed()
        );
        assert_eq!(p.throttled(), 1);
    }

    #[test]
    fn no_wait_when_enough_time_passed() {
        let mut p = Pacing::new(Duration::from_millis(20));
        p.mark_sent();
        std::thread::sleep(Duration::from_millis(35));
        let t = Instant::now();
        p.wait_gap();
        assert!(
            t.elapsed() < Duration::from_millis(10),
            "已经隔够了就不该再睡"
        );
        assert_eq!(p.throttled(), 0);
    }

    #[test]
    fn zero_gap_disables_pacing() {
        let mut p = Pacing::new(Duration::ZERO);
        p.mark_sent();
        let t = Instant::now();
        p.wait_gap();
        assert!(t.elapsed() < Duration::from_millis(5));
    }

    #[test]
    fn consecutive_commands_are_all_paced() {
        // 用户报的核心场景：连发 5 条，每条之间都该有间隔，不是只第一条
        let mut p = Pacing::new(Duration::from_millis(15));
        let t = Instant::now();
        for _ in 0..5 {
            p.wait_gap();
            p.mark_sent();
        }
        // 5 条之间有 4 个间隔
        assert!(
            t.elapsed() >= Duration::from_millis(4 * 15 - 5),
            "5 条命令应有 4 个间隔，实际总耗时 {:?}",
            t.elapsed()
        );
        assert!(p.throttled() >= 4);
    }

    #[test]
    fn stats_accumulate() {
        let mut p = Pacing::new(Duration::from_millis(20));
        p.mark_sent();
        p.wait_gap();
        p.mark_sent();
        p.wait_gap();
        assert_eq!(p.throttled(), 2);
        assert!(p.waited_ms() >= 36, "累计等待应记到 {}ms", p.waited_ms());
    }
}
