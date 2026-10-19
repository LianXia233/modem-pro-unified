//! AT 串口访问调度：**优先级排队 + 限流**。
//!
//! ## 为什么需要它
//!
//! 早先一句话就能概括：`at: Mutex<Option<AtPort>>`。所有调用方排同一队、抢同一把锁，
//! 谁先抢到谁先发，**没有优先级、没有间隔、没有配额**。这在单页面手动操作时看不出
//! 问题，但本项目的后台循环会同时跑信号采样、网络巡检、流量积分、SIM 巡检，
//! 加上页面轮询（概览页 3 秒一轮、拨号页 5 秒一轮），叠加起来就是：
//!
//! - **用户点「发送 AT」要排在后台采样后面**：等十几秒才拿到串口，还以为卡死了；
//! - **两条命令之间零间隔**：串口是半双工，模组固件要处理上一条的尾巴，
//!   连续猛灌会出现应答截断、`ERROR`、甚至 AT 口假死（要重启模组才恢复）；
//! - **冷门命令和关键命令同权**：翻一分钟短信列表（几十条命令）会把信号采样饿死，
//!   页面上的信号值就停在那里不动。
//!
//! ## 优先级
//!
//! 数值越小越优先，与 [`Prio`] 的 `Ord` 一致：
//!
//! | 级别 | 谁在用 | 说明 |
//! | --- | --- | --- |
//! | [`Prio::User`] | `/api/at` 手动发指令、用户点「刷新」 | **实时**，不排队等后台 |
//! | [`Prio::Signal`] | 信号/注册态采样（`model/signal.rs`） | 页面最常看，卡了就是「数据不动」 |
//! | [`Prio::Data`] | 拨号取 IP、载波聚合、网络状态、SIM 身份、模组状态 | 功能正确性依赖它 |
//! | [`Prio::Bulk`] | 短信列表、锁网状态、小区扫描、参数读写、USSD… | 冷门且耗时，**尽量给它缓存** |
//!
//! 同级别内部按先来后到（ticket 序号），所以 Bulk 不会被饿死 —— 只要没有
//! 更高优先级的等待者，它就轮到。
//!
//! ## 限流
//!
//! 两道闸，参数见 [`Limits`]：
//!
//! 1. **最小间隔**：上一条命令发出后至少隔 `gap` 才允许下一条。给模组固件留出
//!    处理时间，这是「别把模组打死」最直接的一道。
//! 2. **窗口配额**：任意 `window` 内最多 `per_window` 次。防止某个 Bug（比如某个
//!    循环忘了加节流）把串口灌爆 —— 有配额时最坏也只是变慢，不会不可用。
//!
//! 被闸门拦下的等待会累加进 [`SchedStats::throttled`]，页面能看到「我到底被限了多少次」。
//!
//! ## 锁的顺序
//!
//! `Scheduler::inner` 必须在 `Modem::at` **之前**拿（见 `Modem::with_at_prio`）。
//! 持 `inner` 期间会 sleep（限流），这是有意的：那段时间串口本来就该空着，
//! 让别的线程挤进来只会破坏间隔。等待者此时都在 condvar 上，不占 CPU。

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// 命令优先级（数值越小越优先）。
///
/// 排序语义直接交给 `Ord`：`Vec<(Prio, ticket)>` 的比较就是「先比优先级、
/// 再比 ticket」，正好是想要的调度顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Prio {
    /// 用户手动发 AT（`POST /api/at`）：实时，插到所有人前面。
    User = 0,
    /// 信号 / 注册态采样：页面最常看的那几个值。
    Signal = 1,
    /// 拨号取 IP、载波聚合、网络状态、SIM 身份。
    Data = 2,
    /// 冷门且耗时：短信列表、锁网状态、小区扫描、参数读写。
    Bulk = 3,
}

