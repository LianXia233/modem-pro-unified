//! NTP 时间同步（SNTP 客户端 + 插件级时钟校准）。
//!
//! ## 为什么自己实现
//!
//! 路由器上通常有 `ntpd` / `chronyd`，但很多固件把它们裁了，或者只在 WAN 起来后
//! 才跑 —— 而本插件在**开机后几秒**就要判定定时任务、写日志、算配额周期。
//! 所以这里自带一个最小 SNTP 客户端（UDP 123，48 字节报文，无依赖）。
//!
//! ## 校准语义
//!
//! 同步成功后把 `offset = 服务器时间 − 本地系统时间` 写进
//! `logging::time::set_clock_offset()`；`logging::time::unix_now()` 全插件共用，
//! 所以**日志、定时任务、配额周期、波动记录、接口时间戳一起变准**，不用逐处改。
//!
//! **不改系统时钟**（`settimeofday` 要 root 且会影响别的进程）。这样也有个好处：
//! 同步一直不成功时 offset 保持 0，时间自然回退到「设备时间」，正是要求的降级路径。
//!
//! ## 重试策略（与需求一一对应）
//!
//! ```text
//! 启动 ──3s──▶ 尝试 ──成功──▶ 稳定期（20 分钟一次）
//!                │失败
//!                ▼
//!         每 3s 重试，最多 10 次
//!                │仍失败
//!                ▼
//!         每 60s 重试，最多 10 次
//!                │仍失败
//!                ▼
//!         常驻：每 20 分钟一次（不再加速）
//! ```
//!
//! 一个周期内会**依次尝试配置里的所有服务器**（腾讯 / Cloudflare / 阿里），
//! 全部失败才算「这一次失败」。

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::logging::{system_now, unix_now};

/// NTP 纪元（1900-01-01）到 Unix 纪元（1970-01-01）的秒数。
const NTP_UNIX_OFFSET: f64 = 2_208_988_800.0;
/// 报文固定 48 字节（不带认证）。
const PACKET: usize = 48;
/// 启动后首次尝试的等待（需求：启动时 3 秒同步一次）。
const FIRST_DELAY: i64 = 3;
/// 阶段 1：3 秒一次，最多 10 次。
const FAST_INTERVAL: i64 = 3;
const FAST_TRIES: u32 = 10;
/// 阶段 2：60 秒一次，最多 10 次。
const SLOW_INTERVAL: i64 = 60;
const SLOW_TRIES: u32 = 10;
/// 阶段 3 / 常态：20 分钟一次。
const STEADY_INTERVAL: i64 = 1200;

/// 一次成功采样的结果。
#[derive(Debug, Clone)]
pub struct Sample {
    /// 服务器时间 − 本地系统时间（秒，四舍五入）。
    pub offset_secs: i64,
    /// 往返时延（毫秒），用来判断这次采样可不可信。
    pub delay_ms: u64,
    pub server: String,
}

/// 同步阶段（对外展示 + 日志用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 启动后的快节奏重试（3 秒）。
    Fast,
    /// 退避到 1 分钟。
    Slow,
    /// 稳定期（20 分钟）。
    Steady,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Fast => "fast",
            Phase::Slow => "slow",
            Phase::Steady => "steady",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Phase::Fast => "启动重试（每 3 秒）",
            Phase::Slow => "退避重试（每 1 分钟）",
            Phase::Steady => "常态（每 20 分钟）",
        }
    }
}

/// 对外暴露的状态（`/api/ntp`）。
#[derive(Debug, Clone)]
pub struct Status {
    pub enabled: bool,
    /// 是否至少成功同步过一次。
    pub synced: bool,
    /// 当前时间来源：`ntp`（已校准）/ `system`（回退设备时间）。
    pub source: &'static str,
    /// 上次成功的服务器。
    pub server: String,
    /// 当前生效的校准偏移（秒）。
    pub offset_secs: i64,
    /// 上次采样的往返时延。
    pub delay_ms: u64,
    /// 上次成功时刻（RFC3339，UTC）。
    pub last_ok_at: String,
    /// 上次尝试时刻。
    pub last_try_at: String,
    pub phase: Phase,
    /// 本阶段已失败次数。
    pub attempts: u32,
    /// 下次尝试还有多少秒（0 = 正在试）。
    pub next_in_secs: i64,
    /// 最近一次错误（成功后清空）。
    pub last_error: String,
    pub ok_count: u64,
    pub fail_count: u64,
    pub servers: Vec<String>,
    /// 配置里的时区写法（原样回显，页面「改时区」拿它当默认值）。
    pub timezone: String,
}

