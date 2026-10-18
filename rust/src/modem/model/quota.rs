//! 流量配额（`GET /api/quota` 与 `quota/set|reset|unblock`、`traffic/reset`）。
//!
//! 三件事分开：
//!   · 本文件 —— 纯口径：用量怎么算、周期怎么滚、超限怎么判、断网动作怎么发；
//!   · `runtime/store.rs` —— `quota.json` 落盘（重启不丢）；
//!   · `http/handlers/quota*.rs` —— 把查询与动作串起来。
//!
//! ⚠️ 断网动作只对**上网用**的 PDP 上下文下手：真机上 `AT+CGACT?` 会把 cid 1（上网）
//! 与 cid 2（IMS）一起报成激活，而停用 IMS 会直接回 ERROR —— 这是实测踩过的坑。
//! ⚠️ 「关闭射频」对展锐 SIPC 那一族（中兴 F50 / UMS9620）是禁用的：整机只有模组这一路
//! 上行，关射频会把它自己的管理页 / SSH 一起断掉，所以先查飞行模式能力再动手。

use serde::{Deserialize, Serialize};

use crate::config::QuotaConfig;
use crate::error::{msg, Result};

use super::super::at::is_ok;
use super::modem::Modem;

/// 按模组保存的配额设置（对应 `quota.json` 里的 `settings`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSettings {
    pub enabled: bool,
    pub limit_bytes: i64,
    pub count_up: bool,
    pub count_down: bool,
    pub action: String,
    pub reset_day: i64,
}

impl QuotaSettings {
    /// 由配置段的默认值构造（配置段是 conffile，升级不覆盖，所以它只能当默认值）。
    pub fn from_config(c: &QuotaConfig) -> Self {
        Self {
            enabled: c.enabled,
            limit_bytes: c.limit_bytes,
            count_up: c.count_up,
            count_down: c.count_down,
            action: c.action().to_string(),
            reset_day: c.reset_day,
        }
    }
}

/// 配额状态（接口直接序列化它）。
#[derive(Debug, Clone, Serialize)]
pub struct QuotaState {
    pub modem_id: u32,
    /// 模组是否提供流量计量（false → 界面提示「无法计量」，配额不可启用）
    pub supported: bool,
    pub enabled: bool,
    pub limit_bytes: i64,
    pub limit_text: String,
    pub used_bytes: u64,
    pub used_text: String,
    pub used_up: u64,
    pub used_down: u64,
    pub remain_bytes: u64,
    pub remain_text: String,
    pub percent: f64,
    pub over_limit: bool,
    /// 已按超限动作断网 / 关射频
    pub blocked: bool,
    /// 本周期解除限制（用户点过「本周期不再限制」）
    pub suspended: bool,
    /// 用量来自速率积分（估算）
    pub estimated: bool,
    pub action: String,
    pub count_up: bool,
    pub count_down: bool,
    pub reset_day: i64,
    pub period_start: String,
    pub period_end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl QuotaState {
    /// 未启用 / 不支持计量时的空状态（界面照常渲染，只是没有进度条）。
    pub fn idle(modem_id: u32, s: &QuotaSettings, supported: bool, note: Option<String>) -> Self {
        Self {
            modem_id,
            supported,
            enabled: s.enabled,
            limit_bytes: s.limit_bytes,
            limit_text: fmt_bytes(s.limit_bytes.max(0) as u64),
            used_bytes: 0,
            used_text: fmt_bytes(0),
            used_up: 0,
            used_down: 0,
            remain_bytes: s.limit_bytes.max(0) as u64,
            remain_text: fmt_bytes(s.limit_bytes.max(0) as u64),
            percent: 0.0,
            over_limit: false,
            blocked: false,
            suspended: false,
            estimated: false,
            action: s.action.clone(),
            count_up: s.count_up,
            count_down: s.count_down,
            reset_day: s.reset_day,
            period_start: String::new(),
            period_end: String::new(),
            blocked_at: None,
            note,
        }
    }

    /// 由「设置 + 本周期用量 + 周期区间」算出完整状态。
    ///
    /// 参数多是这个视图本身的性质（界面要一次拿到全部展示口径），拆成 builder
    /// 只会把口径打散，故保留平铺参数。
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        modem_id: u32,
        s: &QuotaSettings,
        used_up: u64,
        used_down: u64,
        period_start: i64,
        period_end: i64,
        blocked: bool,
        suspended: bool,
        blocked_at: Option<String>,
        estimated: bool,
        supported: bool,
        note: Option<String>,
    ) -> Self {
        let up = if s.count_up { used_up } else { 0 };
        let down = if s.count_down { used_down } else { 0 };
        let used = up.saturating_add(down);
        let limit = s.limit_bytes.max(0) as u64;
        let over_limit = limit > 0 && used >= limit;
        let remain = limit.saturating_sub(used);
        Self {
            modem_id,
            supported,
            enabled: s.enabled,
            limit_bytes: s.limit_bytes,
            limit_text: fmt_bytes(limit),
            used_bytes: used,
            used_text: fmt_bytes(used),
            used_up,
            used_down,
            remain_bytes: remain,
            remain_text: fmt_bytes(remain),
            percent: percent_of(used, limit),
            over_limit,
            blocked,
            suspended,
            estimated,
            action: s.action.clone(),
            count_up: s.count_up,
            count_down: s.count_down,
            reset_day: s.reset_day,
            period_start: stamp_of(period_start),
            period_end: stamp_of(period_end),
            blocked_at,
            note,
        }
    }
}