impl Prio {
    /// 字符串名（进 JSON、进日志）。
    pub fn as_str(self) -> &'static str {
        match self {
            Prio::User => "user",
            Prio::Signal => "signal",
            Prio::Data => "data",
            Prio::Bulk => "bulk",
        }
    }

    pub const ALL: [Prio; 4] = [Prio::User, Prio::Signal, Prio::Data, Prio::Bulk];

    fn idx(self) -> usize {
        self as usize
    }
}

/// 限流参数。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// **两次「占用串口」之间**的最小间隔。
    ///
    /// ⚠️ 默认 **0**：真正的「每条命令之间 25ms」由 `at/pacing.rs` 负责，
    /// 这里再加一层就变成 50ms 叠加，白白拖慢。
    /// 只有想让「换了一个占用者」也额外冷一下时，才把它调大。
    pub gap: Duration,
    /// 配额窗口长度。
    pub window: Duration,
    /// 一个窗口内最多几次。
    pub per_window: u32,
}

impl Default for Limits {
    fn default() -> Self {
        // 取值依据（宁保守不激进，出问题只会「慢」不会「坏」）：
        //
        // · gap = 0：**命令之间的间隔交给 `at/pacing.rs`（每条 25ms）**。
        //   早先这里放 60ms，但它只在「取得串口时」生效，闭包内连发多条
        //   依然零间隔 —— 那正是「把所有 AT 一起打给模块」的漏洞。
        //   现在把间隔下沉到命令层，这里就不再重复计时。
        // · 每 10 秒最多 120 次（12 次/秒）：**实机量出来的**。
        //
        //   早先设 40/10s（4 次/秒），结果在 FM350-GL 上排起了长队：
        //   5 分钟内 signal 等 54 秒、data 等 64 秒、用户手动 AT 等了 2.7 秒，
        //   最长一次 7.7 秒 —— 页面上的信号值就「不动」了。
        //   实测后台 8 个循环 + 页面轮询的自然需求是 4~6 次/秒，
        //   120/10s 给到两倍余量，同时仍能挡住「忘了加节流的循环」
        //   （那种是每秒几十次）。
        //
        //   真正的「别打死模组」由 `at/pacing.rs` 的 30ms/条负责；
        //   这里只是防失控的兜底，不该成为常态瓶颈。
        Self {
            gap: Duration::ZERO,
            window: Duration::from_secs(10),
            per_window: 120,
        }
    }
}

/// 一个优先级的统计。
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct PrioStat {
    /// 该级别累计拿到串口的次数。
    pub served: u64,
    /// 该级别累计等待时长（毫秒，含限流等待）。
    pub wait_ms: u64,
    /// 其中被限流闸门拦住的次数。
    pub throttled: u64,
}

/// 调度统计（`GET /api/at/sched` 用，排障看「到底谁在抢串口」）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SchedStats {
    pub user: PrioStat,
    pub signal: PrioStat,
    pub data: PrioStat,
    pub bulk: PrioStat,
    /// 此刻正在等串口的线程数（含持票者之外的）。
    pub waiting: usize,
    /// 串口当前是否被占着。
    pub busy: bool,
    /// 历史最长单次等待（毫秒）。
    pub max_wait_ms: u64,
}

struct State {
    /// 当前持票者的 ticket；`None` = 串口空闲。
    owner: Option<u64>,
    next_ticket: u64,
    /// 等待队列：(优先级, ticket)，**不保证有序**，取用时线性挑最优。
    ///
    /// 用 `Vec` 而不是 `BinaryHeap`：等待者通常 0~3 个，线性扫描比堆更省事，
    /// 也更容易在 `retain` 时清理被 panic 丢下的僵尸条目。
    waiters: Vec<(Prio, u64)>,
    /// 最近发放配额的时刻（滑动窗口）。
    sent: VecDeque<Instant>,
    stats: [PrioStat; 4],
    max_wait_ms: u64,
}