impl Status {
    fn new(enabled: bool, servers: Vec<String>, timezone: &str) -> Self {
        Self {
            enabled,
            synced: false,
            source: "system",
            server: String::new(),
            offset_secs: 0,
            delay_ms: 0,
            last_ok_at: String::new(),
            last_try_at: String::new(),
            phase: Phase::Fast,
            attempts: 0,
            next_in_secs: FIRST_DELAY,
            last_error: String::new(),
            ok_count: 0,
            fail_count: 0,
            servers,
            timezone: timezone.to_string(),
        }
    }

    /// JSON 形状（`/api/ntp` 与 WS 推送共用）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.enabled,
            "synced": self.synced,
            "source": self.source,
            "server": self.server,
            "offset_secs": self.offset_secs,
            "delay_ms": self.delay_ms,
            "last_ok_at": self.last_ok_at,
            "last_try_at": self.last_try_at,
            "phase": self.phase.as_str(),
            "phase_label": self.phase.label(),
            "attempts": self.attempts,
            "next_in_secs": self.next_in_secs,
            "last_error": self.last_error,
            "ok_count": self.ok_count,
            "fail_count": self.fail_count,
            "servers": self.servers,
            "timezone": self.timezone,
            // 给页面直接显示「现在几点」（本地时区，已含校准）
            "now": crate::logging::rfc3339(unix_now()),
            "now_local": format_local_now(),
        })
    }
}

/// 本地时间的一行可读形态（`2026-10-19 05:12:33 +0800`）。
fn format_local_now() -> String {
    let local = crate::logging::local_unix_now();
    let off = crate::logging::tz_offset_secs();
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.abs();
    let utc_text = crate::logging::rfc3339(local).replace('T', " ");
    format!(
        "{} {}{:02}:{:02}",
        utc_text.trim_end_matches('Z'),
        sign,
        a / 3600,
        (a % 3600) / 60
    )
}

/// 全局状态 + 强制同步开关（进程内单例）。
static STATE: Mutex<Option<Arc<Mutex<Status>>>> = Mutex::new(None);
/// `POST /api/ntp/sync` 把它置 true，循环下一拍立即执行。
static FORCE: AtomicBool = AtomicBool::new(false);

/// 取全局状态（未启动时返回一个「未启用」的占位）。
pub fn state() -> Arc<Mutex<Status>> {
    let mut g = STATE.lock().unwrap();
    g.get_or_insert_with(|| Arc::new(Mutex::new(Status::new(false, Vec::new(), ""))))
        .clone()
}

/// 请求立即同步一次（不等下一个节拍）。
pub fn request_sync() {
    FORCE.store(true, Ordering::Relaxed);
}

// ---------- SNTP ----------

fn u32_be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// 8 字节 NTP 时间戳 → Unix 秒（f64，含小数）。
fn ts_to_secs(b: &[u8]) -> f64 {
    let secs = u32_be(&b[0..4]) as f64;
    let frac = u32_be(&b[4..8]) as f64 / 4_294_967_296.0;
    secs + frac - NTP_UNIX_OFFSET
}

/// 把 Unix 秒（f64）写进 8 字节 NTP 时间戳。
fn secs_to_ts(unix: f64, out: &mut [u8]) {
    let ntp = unix + NTP_UNIX_OFFSET;
    let secs = ntp.floor();
    let frac = ((ntp - secs) * 4_294_967_296.0) as u64;
    out[0..4].copy_from_slice(&(secs as u32).to_be_bytes());
    out[4..8].copy_from_slice(&(frac as u32).to_be_bytes());
}