/// 字节数 → 人类可读（`1.23 GB`）。与界面的量级分档一致（1024 进制）。
pub fn fmt_bytes(n: u64) -> String {
    const K: u64 = 1024;
    let n = n as f64;
    if n < K as f64 {
        return format!("{} B", n as u64);
    }
    if n < (K * K) as f64 {
        return format!("{:.1} KB", n / K as f64);
    }
    if n < (K * K * K) as f64 {
        return format!("{:.1} MB", n / (K * K) as f64);
    }
    if n < (K * K * K * K) as f64 {
        return format!("{:.2} GB", n / (K * K * K) as f64);
    }
    format!("{:.2} TB", n / (K * K * K * K) as f64)
}

/// 用量百分比（上限为 0 = 不限量，返回 0；结果夹到 0~100）。
pub fn percent_of(used: u64, limit: u64) -> f64 {
    if limit == 0 {
        return 0.0;
    }
    let p = used as f64 * 100.0 / limit as f64;
    if p < 0.0 {
        0.0
    } else if p > 100.0 {
        100.0
    } else {
        (p * 10.0).round() / 10.0
    }
}

/// 周期区间：`reset_day` 为 1~28 时按「每月该日」给出本周期的 [起, 止)；
/// 为 0（不自动重置）时止点返回 0 —— 由调用方沿用记录里的起点。
pub fn period_bounds(reset_day: i64, now: i64) -> (i64, i64) {
    if reset_day <= 0 || reset_day > 28 {
        return (now, 0);
    }
    let (y, m, d) = crate::logging::civil_from_days(now.div_euclid(86_400));
    let (sy, sm) = if (d as i64) >= reset_day {
        (y, m)
    } else {
        prev_month(y, m)
    };
    let (ey, em) = next_month(sy, sm);
    let start = crate::logging::days_from_civil(sy, sm, reset_day as u32) * 86_400;
    let end = crate::logging::days_from_civil(ey, em, reset_day as u32) * 86_400;
    (start, end)
}

fn prev_month(y: i64, m: u32) -> (i64, u32) {
    if m <= 1 {
        (y - 1, 12)
    } else {
        (y, m - 1)
    }
}

fn next_month(y: i64, m: u32) -> (i64, u32) {
    if m >= 12 {
        (y + 1, 1)
    } else {
        (y, m + 1)
    }
}

fn stamp_of(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    crate::logging::rfc3339(secs)
}

/// 速率式积分的增量（字节）。
///
/// 间隔夹到 1~3600 秒：长时间没人查询时，用最后一次的瞬时速率乘一大段时间会明显虚高，
/// 宁可少算也不能让界面凭空多出几个 GB。
pub fn integrate(rate_bps: u64, elapsed_s: i64) -> u64 {
    let dt = elapsed_s.clamp(1, 3600) as u64;
    rate_bps.saturating_mul(dt) / 8
}

/// 计数式差分：模组重启 / 计数器归零后当前值会小于上次，此时把当前值当增量。
pub fn delta(cur: u64, last: u64) -> u64 {
    if cur < last {
        cur
    } else {
        cur - last
    }
}

impl Modem {
    /// 当前激活的**上网用** PDP 上下文（排除 ims / sos / mms）。
    pub fn data_cids(&self) -> Vec<i32> {
        let defined = self.at_cmd("AT+CGDCONT?");
        let activated = self.at_cmd("AT+CGACT?");
        super::params::build_contexts(&defined, &activated)
            .into_iter()
            .filter(|c| c.active && !c.internal)
            .map(|c| c.cid)
            .collect()
    }

    /// 停用上网用的数据连接（`AT+CGACT=0,<cid>`），返回真正停用的 cid。
    pub fn block_data(&self) -> Result<Vec<i32>> {
        let cids = self.data_cids();
        if cids.is_empty() {
            return Err(msg("没有处于激活状态的上网数据连接"));
        }
        let mut done = Vec::new();
        for cid in cids {
            let resp = self.at_cmd(&format!("AT+CGACT=0,{cid}"));
            if is_ok(&resp) {
                done.push(cid);
            } else {
                log::warn!("模组 {} 停用 cid {cid} 失败: {}", self.id, resp.trim());
            }
        }
        if done.is_empty() {
            return Err(msg("下发停用数据连接失败（模组回 ERROR）"));
        }
        Ok(done)
    }

