//! 流量计量（`AT+QGDCNT?` / `AT+GTSTATIS?` 等厂商私有指令）。
//!
//! 两类口径，界面与配额计算都按它区分：
//!   · **计数式**（移远等）：模组直接给累计字节数，差分即本周期用量；
//!   · **速率式**（广和通 FM 系列只有瞬时速率、没有累计计数器）：由本进程按
//!     `速率 × 间隔` 积分，属**估算**，界面上会标出来。
//!
//! 指令是厂商私有的，所以不做「按厂商硬编码后假装一定成功」：按厂商给出候选，
//! 逐个探测，第一个解析出数据的才算支持；一个都不行就 `supported=false`，
//! 由上层如实告诉界面「这台模组不支持计量，配额无法启用」。

use serde::Serialize;

use super::super::at::{csv, extract_all};
use super::super::vendor::Vendor;
use super::modem::Modem;

/// 计量方式。
pub const KIND_COUNTER: &str = "counter";
pub const KIND_RATE: &str = "rate";
pub const KIND_NONE: &str = "none";

/// 一次流量采样。
#[derive(Debug, Clone, Default, Serialize)]
pub struct TrafficSample {
    pub modem_id: u32,
    /// 这台模组是否提供流量数据（false 时配额无法启用）
    pub supported: bool,
    /// [`KIND_COUNTER`] / [`KIND_RATE`] / [`KIND_NONE`]
    pub kind: String,
    /// 实际用到的指令（界面上要显示「按哪条指令统计」）
    pub cmd: String,
    /// 累计上行/下行字节（仅计数式）
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// 瞬时上行/下行速率（bps，仅速率式）
    pub tx_rate_bps: u64,
    pub rx_rate_bps: u64,
    /// 速率式积分出来的用量属于估算
    pub estimated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl Modem {
    /// 读一次流量计数（失败/不支持也返回一个 `supported=false` 的采样，不报错）。
    pub fn traffic(&self) -> TrafficSample {
        for cmd in candidates(self.vendor()) {
            let resp = self.at_cmd(cmd);
            if let Some(mut s) = parse(cmd, &resp) {
                s.modem_id = self.id;
                s.cmd = cmd.to_string();
                s.raw = Some(resp.trim().to_string());
                return s;
            }
        }
        TrafficSample {
            modem_id: self.id,
            supported: false,
            kind: KIND_NONE.to_string(),
            note: Some("该模组未上报流量计数器，无法计量".to_string()),
            ..Default::default()
        }
    }

    /// 清掉模组的累计流量计数器（速率式没有可清的计数器，只清本地基准）。
    ///
    /// 返回是否真的对模组下发了清零指令。
    pub fn reset_traffic_counter(&self) -> (bool, Option<String>) {
        // 速率差分用的本地基线也要一起清：否则下一轮会拿清零前的量算出虚高速度
        *self.traffic_prev.lock().unwrap() = None;
        for cmd in candidates(self.vendor()) {
            let resp = self.at_cmd(cmd);
            if parse(cmd, &resp).is_none() {
                continue;
            }
            if cmd.starts_with("AT+QGDCNT") {
                // 移远的清零写法是同一条指令带参数 0
                let r = self.at_cmd("AT+QGDCNT=0");
                return (r.contains("OK"), Some(r.trim().to_string()));
            }
            return (false, None);
        }
        (false, None)
    }
}

/// 界面上的流量视图（`/api/network/status` 的 `traffic`、`/api/traffic/reset` 的返回）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct TrafficView {
    pub supported: bool,
    /// 速率/累计来自积分估算（广和通只有瞬时速率）
    pub estimated: bool,
    pub cmd: String,
    /// 累计上行 / 下行（字节）
    pub sent: u64,
    pub recv: u64,
    pub up_bps: u64,
    pub down_bps: u64,
    pub up_text: String,
    pub down_text: String,
    /// 本次速率的采样窗（秒）
    pub interval_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 上一次采样的快照（速率差分用，存在 `Modem` 上）。
#[derive(Debug, Clone, Copy)]
pub struct TrafficPrev {
    pub at: i64,
    pub tx: u64,
    pub rx: u64,
}

/// 速率式模组的本地积分累计量（存在 `Modem` 上）。
///
/// ## 为什么需要积分
///
/// 广和通的 `AT+GTSTATIS` 只回**瞬时速率**（bps），没有累计计数器。
/// 而页面上的「累计已发送 / 已接收」不能永远显示 0 —— 早先 `traffic_view()`
/// 直接取 `s.tx_bytes`（速率式路径下恒 0），于是广和通用户看到的累计流量
/// 一直为零，配额页也就无从判断。
///
/// ## 局限性（如实说明，不假装是精确值）
///
/// 这是「采样点速率 × 采样间隔」的梯形近似：
/// - 只在**有人查询 / 采样循环运行时**推进，没人看的那段时间会被漏掉；
/// - 速率本身是模组按自己窗口算的平均值，与我们的 `dt` 不同步。
///
/// 因此该值标 `estimated=true`，语义是「本次上电以来的估算量」，
/// 不是「本月账单用量」。要做账请以运营商侧为准。
#[derive(Debug, Clone, Copy, Default)]
pub struct RateTotal {
    pub at: i64,
    pub sent: u64,
    pub recv: u64,
}

impl Modem {
    /// 流量视图：累计字节 + 实测速率。
    ///
    /// 速率是**差分**得来的（计数式），所以第一次采样只能给出累计值、速率为 0
    /// —— 不能把「模组开机至今的总量」当成「这一秒的速度」。
    pub fn traffic_view(&self) -> TrafficView {
        let s = self.traffic();
        let base = TrafficView {
            supported: s.supported,
            estimated: s.estimated,
            cmd: s.cmd.clone(),
            up_text: fmt_bps(s.tx_rate_bps),
            down_text: fmt_bps(s.rx_rate_bps),
            note: s.note.clone(),
            ..Default::default()
        };
        if !s.supported {
            return base;
        }
        let now = crate::logging::unix_now();
        let mut view = base;
        if s.kind == KIND_RATE {
            // 速率式：模组只给瞬时速率，累计量靠本地积分（速率 × 间隔）
            let mut g = self.rate_total.lock().unwrap();
            let mut total = g.unwrap_or_default();
            let dt = if total.at > 0 {
                (now - total.at).max(0) as f64
            } else {
                0.0
            };
            // bps × 秒 / 8 = 字节；用 f64 累加避免整数截断把小流量抹成 0
            total.sent += (s.tx_rate_bps as f64 * dt / 8.0) as u64;
            total.recv += (s.rx_rate_bps as f64 * dt / 8.0) as u64;
            total.at = now;
            *g = Some(total);

            view.sent = total.sent;
            view.recv = total.recv;
            view.up_bps = s.tx_rate_bps;
            view.down_bps = s.rx_rate_bps;
            view.up_text = fmt_bps(s.tx_rate_bps);
            view.down_text = fmt_bps(s.rx_rate_bps);
            view.interval_s = dt;
        } else {
            view.sent = s.tx_bytes;
            view.recv = s.rx_bytes;
            if let Some(prev) = *self.traffic_prev.lock().unwrap() {
                let dt = (now - prev.at) as f64;
                if dt >= 1.0 {
                    let up = (super::quota::delta(s.tx_bytes, prev.tx) as f64 * 8.0 / dt) as u64;
                    let down = (super::quota::delta(s.rx_bytes, prev.rx) as f64 * 8.0 / dt) as u64;
                    view.up_bps = up;
                    view.down_bps = down;
                    view.up_text = fmt_bps(up);
                    view.down_text = fmt_bps(down);
                    view.interval_s = (dt * 10.0).round() / 10.0;
                }
            }
        }
        *self.traffic_prev.lock().unwrap() = Some(TrafficPrev {
            at: now,
            tx: s.tx_bytes,
            rx: s.rx_bytes,
        });
        view
    }
}

/// 速率可读文本（`12.34 Mbps`）。
pub fn fmt_bps(bps: u64) -> String {
    let v = bps as f64;
    if v >= 1_000_000.0 {
        format!("{:.2} Mbps", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1} Kbps", v / 1_000.0)
    } else {
        format!("{} bps", bps)
    }
}

/// 按厂商给出候选指令（按可信度排序）。
pub fn candidates(vendor: Vendor) -> &'static [&'static str] {
    // 各家的候选表在 `modem/vendor/<厂商>.rs`；默认是「先计数式再速率式」
    vendor.spec().traffic_candidates()
}