/// 向一台服务器发一次 SNTP 查询。
///
/// `server` 可以是域名（`ntp.tencent.com`）或 IP；域名解析交给 `ToSocketAddrs`。
pub fn query_one(server: &str, timeout: Duration) -> Result<Sample, String> {
    let host = server.trim();
    if host.is_empty() {
        return Err("服务器名是空的".into());
    }
    let addr = (host, 123u16)
        .to_socket_addrs_safe()
        .map_err(|e| format!("解析 {host} 失败: {e}"))?;
    query_addr(addr, host, timeout)
}

/// 对一个已解析的地址发查询（测试用它打本地假服务器；生产走 [`query_one`]）。
pub(crate) fn query_addr(
    addr: std::net::SocketAddr,
    host: &str,
    timeout: Duration,
) -> Result<Sample, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("创建 UDP 套接字失败: {e}"))?;
    sock.set_read_timeout(Some(timeout))
        .map_err(|e| format!("设置超时失败: {e}"))?;
    sock.connect(addr)
        .map_err(|e| format!("连接 {host} 失败: {e}"))?;

    // T1：本机发送时刻（**未校准**的系统时间 —— 用校准时间会自激）
    let t1 = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("读系统时间失败: {e}"))?;
        now.as_secs_f64()
    };
    let mut req = [0u8; PACKET];
    // LI=0（无闰秒警告）、VN=3、Mode=3（client）
    req[0] = 0x1B;
    secs_to_ts(t1, &mut req[40..48]);

    sock.send(&req)
        .map_err(|e| format!("{host} 发送失败: {e}"))?;
    let mut resp = [0u8; PACKET];
    let n = sock.recv(&mut resp).map_err(|e| match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            format!("{host} 无应答（超时 {}s）", timeout.as_secs())
        }
        _ => format!("{host} 接收失败: {e}"),
    })?;
    if n < PACKET {
        return Err(format!("{host} 回了短包（{n} 字节，至少要 {PACKET}）"));
    }
    let t4 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("读系统时间失败: {e}"))?
        .as_secs_f64();

    // 模式 4 = server；stratum 0 = kiss-o'-death（服务器拒绝服务）
    let mode = resp[0] & 0x07;
    if mode != 4 {
        return Err(format!("{host} 回的报文 mode={mode}（期望 4=server）"));
    }
    if resp[1] == 0 {
        let code = String::from_utf8_lossy(&resp[12..16]).to_string();
        return Err(format!("{host} 返回 kiss-o'-death（{code}），换一台再试"));
    }
    // originate 必须回显我们发的 T1，否则是串包/伪造
    let originate = ts_to_secs(&resp[24..32]);
    if (originate - t1).abs() > 1.0 {
        return Err(format!("{host} 的 originate 时间戳对不上（串包？）"));
    }
    let t2 = ts_to_secs(&resp[32..40]); // 服务器收到
    let t3 = ts_to_secs(&resp[40..48]); // 服务器发出

    let offset = ((t2 - t1) + (t3 - t4)) / 2.0;
    let delay = ((t4 - t1) - (t3 - t2)).max(0.0);
    if !offset.is_finite() || !delay.is_finite() {
        return Err(format!("{host} 的时间戳无法解析"));
    }
    // 时延过大（>5s）说明链路抖得厉害，这次采样不可信
    if delay > 5.0 {
        return Err(format!(
            "{host} 往返时延 {:.0}ms 过大，本次采样不可信",
            delay * 1000.0
        ));
    }
    Ok(Sample {
        offset_secs: offset.round() as i64,
        delay_ms: (delay * 1000.0).round() as u64,
        server: host.to_string(),
    })
}

/// 依次尝试所有服务器，返回第一个成功的采样。
pub fn query_any(servers: &[String], timeout: Duration) -> Result<Sample, String> {
    let mut errs = Vec::new();
    for s in servers {
        match query_one(s, timeout) {
            Ok(sample) => return Ok(sample),
            Err(e) => errs.push(e),
        }
    }
    Err(if errs.is_empty() {
        "没有配置任何 NTP 服务器".to_string()
    } else {
        errs.join("；")
    })
}

// ---------- 时区 ----------

