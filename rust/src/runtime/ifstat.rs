//! 基于 Linux 网络接口统计（sysfs）的实时速率与流量采样。
//!
//! ## 与 `modem/model/traffic.rs` 的分工（别混用）
//!
//! | | 本模块 | `model/traffic.rs` |
//! | --- | --- | --- |
//! | 数据源 | `/sys/class/net/<iface>/statistics/{rx,tx}_bytes` | 模组 AT 计数器（`AT+QGDCNT` / `AT+GTSTATIS`） |
//! | 口径 | **这条数据链路上实际过了多少字节**（含重传、开销） | 模组自己统计的「SIM 卡侧」量 |
//! | 可用性 | 只要有网卡就有 | 依赖厂商支持；**FM350-GL 实测回 ERROR** |
//!
//! 为什么要以本模块为主：实机 FM350-GL 的 `AT+GTSTATIS` 直接 ERROR，于是旧实现
//! `traffic.supported=false`、页面速率与累计全是 0 —— 而 sysfs 里 `eth2` 的
//! rx/tx 一直是准的（238 MB / 65 MB）。AT 那条留作「模组侧对账」，不再作为主数据源。
//!
//! ## 独立性（本轮硬要求）
//!
//! 采样由 [`spawn`] 起的**长期后台线程**驱动，固定 1000ms 一拍，**完全不看有没有
//! 前端在连着**。原先流量循环用 `viewer_active()` 门控（没人看就不采），导致
//! 「关掉页面 → 曲线断档、累计不走」。现在前后端彻底解耦：
//! 前端只读快照，不触发采样。
//!
//! ## 不持久化（有意的取舍）
//!
//! 累计量只在内存里，**每秒写 flash 是禁忌**（OpenWrt 的 flash 寿命经不起）。
//! 所以明确区分三种累计，并在 API 里各自命名，禁止混用：
//!
//! - `iface_sent` / `iface_recv`：**接口生命周期内**累计（等于 sysfs 的绝对值，
//!   接口重建就归零 —— 因为计数器本来就是从 0 开始）；
//! - `session_sent` / `session_recv`：**本进程启动后**累计（跨接口重建仍然连续，
//!   用增量累加得到；进程重启归零）；
//! - 跨重启的累计属于配额台账（`http/quota.rs`）的职责，它有自己的节流落盘策略。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

/// 采样间隔：**固定 1000ms**（用户硬性要求）。
pub const SAMPLE_MS: u64 = 1000;

/// 环形缓冲保留的采样点数：60 个 = 最近 60 秒（默认曲线窗口）。
///
/// 有界是关键：服务跑几个月也不会因为历史无限增长而吃内存。
pub const HISTORY_POINTS: usize = 60;

/// 一个采样点（用于画曲线，字段刻意做小）。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Point {
    /// 采样时刻（Unix 毫秒）。前端按它算相对时间。
    pub t_ms: i64,
    /// 上行速率（bit/s）。
    pub up_bps: u64,
    /// 下行速率（bit/s）。
    pub down_bps: u64,
}

/// 累计量（接口生命周期内 vs 进程启动后）。
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Totals {
    /// 接口自身计数器（等于 sysfs 绝对值；接口重建会归零）。
    pub iface_sent: u64,
    pub iface_recv: u64,
    /// 本进程启动后累计（跨接口重建连续；进程重启归零）。
    pub session_sent: u64,
    pub session_recv: u64,
}

/// 上一拍的原始计数（算差值用）。
#[derive(Debug, Clone, Copy)]
struct Prev {
    at: Instant,
    rx: u64,
    tx: u64,
}