/// 按指令解析应答（解析不出返回 None）。
pub fn parse(cmd: &str, resp: &str) -> Option<TrafficSample> {
    if cmd.starts_with("AT+QGDCNT") {
        let (tx, rx) = parse_qgdcnt(resp)?;
        return Some(TrafficSample {
            supported: true,
            kind: KIND_COUNTER.to_string(),
            tx_bytes: tx,
            rx_bytes: rx,
            ..Default::default()
        });
    }
    if cmd.starts_with("AT+GTSTATIS") {
        let (tx, rx) = parse_gtstatis(resp)?;
        return Some(TrafficSample {
            supported: true,
            kind: KIND_RATE.to_string(),
            tx_rate_bps: tx,
            rx_rate_bps: rx,
            estimated: true,
            note: Some("该模组只报瞬时速率，累计用量由本地积分估算".to_string()),
            ..Default::default()
        });
    }
    None
}

/// `+QGDCNT: <tx>,<rx>`（部分固件多带一个 cid：`+QGDCNT: <cid>,<tx>,<rx>`）。
///
/// 按量纲定位：一行里全是数字，cid 是个位数、字节数至少上万，故统一取**最后两个**。
pub fn parse_qgdcnt(resp: &str) -> Option<(u64, u64)> {
    let line = extract_all(resp, "+QGDCNT").first()?.clone();
    let nums = numbers_in(&line);
    let n = nums.len();
    if n < 2 {
        return None;
    }
    Some((nums[n - 2], nums[n - 1]))
}