/// IANA 时区名 → 固定偏移（秒，东为正）。
///
/// ⚠️ 只做**固定偏移**：不套 DST 规则。中国（`Asia/Shanghai` 等）本来就没有夏令时，
/// 家用路由器绝大多数也在这个时区；欧美时区在夏令时期间会差 1 小时 ——
/// 这是刻意取舍（要正确 DST 就得内嵌 tzdata，体积与维护成本都不划算）。
/// 需要精确 DST 的用 `TZ` 环境变量交给系统 libc。
pub fn iana_offset(name: &str) -> Option<i64> {
    let n = name.trim().to_ascii_lowercase();
    // `Asia / Shanghai` 这类手滑加空格的也兜住（`Hong_Kong` 的下划线要**保留**：
    // IANA 名里下划线就是分隔符，换成斜杠反而查不到）
    let n = n.split('/').map(|s| s.trim()).collect::<Vec<_>>().join("/");
    // 常见别名先规整
    let canon = match n.as_str() {
        "asia/shanghai" | "asia/chongqing" | "asia/harbin" | "asia/urumqi" | "prc" => {
            "asia/shanghai"
        }
        "asia/hong_kong" | "hongkong" => "asia/hong_kong",
        "asia/macau" | "asia/macao" => "asia/macau",
        "asia/taipei" | "roc" => "asia/taipei",
        "asia/singapore" | "singapore" => "asia/singapore",
        "asia/tokyo" | "japan" => "asia/tokyo",
        "asia/seoul" | "rok" => "asia/seoul",
        "asia/kolkata" | "asia/calcutta" => "asia/kolkata",
        "utc" | "etc/utc" | "gmt" | "etc/gmt" | "zulu" | "universal" => "utc",
        other => other,
    };
    let hours = match canon {
        "utc" => 0,
        "asia/shanghai" | "asia/hong_kong" | "asia/macau" | "asia/taipei" | "asia/singapore"
        | "asia/manila" | "asia/kuala_lumpur" | "asia/brunei" | "australia/perth" => 8,
        "asia/tokyo" | "asia/seoul" | "asia/pyongyang" => 9,
        "asia/bangkok" | "asia/jakarta" | "asia/ho_chi_minh" | "asia/phnom_penh"
        | "asia/vientiane" | "asia/yangon" => 7,
        "asia/dhaka" => 6,
        "asia/kathmandu" => 5, // 5:45 的近似（见下方特殊处理）
        "asia/kolkata" | "asia/colombo" => 5,
        "asia/karachi" | "asia/tashkent" | "asia/dushanbe" => 5,
        "asia/dubai" | "asia/muscat" | "asia/baku" | "asia/tbilisi" | "asia/yerevan" => 4,
        "asia/tehran" => 3,
        "asia/baghdad" | "asia/riyadh" | "asia/kuwait" | "asia/qatar" | "asia/nairobi"
        | "africa/nairobi" | "europe/moscow" | "europe/istanbul" | "europe/minsk" => 3,
        "asia/amman" | "asia/beirut" | "asia/damascus" | "asia/jerusalem" | "africa/cairo"
        | "europe/athens" | "europe/helsinki" | "europe/kiev" | "europe/bucharest" => 2,
        "europe/london" | "europe/dublin" | "europe/lisbon" | "africa/lagos" => 0,
        "europe/paris" | "europe/berlin" | "europe/madrid" | "europe/rome" | "europe/amsterdam"
        | "europe/brussels" | "europe/vienna" | "europe/zurich" | "europe/stockholm"
        | "europe/oslo" | "europe/copenhagen" | "europe/prague" | "europe/warsaw"
        | "europe/budapest" | "africa/algiers" => 1,
        "america/sao_paulo" | "america/buenos_aires" | "america/montevideo" => -3,
        "america/new_york" | "america/toronto" | "america/miami" | "america/bogota"
        | "america/lima" | "america/panama" => -5,
        "america/chicago" | "america/mexico_city" | "america/winnipeg" => -6,
        "america/denver" | "america/phoenix" | "america/edmonton" => -7,
        "america/los_angeles" | "america/vancouver" | "america/seattle" => -8,
        "america/anchorage" => -9,
        "pacific/honolulu" => -10,
        "australia/sydney"
        | "australia/melbourne"
        | "australia/brisbane"
        | "australia/canberra"
        | "australia/hobart" => 10,
        "australia/adelaide" | "australia/darwin" => 9,
        "pacific/auckland" | "pacific/fiji" => 12,
        _ => return None,
    };
    let mut secs = hours * 3600;
    // 半小时 / 45 分钟时区的例外
    match canon {
        "asia/kolkata" | "asia/colombo" => secs += 1800,
        "asia/kathmandu" => secs += 2700,
        "asia/yangon" => secs += 1800,
        "australia/adelaide" | "australia/darwin" => secs += 1800,
        _ => {}
    }
    Some(secs)
}