/// 采样器：后台线程写、HTTP 处理器读。
pub struct IfStat {
    /// 当前统计的接口名（空 = 还没识别出来 / 模组不在）。
    ///
    /// 用 `Mutex<String>` 而不是缓存一份：接口可能被重建（`eth2` → `eth3`），
    /// 后台线程会在每拍重新校验并更新，读侧永远看到最新的那个。
    iface: Mutex<String>,
    /// 接口的底层驱动名（`rndis_host` / `qmi_wwan` / `cdc_mbim`…），排障用。
    driver: Mutex<String>,
    /// 速率与累计。
    inner: Mutex<Inner>,
    /// 后台线程是否已启动（防重复 `spawn`）。
    running: AtomicBool,
}

#[derive(Default)]
struct Inner {
    prev: Option<Prev>,
    totals: Totals,
    history: VecDeque<Point>,
    /// 上一拍的速率（接口暂时读不到时沿用它，避免曲线掉到 0 造成「断流」错觉）。
    last_up_bps: u64,
    last_down_bps: u64,
    /// 最近一次成功采样的时刻。
    last_ok_ms: i64,
    /// 连续读失败次数（用于「接口消失 → 重新识别」的判定与日志节流）。
    miss_streak: u32,
}

/// 推进一拍：算速率、更新累计、写环形缓冲。
///
/// **抽成自由函数**（而非 `tick_at` 里的一段）是为了让单测跑**真实逻辑**：
/// 测试不该复制一遍「怎么算差值、怎么裁剪历史」，否则验证的是测试自己的实现。
/// 这里只做纯计算，不碰 IO —— sysfs 读取留给 [`IfStat::tick_at`]。
fn advance(g: &mut Inner, now: Instant, wall_ms: i64, rx: u64, tx: u64) {
    // 接口生命周期内的累计 = sysfs 绝对值
    g.totals.iface_sent = tx;
    g.totals.iface_recv = rx;

    let (up_bps, down_bps) = match g.prev {
        Some(p) => {
            let dt = now.saturating_duration_since(p.at).as_secs_f64();
            // 计数器回退（接口重建 / 驱动重载 / 32 位回绕）：
            // 不能算出巨大的负数，也不该把旧值当增量 —— 直接把这一拍当新基线。
            let rewound = rx < p.rx || tx < p.tx;
            if rewound || dt <= 0.0 {
                (0, 0)
            } else {
                let d_tx = tx - p.tx;
                let d_rx = rx - p.rx;
                g.totals.session_sent = g.totals.session_sent.saturating_add(d_tx);
                g.totals.session_recv = g.totals.session_recv.saturating_add(d_rx);
                // 字节/秒 × 8 = bit/s；用实际 dt 而不是固定 1 秒，
                // 调度延迟（本拍晚了 200ms）不会把速率算虚高。
                (
                    (d_tx as f64 * 8.0 / dt) as u64,
                    (d_rx as f64 * 8.0 / dt) as u64,
                )
            }
        }
        // 第一拍只有基线，没有速率（不能把「开机至今的总量」当成一秒的速度）
        None => (0, 0),
    };
    g.prev = Some(Prev { at: now, rx, tx });
    g.last_up_bps = up_bps;
    g.last_down_bps = down_bps;

    g.history.push_back(Point {
        t_ms: wall_ms,
        up_bps,
        down_bps,
    });
    // 有界：跑几个月也只留 60 个点
    while g.history.len() > HISTORY_POINTS {
        g.history.pop_front();
    }
}

impl Default for IfStat {
    fn default() -> Self {
        Self::new()
    }
}

impl IfStat {
    pub fn new() -> Self {
        Self {
            iface: Mutex::new(String::new()),
            driver: Mutex::new(String::new()),
            inner: Mutex::new(Inner::default()),
            running: AtomicBool::new(false),
        }
    }

    /// 当前接口名（空 = 未识别）。
    pub fn iface(&self) -> String {
        self.iface.lock().unwrap().clone()
    }