impl State {
    /// 我是不是当前最该拿串口的人。
    ///
    /// 条件：串口空闲 + 没有比我更优先的等待者。同优先级靠 ticket 比先后，
    /// 所以 Bulk 也不会被同级的新来者插队。
    fn my_turn(&self, mine: (Prio, u64)) -> bool {
        self.owner.is_none() && !self.waiters.iter().any(|&w| w < mine)
    }
}

/// 调度器。每个 `Modem` 一份（各自一把串口，互不影响）。
pub struct Scheduler {
    inner: Mutex<State>,
    cv: Condvar,
    limits: Limits,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Scheduler {
    pub fn new(limits: Limits) -> Self {
        Self {
            inner: Mutex::new(State {
                owner: None,
                next_ticket: 0,
                waiters: Vec::new(),
                sent: VecDeque::new(),
                stats: Default::default(),
                max_wait_ms: 0,
            }),
            cv: Condvar::new(),
            limits,
        }
    }

    /// 排队拿串口。返回的 [`Ticket`] drop 时自动放行下一位。
    ///
    /// 拿不到就一直等 —— 没有超时。理由：AT 命令自身有超时（`command_timeout`），
    /// 队列再叠一层超时会让「到底哪一层超了」变得难以排查；真出问题看
    /// [`Self::stats`] 里的 `waiting` 与 `max_wait_ms` 更直接。
    pub fn acquire(&self, prio: Prio) -> Ticket<'_> {
        let start = Instant::now();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let ticket = g.next_ticket;
        g.next_ticket = g.next_ticket.wrapping_add(1);
        let mine = (prio, ticket);
        g.waiters.push(mine);

        while !g.my_turn(mine) {
            // wait_timeout 而不是 wait：即使漏了一次 notify（panic 路径），
            // 200ms 后也会自己重查条件，不会永久卡死。
            let (ng, _) = self
                .cv
                .wait_timeout(g, Duration::from_millis(200))
                .unwrap_or_else(|e| e.into_inner());
            g = ng;
        }

        g.owner = Some(ticket);
        g.waiters.retain(|&w| w != mine);

        // 拿到串口之后再限流：闸门衡量的是「真正发命令的节奏」，
        // 不是「谁先排到队」。放前面会让限流等待也占用 owner，别人白等。
        let throttled = self.throttle(&mut g, prio);

        let wait = start.elapsed().as_millis() as u64;
        let s = &mut g.stats[prio.idx()];
        s.served += 1;
        s.wait_ms = s.wait_ms.saturating_add(wait);
        if throttled {
            s.throttled += 1;
        }
        if wait > g.max_wait_ms {
            g.max_wait_ms = wait;
        }

        Ticket {
            sched: self,
            ticket,
        }
    }

    /// 闸门：最小间隔 + 窗口配额。返回是否真的等过。
    ///
    /// 持 `inner` 锁调用（见模块注释：那段时间串口本就该空着）。
    ///
    /// **[`Prio::User`] 不参与窗口配额**：配额是防「程序失控把串口灌爆」的，
    /// 不是限制人。实机量到用户手动 AT 被配额挡了 2.7 秒、最长 7.7 秒 ——
    /// 用户点一下要等十秒，那这个功能就没法用了。用户自己狂点的后果由
    /// 30ms 命令间隔兜住（最多 33 次/秒，且每次都真实等应答）。
    fn throttle(&self, g: &mut State, prio: Prio) -> bool {
        let mut waited = false;

        // ① 占用之间的兜底间隔（默认 0 = 关；命令级间隔由 pacing 负责）
        if !self.limits.gap.is_zero() {
            if let Some(&last) = g.sent.back() {
                let elapsed = last.elapsed();
                if elapsed < self.limits.gap {
                    std::thread::sleep(self.limits.gap - elapsed);
                    waited = true;
                }
            }
        }

        // ② 窗口配额：清掉滑出窗口的记录后若仍满，就等到最老那条过期。
        //
        //    `User` 跳过这一条：配额防的是「程序失控」，不是限制人。
        //    但上面的 ① 间隔它照样遵守 —— 那是物理限制（半双工串口），
        //    跟「是不是人点的」无关。
        if prio == Prio::User {
            g.sent.push_back(Instant::now());
            self.evict(g);
            return waited;
        }
        self.evict(g);
        if g.sent.len() >= self.limits.per_window as usize {
            if let Some(&oldest) = g.sent.front() {
                let held = oldest.elapsed();
                if held < self.limits.window {
                    std::thread::sleep(self.limits.window - held);
                    waited = true;
                }
            }
            self.evict(g);
        }

        g.sent.push_back(Instant::now());
        waited
    }