    /// 恢复上网用的数据连接（`AT+CGACT=1,<cid>`）。
    ///
    /// 上下文定义还在（只是被停用），所以按 `AT+CGDCONT?` 里**非内部**的 cid 重新激活。
    pub fn unblock_data(&self) -> Vec<i32> {
        let defined = self.at_cmd("AT+CGDCONT?");
        let cids = super::params::build_contexts(&defined, "")
            .into_iter()
            .filter(|c| !c.internal)
            .map(|c| c.cid)
            .collect::<Vec<_>>();
        cids.into_iter()
            .filter(|cid| is_ok(&self.at_cmd(&format!("AT+CGACT=1,{cid}"))))
            .collect()
    }

    /// 开关射频（飞行模式）。先确认固件支持 `AT+CFUN?`，不支持就直接失败
    /// —— 展锐 SIPC 那一族关射频会把设备自己的管理通道一起断掉。
    pub fn set_airplane(&self, on: bool) -> Result<()> {
        let st = self.airplane();
        if !st.supported {
            return Err(msg(super::airplane::airplane_blocked_reason()));
        }
        let cmd = if on { "AT+CFUN=4" } else { "AT+CFUN=1" };
        let resp = self.at_cmd(cmd);
        if is_ok(&resp) {
            Ok(())
        } else {
            Err(msg(format!("下发 {cmd} 失败：{}", resp.trim())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fmt_bytes, integrate, percent_of, period_bounds, QuotaSettings, QuotaState};
    use crate::config::QuotaConfig;
    use crate::logging::days_from_civil;

    fn settings(limit: i64, reset_day: i64) -> QuotaSettings {
        let c = QuotaConfig {
            enabled: true,
            limit_bytes: limit,
            reset_day,
            ..Default::default()
        };
        QuotaSettings::from_config(&c.normalized())
    }

    #[test]
    fn byte_formatting_matches_ui() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(fmt_bytes(1024u64.pow(4)), "1.00 TB");
    }

    #[test]
    fn percent_is_clamped_and_rounded() {
        assert_eq!(percent_of(0, 0), 0.0); // 不限量
        assert_eq!(percent_of(50, 100), 50.0);
        assert_eq!(percent_of(200, 100), 100.0);
        assert_eq!(percent_of(1, 3), 33.3);
    }

    #[test]
    fn monthly_period_rolls_at_reset_day() {
        // 2026-03-01 起，每月 1 日重置
        let start = days_from_civil(2026, 3, 1) * 86_400;
        let mid = start + 10 * 86_400;
        let (s, e) = period_bounds(1, mid);
        assert_eq!(s, start);
        assert_eq!(e, days_from_civil(2026, 4, 1) * 86_400);

        // 还没到重置日：属于上一个周期
        let before = start - 86_400; // 2026-02-28
        let (s, e) = period_bounds(1, before);
        assert_eq!(s, days_from_civil(2026, 2, 1) * 86_400);
        assert_eq!(e, start);
    }

    #[test]
    fn no_auto_reset_has_no_end() {
        let now = days_from_civil(2026, 5, 20) * 86_400;
        let (s, e) = period_bounds(0, now);
        assert_eq!(s, now);
        assert_eq!(e, 0);
        // 29~31 一律夹成「不自动重置」（有的月份没有这一天）
        assert_eq!(period_bounds(31, now).1, 0);
    }

    #[test]
    fn rate_integration_is_clamped() {
        assert_eq!(integrate(8_000_000, 1), 1_000_000);
        assert_eq!(integrate(8_000_000, 0), 1_000_000); // 至少按 1 秒算
        assert_eq!(integrate(8_000_000, 100_000), 8_000_000 * 3600 / 8);
    }

    #[test]
    fn over_limit_and_remaining() {
        let s = settings(1000, 0);
        let st = QuotaState::build(
            0, &s, 400, 700, 100, 0, false, false, None, false, true, None,
        );
        assert!(st.over_limit);
        assert_eq!(st.used_bytes, 1100);
        assert_eq!(st.remain_bytes, 0);
        assert_eq!(st.percent, 100.0);

        // 只统计下行时上行不计入
        let mut only_down = s.clone();
        only_down.count_up = false;
        let st = QuotaState::build(
            0, &only_down, 400, 700, 100, 0, false, false, None, false, true, None,
        );
        assert!(!st.over_limit);
        assert_eq!(st.used_bytes, 700);
    }

    #[test]
    fn idle_state_has_no_progress() {
        let s = settings(0, 0);
        let st = QuotaState::idle(1, &s, false, None);
        assert!(!st.supported);
        assert_eq!(st.limit_text, "0 B");
        assert_eq!(st.period_start, "");
    }
}