    /// 设置/切换统计接口。接口名变化时清空差值基线（否则会算出一个巨大的假速率）。
    ///
    /// 返回是否真的变了（调用方据此决定要不要记日志）。
    pub fn set_iface(&self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let mut cur = self.iface.lock().unwrap();
        if *cur == name {
            return false;
        }
        let old = std::mem::replace(&mut *cur, name.to_string());
        drop(cur);

        // 驱动名：纯排障信息，读不到就留空
        let drv = crate::runtime::hostnic::iface_driver(name);
        *self.driver.lock().unwrap() = drv.clone();

        // 换口 ⇒ 计数器是另一套，基线必须丢掉，否则第一拍会算出
        // 「(新口 rx - 旧口 rx) / 1s」这种天文数字。
        {
            let mut g = self.inner.lock().unwrap();
            g.prev = None;
            g.miss_streak = 0;
            // 历史不清：曲线是「这台模组的链路速率」，换口正是要接着画
        }
        log::info!(
            "流量采样接口：{} → {}（驱动 {}）",
            if old.is_empty() { "(无)" } else { &old },
            name,
            if drv.is_empty() { "未知" } else { &drv }
        );
        true
    }

    /// 读一次 sysfs 计数。
    fn read_counters(iface: &str) -> Option<(u64, u64)> {
        let base = format!("/sys/class/net/{iface}/statistics");
        let rx = std::fs::read_to_string(format!("{base}/rx_bytes"))
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?;
        let tx = std::fs::read_to_string(format!("{base}/tx_bytes"))
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?;
        Some((rx, tx))
    }

    /// 采一拍。`now` 传入是为了单测能控制时间。
    ///
    /// 返回 `true` 表示本拍成功读到了计数。
    pub fn tick_at(&self, now: Instant, wall_ms: i64) -> bool {
        let iface = self.iface();
        if iface.is_empty() {
            return false;
        }
        let Some((rx, tx)) = Self::read_counters(&iface) else {
            let mut g = self.inner.lock().unwrap();
            g.miss_streak = g.miss_streak.saturating_add(1);
            // 连续读不到 ⇒ 接口可能没了（拔模组 / 重建）。只记一次日志，避免刷屏。
            if g.miss_streak == 3 {
                log::debug!("接口 {iface} 连续 3 拍读不到统计（可能已消失）");
            }
            return false;
        };

        let mut g = self.inner.lock().unwrap();
        g.miss_streak = 0;
        g.last_ok_ms = wall_ms;
        advance(&mut g, now, wall_ms, rx, tx);
        true
    }

    /// 采一拍（用当前时刻）。
    pub fn tick(&self) {
        self.tick_at(Instant::now(), crate::logging::unix_now_ms());
    }

    /// 给页面/接口用的快照。
    pub fn view(&self) -> IfStatView {
        let iface = self.iface();
        let driver = self.driver.lock().unwrap().clone();
        let g = self.inner.lock().unwrap();
        let up = g.last_up_bps;
        let down = g.last_down_bps;
        IfStatView {
            iface: iface.clone(),
            driver,
            iface_up: if iface.is_empty() {
                false
            } else {
                // ⚠️ 只排除 `down`，**不要**把 `unknown` 当坏状态：
                // RNDIS / ECM 这类 USB 网卡没有传统链路层，`operstate` 常态就是
                // `unknown`（实机 eth2 正是如此）。早先把它判成「未 UP」，
                // 于是链路明明在跑、页面却挂个「未 UP」的黄标，误导排障。
                std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate"))
                    .map(|s| s.trim() != "down")
                    .unwrap_or(false)
            },
            supported: !iface.is_empty() && g.last_ok_ms > 0,
            // 「算得出速率」= 至少有两个采样点（第一拍只有基线）
            has_rate: g.history.len() >= 2,
            up_bps: up,
            down_bps: down,
            up_text: fmt_bps(up),
            down_text: fmt_bps(down),
            iface_sent: g.totals.iface_sent,
            iface_recv: g.totals.iface_recv,
            session_sent: g.totals.session_sent,
            session_recv: g.totals.session_recv,
            sent: g.totals.session_sent,
            recv: g.totals.session_recv,
            interval_ms: SAMPLE_MS,
            last_sample_ms: g.last_ok_ms,
            history: g.history.iter().copied().collect(),
        }
    }
}