    fn evict(&self, g: &mut State) {
        let now = Instant::now();
        while let Some(&t) = g.sent.front() {
            // 用 elapsed 而不是 `now - t`：Instant 不保证能相减出负数
            if t.elapsed() >= self.limits.window {
                g.sent.pop_front();
            } else {
                break;
            }
        }
        let _ = now;
    }

    /// 快照统计（只读，用 `try_lock` 避免和持锁 sleep 的限流打架）。
    pub fn stats(&self) -> SchedStats {
        let g = match self.inner.try_lock() {
            Ok(g) => g,
            // 正在限流 sleep 中：返回「忙」而不是阻塞调用方（这是给 HTTP 接口用的，
            // 卡在这里会让页面等 10 秒）。拿不到就如实说现在忙。
            Err(_) => {
                return SchedStats {
                    waiting: usize::MAX,
                    busy: true,
                    ..Default::default()
                }
            }
        };
        SchedStats {
            user: g.stats[Prio::User.idx()],
            signal: g.stats[Prio::Signal.idx()],
            data: g.stats[Prio::Data.idx()],
            bulk: g.stats[Prio::Bulk.idx()],
            waiting: g.waiters.len(),
            busy: g.owner.is_some(),
            max_wait_ms: g.max_wait_ms,
        }
    }

    /// 当前限流参数（给接口展示）。
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// 立刻放行下一位（panic / 端口重建时用；正常路径靠 `Ticket` 的 Drop）。
    fn release(&self, ticket: u64) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.owner == Some(ticket) {
            g.owner = None;
            drop(g);
            self.cv.notify_all();
        }
    }
}