/// 解析配置里的时区写法：IANA 名 / POSIX（`CST-8`）都认。
pub fn parse_timezone(spec: &str) -> Option<i64> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    // `:Asia/Shanghai`（带冒号的 TZif 引用）也接受，去掉冒号后按 IANA 查
    let s = s.strip_prefix(':').unwrap_or(s);
    if s.contains('/') {
        return iana_offset(s);
    }
    // `UTC+08:00` / `GMT-5` 这类：交给 POSIX 解析（它按 POSIX 西为正再翻转）
    crate::logging::parse_posix_tz_offset(s).or_else(|| iana_offset(s))
}

// ---------- 后台循环 ----------

/// 启动后台同步线程（`enabled=false` 时只写一条日志并返回）。
///
/// `on_change` 在每次状态变化后调用（推送 WS 用），避免 runtime 反向依赖 http。
pub fn spawn<F>(cfg: crate::config::NtpConfig, on_change: F)
where
    F: Fn(&Status) + Send + Sync + 'static,
{
    let servers = cfg.normalized_servers();
    let st = Arc::new(Mutex::new(Status::new(
        cfg.enabled,
        servers.clone(),
        &cfg.timezone,
    )));
    {
        let mut g = STATE.lock().unwrap();
        *g = Some(st.clone());
    }
    // 配置里的时区优先于 TZ / /etc/TZ
    if let Some(off) = parse_timezone(&cfg.timezone) {
        crate::logging::set_configured_tz(Some(off));
        log::info!(
            "时区：{} → UTC{:+}（[ntp] timezone）",
            cfg.timezone,
            off / 3600
        );
    } else if !cfg.timezone.trim().is_empty() {
        log::warn!(
            "认不出 [ntp] timezone = {:?}，改用 TZ / /etc/TZ",
            cfg.timezone
        );
    }

    if !cfg.enabled {
        log::info!("NTP 同步已关闭（[ntp] enabled=false），时间回退为设备时间");
        let s = st.lock().unwrap();
        on_change(&s);
        return;
    }

    let timeout = Duration::from_secs(cfg.timeout_secs.clamp(1, 10) as u64);
    let on_change = Arc::new(on_change);
    std::thread::spawn(move || {
        let mut phase = Phase::Fast;
        let mut attempts = 0u32;
        let mut next_at = system_now() + FIRST_DELAY;
        let mut last_logged_err = String::new();
        log::info!("NTP 同步已启动：服务器 {:?}，3 秒后首次尝试", servers);
        loop {
            // 每 1 秒醒一次：既能按时执行，也能秒级响应「立即同步」
            let now = system_now();
            let forced = FORCE.swap(false, Ordering::Relaxed);
            if !forced && now < next_at {
                {
                    let mut s = st.lock().unwrap();
                    s.next_in_secs = (next_at - now).max(0);
                }
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let _ = forced;

            {
                let mut s = st.lock().unwrap();
                s.last_try_at = crate::logging::rfc3339(unix_now());
                s.next_in_secs = 0;
            }

            match query_any(&servers, timeout) {
                Ok(sample) => {
                    crate::logging::set_clock_offset(sample.offset_secs);
                    let mut s = st.lock().unwrap();
                    s.synced = true;
                    s.source = "ntp";
                    s.server = sample.server.clone();
                    s.offset_secs = sample.offset_secs;
                    s.delay_ms = sample.delay_ms;
                    s.last_ok_at = crate::logging::rfc3339(unix_now());
                    s.last_error.clear();
                    s.ok_count += 1;
                    s.phase = Phase::Steady;
                    s.attempts = 0;
                    s.next_in_secs = STEADY_INTERVAL;
                    log::info!(
                        "NTP 已同步：{} 偏移 {:+.3}s（往返 {}ms），下次 20 分钟后",
                        sample.server,
                        sample.offset_secs,
                        sample.delay_ms
                    );
                    on_change(&s);
                    phase = Phase::Steady;
                    attempts = 0;
                    next_at = system_now() + STEADY_INTERVAL;
                    last_logged_err.clear();
                }
                Err(e) => {
                    attempts += 1;
                    let (next_phase, interval, note) = match phase {
                        Phase::Fast if attempts >= FAST_TRIES => (
                            Phase::Slow,
                            SLOW_INTERVAL,
                            format!("前 {FAST_TRIES} 次（每 3 秒）都没成功，退避到每 1 分钟"),
                        ),
                        Phase::Fast => (Phase::Fast, FAST_INTERVAL, String::new()),
                        Phase::Slow if attempts >= SLOW_TRIES => (
                            Phase::Steady,
                            STEADY_INTERVAL,
                            format!("又 {SLOW_TRIES} 次（每 1 分钟）都没成功，转为每 20 分钟"),
                        ),
                        Phase::Slow => (Phase::Slow, SLOW_INTERVAL, String::new()),
                        Phase::Steady => (Phase::Steady, STEADY_INTERVAL, String::new()),
                    };
                    {
                        let mut s = st.lock().unwrap();
                        s.synced = s.ok_count > 0;
                        s.source = if s.ok_count > 0 { "ntp" } else { "system" };
                        s.attempts = attempts;
                        s.phase = next_phase;
                        s.fail_count += 1;
                        s.next_in_secs = interval;
                        s.last_error = e.clone();
                        // 阶段没变时不刷屏：同一错误只记一次
                        if note.is_empty() && last_logged_err != e {
                            log::warn!(
                                "NTP 同步失败（{}/{}）：{e}",
                                attempts,
                                match phase {
                                    Phase::Fast => FAST_TRIES,
                                    Phase::Slow => SLOW_TRIES,
                                    Phase::Steady => 0,
                                }
                            );
                        }
                        if !note.is_empty() {
                            log::warn!("{note}；最后一次错误：{e}");
                        }
                        on_change(&s);
                    }
                    last_logged_err = e;
                    if next_phase != phase {
                        phase = next_phase;
                        attempts = 0;
                    }
                    next_at = system_now() + interval;
                }
            }
        }
    });
}

// ---------- 便捷 trait（把 ToSocketAddrs 的错误统一成 String） ----------

trait ToSockAddrsSafe {
    fn to_socket_addrs_safe(&self) -> Result<std::net::SocketAddr, String>;
}

impl ToSockAddrsSafe for (&str, u16) {
    fn to_socket_addrs_safe(&self) -> Result<std::net::SocketAddr, String> {
        use std::net::ToSocketAddrs;
        let mut it = self.to_socket_addrs().map_err(|e| e.to_string())?;
        it.next().ok_or_else(|| "解析结果为空".to_string())
    }
}

/// 运行期改时区（`POST /api/ntp/set`）：解析成功就立刻全局生效并写回 config.toml。
///
/// 返回 Err 时不动现有时区（调用方把错误原样给页面）。
pub fn set_timezone(spec: &str) -> Result<i64, String> {
    match parse_timezone(spec) {
        Some(off) => {
            crate::logging::set_configured_tz(Some(off));
            // 状态里的时区名也要跟上（页面「改时区」拿它当默认值，不能显示旧值）
            state().lock().unwrap().timezone = spec.trim().to_string();
            log::info!(
                "时区已改为 {} → UTC{:+}（运行期，立即生效）",
                spec,
                off / 3600
            );
            Ok(off)
        }
        None => Err(format!(
            "认不出时区写法 {spec:?}（支持 IANA 名如 Asia/Shanghai，或 POSIX 如 CST-8）"
        )),
    }
}

/// 清除配置时区（回到 `TZ` / `/etc/TZ`）。
pub fn clear_timezone() {
    crate::logging::set_configured_tz(None);
    log::info!("配置时区已清除，时区跟随 TZ / /etc/TZ");
}

#[cfg(test)]
mod tests;