/// `/api/traffic` 的响应体（也内嵌进 `/api/network/status` 的 `traffic`）。
#[derive(Debug, Clone, Serialize)]
pub struct IfStatView {
    /// 统计的接口名（`eth2` / `wwan0_1` …）；空 = 没识别到。
    pub iface: String,
    /// 驱动名（排障用）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub driver: String,
    /// 接口是否 up（读 sysfs `operstate`）。
    pub iface_up: bool,
    /// 是否已经采到过数据。
    pub supported: bool,
    /// 是否已经有过两个采样点（有基线才能算速率）。
    pub has_rate: bool,
    pub up_bps: u64,
    pub down_bps: u64,
    pub up_text: String,
    pub down_text: String,
    pub iface_sent: u64,
    pub iface_recv: u64,
    pub session_sent: u64,
    pub session_recv: u64,
    /// 与旧字段名兼容：等于 [`Self::session_sent`]（本进程启动后累计）。
    ///
    /// 保留是为了让 `overview.js` 里既有的 `traffic.sent` 不用改就能用上
    /// sysfs 口径 —— 前端一行不动，数据源从「AT 计数器（FM350 不支持）」
    /// 换成「网卡实际字节数」。
    pub sent: u64,
    /// 与旧字段名兼容：等于 [`Self::session_recv`]。
    pub recv: u64,
    pub interval_ms: u64,
    pub last_sample_ms: i64,
    /// 最近 60 个点（1 秒一个）。
    pub history: Vec<Point>,
}

/// 起后台采样线程（全局只起一个）。
///
/// **不看有没有前端**：这是用户明确要求的「独立于页面生命周期」。
/// 接口识别交给 `pick` 回调——它每拍都会被调用一次（内部有缓存，开销很小），
/// 这样模组热插拔、接口重建都能自动跟上。
pub fn spawn(stat: Arc<IfStat>, pick: impl Fn() -> Option<String> + Send + 'static) {
    if stat.running.swap(true, Ordering::SeqCst) {
        return; // 已经起过
    }
    std::thread::spawn(move || {
        log::info!("流量采样线程已启动：固定 {SAMPLE_MS}ms 一拍（不依赖前端连接）");
        let mut last_pick = Instant::now() - Duration::from_secs(3600);
        loop {
            let tick_start = Instant::now();
            // 接口每 5 秒重认一次：够快跟上热插拔，又不至于每拍都扫 sysfs
            if last_pick.elapsed() >= Duration::from_secs(5) {
                last_pick = tick_start;
                if let Some(name) = pick() {
                    stat.set_iface(&name);
                }
            }
            stat.tick_at(tick_start, crate::logging::unix_now_ms());

            // 固定节拍：扣掉本拍耗时，避免「1s + 处理时间」越跑越偏
            let spent = tick_start.elapsed();
            std::thread::sleep(Duration::from_millis(SAMPLE_MS).saturating_sub(spent));
        }
    });
}