/// `+GTSTATIS: <cid>,<tx_rate>,<rx_rate>`（广和通：瞬时速率，单位 bps）。
pub fn parse_gtstatis(resp: &str) -> Option<(u64, u64)> {
    let line = extract_all(resp, "+GTSTATIS").first()?.clone();
    let nums = numbers_in(&line);
    match nums.len() {
        // 带 cid：跳过它取后两个
        n if n >= 3 => Some((nums[1], nums[2])),
        2 => Some((nums[0], nums[1])),
        _ => None,
    }
}

/// 取一行里的所有非负整数（用于「按量纲定位」的数字型应答）。
fn numbers_in(line: &str) -> Vec<u64> {
    csv(line)
        .iter()
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{candidates, numbers_in, parse_gtstatis, parse_qgdcnt, Vendor, KIND_COUNTER};

    #[test]
    fn qgdcnt_with_and_without_cid() {
        // 两字段：直接是上行/下行累计字节
        assert_eq!(
            parse_qgdcnt("+QGDCNT: 12345678,987654321\r\nOK\r\n"),
            Some((12_345_678, 987_654_321))
        );
        // 三字段：首字段是 cid，取后两个
        assert_eq!(
            parse_qgdcnt("+QGDCNT: 1,12345678,987654321\r\nOK\r\n"),
            Some((12_345_678, 987_654_321))
        );
        assert_eq!(parse_qgdcnt("+QGDCNT: 1\r\nOK\r\n"), None);
        assert_eq!(parse_qgdcnt("ERROR\r\n"), None);
    }

    #[test]
    fn gtstatis_rates() {
        assert_eq!(
            parse_gtstatis("+GTSTATIS: 1,1500000,30000000\r\nOK\r\n"),
            Some((1_500_000, 30_000_000))
        );
        assert_eq!(
            parse_gtstatis("+GTSTATIS: 1500000,30000000\r\nOK\r\n"),
            Some((1_500_000, 30_000_000))
        );
        assert_eq!(parse_gtstatis("ERROR"), None);
    }

    #[test]
    fn numbers_are_filtered_from_noise() {
        assert_eq!(numbers_in("1,\"IPV4\",2048,999"), vec![1, 2048, 999]);
    }

    #[test]
    fn candidate_order_per_vendor() {
        assert_eq!(candidates(Vendor::Quectel), &["AT+QGDCNT?"]);
        assert_eq!(candidates(Vendor::Fibocom), &["AT+GTSTATIS?"]);
        assert_eq!(candidates(Vendor::Unknown), &["AT+QGDCNT?", "AT+GTSTATIS?"]);
        assert_eq!(candidates(Vendor::Simcom), &["AT+QGDCNT?", "AT+GTSTATIS?"]);
    }

    #[test]
    fn counter_kind_is_not_estimated() {
        let s = super::parse("AT+QGDCNT?", "+QGDCNT: 100,200\r\nOK\r\n").unwrap();
        assert!(s.supported);
        assert_eq!(s.kind, KIND_COUNTER);
        assert!(!s.estimated);
        assert_eq!((s.tx_bytes, s.rx_bytes), (100, 200));
    }

    #[test]
    fn rate_kind_is_estimated() {
        let s = super::parse("AT+GTSTATIS?", "+GTSTATIS: 1,10,20\r\nOK\r\n").unwrap();
        assert!(s.estimated);
        assert_eq!((s.tx_rate_bps, s.rx_rate_bps), (10, 20));
    }
}