/// 占着串口的凭证。drop = 放行。
pub struct Ticket<'a> {
    sched: &'a Scheduler,
    ticket: u64,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        self.sched.release(self.ticket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched() -> Scheduler {
        // 测试里把限流关掉，免得每个用例白等 60ms
        Scheduler::new(Limits {
            gap: Duration::ZERO,
            window: Duration::from_secs(10),
            per_window: u32::MAX,
        })
    }

    #[test]
    fn exclusive_access() {
        let s = sched();
        let t = s.acquire(Prio::Data);
        assert!(s.stats().busy);
        drop(t);
        assert!(!s.stats().busy);
    }

    #[test]
    fn ticket_released_on_panic() {
        // drop 语义必须扛住 panic：后台线程一发 panic，串口不能永久锁死。
        let s = sched();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _t = s.acquire(Prio::Bulk);
            panic!("boom");
        }));
        assert!(r.is_err());
        // 还能再拿到 → 说明 panic 时 Ticket 被正确 drop 了
        let _t2 = s.acquire(Prio::Signal);
    }

    #[test]
    fn higher_priority_goes_first() {
        // 场景：Bulk 持票，Data 与 User 先后排队（User 后到）。
        // 期望：Bulk 放行后 **User** 先拿，Data 其次 —— 优先级压过先来后到。
        let s = std::sync::Arc::new(sched());
        let order = std::sync::Arc::new(Mutex::new(Vec::new()));

        let held = s.acquire(Prio::Bulk);

        let mut hs = Vec::new();
        for (prio, delay_ms) in [(Prio::Data, 0u64), (Prio::User, 30)] {
            let s2 = s.clone();
            let o2 = order.clone();
            hs.push(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(delay_ms));
                let _t = s2.acquire(prio);
                o2.lock().unwrap().push(prio);
            }));
        }
        // 等两个线程都排上（Data 先到、User 后到，但 User 该先拿）
        std::thread::sleep(Duration::from_millis(120));
        drop(held);

        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(
            *order.lock().unwrap(),
            vec![Prio::User, Prio::Data],
            "用户手动命令必须插到后台数据命令前面"
        );
    }

    #[test]
    fn same_priority_is_fifo() {
        // 同级别不能被后来的插队（否则低优先级会饿死）
        let s = std::sync::Arc::new(sched());
        let order = std::sync::Arc::new(Mutex::new(Vec::new()));
        let held = s.acquire(Prio::Data);

        let mut hs = Vec::new();
        for i in 0..3u32 {
            let s2 = s.clone();
            let o2 = order.clone();
            hs.push(std::thread::spawn(move || {
                // 保证入队顺序：后一个等前一个排上
                std::thread::sleep(Duration::from_millis(20 * i as u64));
                let _t = s2.acquire(Prio::Bulk);
                o2.lock().unwrap().push(i);
            }));
        }
        std::thread::sleep(Duration::from_millis(120));
        drop(held);
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn gap_is_enforced() {
        let s = Scheduler::new(Limits {
            gap: Duration::from_millis(50),
            window: Duration::from_secs(10),
            per_window: u32::MAX,
        });
        let t0 = Instant::now();
        drop(s.acquire(Prio::User));
        drop(s.acquire(Prio::User));
        // 第二次至少要等 50ms
        assert!(
            t0.elapsed() >= Duration::from_millis(50),
            "两条命令之间的最小间隔没生效：{:?}",
            t0.elapsed()
        );
        assert_eq!(s.stats().user.throttled, 1);
    }

    #[test]
    fn window_quota_is_enforced() {
        let s = Scheduler::new(Limits {
            gap: Duration::ZERO,
            window: Duration::from_millis(400),
            per_window: 2,
        });
        let t0 = Instant::now();
        for _ in 0..3 {
            drop(s.acquire(Prio::Signal));
        }
        // 第 3 次要等第 1 次滑出 400ms 窗口
        assert!(
            t0.elapsed() >= Duration::from_millis(380),
            "窗口配额没生效：{:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn user_bypasses_window_quota() {
        // 实机教训：配额把用户手动 AT 挡了 2.7 秒、最长 7.7 秒。
        // 配额是防「程序失控」的，不是限制人 —— 用户必须能立刻发。
        let s = Scheduler::new(Limits {
            gap: Duration::ZERO,
            window: Duration::from_secs(10),
            per_window: 1, // 极紧的配额
        });
        drop(s.acquire(Prio::Signal)); // 用掉唯一的配额
        let t = Instant::now();
        // 配额已满，但 User 不该等
        drop(s.acquire(Prio::User));
        assert!(
            t.elapsed() < Duration::from_millis(200),
            "用户手动 AT 被配额挡住了 {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn backend_still_obeys_quota() {
        // 反过来：后台（Signal/Data/Bulk）必须老实排队，否则配额形同虚设
        let s = Scheduler::new(Limits {
            gap: Duration::ZERO,
            window: Duration::from_millis(300),
            per_window: 1,
        });
        drop(s.acquire(Prio::Signal));
        let t = Instant::now();
        drop(s.acquire(Prio::Data));
        assert!(
            t.elapsed() >= Duration::from_millis(250),
            "后台命令应等配额窗口滑过，实际只等了 {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn stats_count_per_priority() {
        let s = sched();
        drop(s.acquire(Prio::User));
        drop(s.acquire(Prio::Signal));
        drop(s.acquire(Prio::Signal));
        let st = s.stats();
        assert_eq!(st.user.served, 1);
        assert_eq!(st.signal.served, 2);
        assert_eq!(st.data.served, 0);
        assert!(!st.busy);
    }
}