/// 速率可读文本（`12.34 Mbps`）。
///
/// 单位换算口径（与前端的 `fmt.bpsRate` 保持一致）：
/// `< 1000` → `bps`，`< 1e6` → `Kbps`，其余 → `Mbps`。
pub fn fmt_bps(bps: u64) -> String {
    let v = bps as f64;
    if v >= 1_000_000.0 {
        format!("{:.2} Mbps", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1} Kbps", v / 1_000.0)
    } else {
        format!("{bps} bps")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 喂一拍计数（跳过 sysfs，但**走真实的 `advance` 逻辑**）。
    ///
    /// 刻意不复制一遍「怎么算差值、怎么裁剪历史」：那样测的就是测试自己的实现，
    /// 生产代码改坏了也发现不了。
    fn with_counts(stat: &IfStat, at: Instant, rx: u64, tx: u64) {
        let mut g = stat.inner.lock().unwrap();
        g.last_ok_ms = 1_000;
        advance(&mut g, at, 1_000, rx, tx);
    }

    #[test]
    fn first_sample_has_no_rate() {
        // 第一拍只能建基线：不能把「开机至今总量」当成一秒的速度
        let st = IfStat::new();
        let t0 = Instant::now();
        with_counts(&st, t0, 1_000_000, 500_000);
        let v = st.view();
        assert_eq!(v.up_bps, 0);
        assert_eq!(v.down_bps, 0);
        assert!(!v.has_rate);
        // 但累计要如实反映
        assert_eq!(v.iface_recv, 1_000_000);
        assert_eq!(v.iface_sent, 500_000);
    }

    #[test]
    fn rate_uses_actual_delta_time() {
        // 1 秒内 tx +1000 字节 ⇒ 8000 bit/s
        let st = IfStat::new();
        let t0 = Instant::now();
        with_counts(&st, t0, 10_000, 10_000);
        with_counts(&st, t0 + Duration::from_secs(1), 20_000, 11_000);
        let v = st.view();
        assert_eq!(v.up_bps, 8_000, "tx 增量 1000 字节/1s");
        assert_eq!(v.down_bps, 80_000, "rx 增量 10000 字节/1s");
        // 累计（进程启动后）
        assert_eq!(v.session_sent, 1_000);
        assert_eq!(v.session_recv, 10_000);
    }

    #[test]
    fn delayed_tick_does_not_inflate_rate() {
        // 本拍晚了 2 秒（调度延迟）：速率要按 2 秒算，不能按 1 秒算成两倍
        let st = IfStat::new();
        let t0 = Instant::now();
        with_counts(&st, t0, 0, 0);
        with_counts(&st, t0 + Duration::from_secs(2), 20_000, 2_000);
        let v = st.view();
        assert_eq!(v.down_bps, 80_000, "20000 字节 / 2s = 80 kbps");
        assert_eq!(v.up_bps, 8_000);
    }

    #[test]
    fn counter_rewind_resets_baseline_without_huge_rate() {
        // 接口重建：rx/tx 从 0 重新开始。不能算出负数或天文数字。
        let st = IfStat::new();
        let t0 = Instant::now();
        with_counts(&st, t0, 5_000_000, 5_000_000);
        with_counts(&st, t0 + Duration::from_secs(1), 100, 50); // 回退
        let v = st.view();
        assert_eq!(v.down_bps, 0, "回退这一拍不出速率");
        assert_eq!(v.up_bps, 0);
        // 回退后继续正常计
        with_counts(&st, t0 + Duration::from_secs(2), 1_100, 1_050);
        let v = st.view();
        assert_eq!(v.down_bps, 8_000, "新基线之后的增量照常算");
    }

    #[test]
    fn history_is_bounded() {
        // 环形缓冲必须有界：跑 500 拍也只留 60 个点
        let st = IfStat::new();
        let t0 = Instant::now();
        for i in 0..500u64 {
            with_counts(&st, t0 + Duration::from_millis(i * 1000), i * 1000, i * 500);
        }
        let v = st.view();
        assert_eq!(v.history.len(), HISTORY_POINTS);
        assert_eq!(v.history.last().unwrap().up_bps, 4_000);
    }

    #[test]
    fn fmt_bps_units() {
        assert_eq!(fmt_bps(0), "0 bps");
        assert_eq!(fmt_bps(999), "999 bps");
        assert_eq!(fmt_bps(1_500), "1.5 Kbps");
        assert_eq!(fmt_bps(2_500_000), "2.50 Mbps");
    }

    #[test]
    fn switching_iface_drops_baseline() {
        // 换口（eth2 → eth3）时基线必须丢掉，否则第一拍会算出巨大假速率
        let st = IfStat::new();
        *st.iface.lock().unwrap() = "eth2".to_string();
        st.set_iface("eth3");
        assert!(st.inner.lock().unwrap().prev.is_none());
    }
}
