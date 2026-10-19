//! 无依赖的时间戳（秒精度）：UTC 工具 + 本机时区偏移（POSIX TZ，无 DST）。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// 形如 `2026-10-17T23:42:01Z`。
pub(crate) fn timestamp() -> String {
    rfc3339(unix_now())
}

static TZ_OFFSET: OnceLock<i64> = OnceLock::new();

/// NTP 校时偏移（秒）：`插件时间 = 系统时间 + 本偏移`。
///
/// 由 `runtime/ntp.rs` 在同步成功后写入；未同步 / 同步一直失败时保持 0，
/// 也就是「回退读设备时间」—— 这正是设计要求的降级路径。
static CLOCK_OFFSET: AtomicI64 = AtomicI64::new(0);

/// 写入/清除校时偏移（NTP 模块用；秒，可为负）。
pub(crate) fn set_clock_offset(secs: i64) {
    CLOCK_OFFSET.store(secs, Ordering::Relaxed);
}

/// 当前校时偏移（秒）。
pub(crate) fn clock_offset() -> i64 {
    CLOCK_OFFSET.load(Ordering::Relaxed)
}

/// **未经校准**的系统时间（NTP 自己算 offset 时必须用它，否则会自激）。
pub(crate) fn system_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 本机时区偏移（秒；**东为正**）。进程内只算一次。
///
/// 数据来源：`TZ` 环境变量，缺了再读 `/etc/TZ`（OpenWrt 惯例）。只认 **POSIX
/// std offset** 写法（`CST-8`、`UTC+3`、`GMT-2`、`<+03>-3`）—— 不解析 TZif
/// 区域库（`:Asia/Shanghai` 认不出，按 UTC 并 warn），也不套 DST 规则。
/// 定时任务「按本机时区判定」就以这里为准，认不出时如实降级并记日志。
pub(crate) fn tz_offset_secs() -> i64 {
    let configured = CONFIGURED_TZ.load(Ordering::Relaxed);
    if configured != UNSET_TZ {
        return configured;
    }
    *TZ_OFFSET.get_or_init(|| {
        let spec = match std::env::var("TZ") {
            // 空串按 POSIX 语义 = UTC
            Ok(v) => Some(v),
            Err(_) => std::fs::read_to_string("/etc/TZ").ok(),
        };
        match spec {
            None => 0,
            Some(s) if s.trim().is_empty() => 0,
            Some(s) => match parse_posix_tz_offset(&s) {
                Some(off) => off,
                None => {
                    log::warn!("认不出时区写法 {s:?}（不解析 TZif 区域库），定时任务按 UTC 判定");
                    0
                }
            },
        }
    })
}

/// 配置里指定的时区偏移（`[ntp] timezone`）。设置后**优先于** `TZ` / `/etc/TZ`。
///
/// 用哨兵值实现「运行期可改」：`UNSET` 表示配置没给时区（走 `TZ` / `/etc/TZ`）。
/// 页面上改时区（`POST /api/ntp/set`）会直接改这里 —— 比 OnceLock 灵活，
/// 改完立刻对所有 `local_unix_now()` 生效，不用重启。
static CONFIGURED_TZ: AtomicI64 = AtomicI64::new(UNSET_TZ);
/// 「没配置时区」的哨兵值（i64::MIN 不可能是任何真实偏移）。
const UNSET_TZ: i64 = i64::MIN;

/// 设置/清除配置时区偏移（秒，东为正）。
///
/// 传 `None` 表示清除（回到 `TZ` / `/etc/TZ`）；重复调用以最后一次为准。
pub(crate) fn set_configured_tz(offset_secs: Option<i64>) {
    match offset_secs {
        Some(v) => CONFIGURED_TZ.store(v, Ordering::Relaxed),
        None => CONFIGURED_TZ.store(UNSET_TZ, Ordering::Relaxed),
    }
}

/// 解析 POSIX TZ 字符串的**标准偏移**并翻转成「东为正」（POSIX 西为正）。
///
/// 形态：`std offset [dst [offset] [,rule]]`——标签（3 个以上字母，或 `<...>`
/// 引用形式）后跟 `[+-]hh[:mm[:ss]]`；偏移后面的 DST 名 / 规则（如
/// `EST5EDT,M3.2.0/2,M11.1.0/2`）直接忽略。`CST-8` → +28800、`UTC+3` → -10800。
pub(crate) fn parse_posix_tz_offset(spec: &str) -> Option<i64> {
    let s = spec.trim();
    if s.is_empty() || s.starts_with(':') {
        return None; // `:Asia/Shanghai` 是区域库名，不解析
    }
    let mut rest = s;
    if let Some(stripped) = rest.strip_prefix('<') {
        let end = stripped.find('>')?;
        rest = stripped[end + 1..].trim_start();
    } else {
        // POSIX 要求标签至少 3 个字母
        let n = rest.chars().take_while(|c| c.is_ascii_alphabetic()).count();
        if n < 3 {
            return None;
        }
        rest = &rest[n..];
    }
    if rest.is_empty() {
        return Some(0);
    }
    let (sign, body) = match rest.as_bytes()[0] {
        b'+' => (-1i64, &rest[1..]),
        b'-' => (1, &rest[1..]),
        // POSIX 缺省按正（西为正）→ 翻转成东为负
        _ => (-1, rest),
    };
    // 偏移的数字部分到第一个非数字字符为止（后面是 DST 名或规则，忽略）
    let end = body
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(body.len());
    let hh: i64 = body[..end].parse().ok()?;
    let mut secs = hh * 3600;
    let mut tail = body[end..].strip_prefix(':').unwrap_or(&body[end..]);
    for scale in [60i64, 1] {
        let end = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        if end == 0 {
            break;
        }
        secs += tail[..end].parse::<i64>().ok()? * scale;
        tail = tail[end..].strip_prefix(':').unwrap_or(&tail[end..]);
    }
    // 偏移后面只允许 DST 缩写（POSIX 要求 ≥3 个字母）或 `,规则`，其余当垃圾拒掉
    if !tail.is_empty() && !tail.starts_with(',') {
        let name_len = tail.chars().take_while(|c| c.is_ascii_alphabetic()).count();
        let quoted = tail.starts_with('<');
        if (!quoted && name_len < 3) || (quoted && !tail.contains('>')) {
            return None;
        }
    }
    Some(sign * secs)
}

/// 本机时区的当前 Unix 秒（= UTC 秒 + 时区偏移；后续日期/时刻判定全用它）。
pub(crate) fn local_unix_now() -> i64 {
    unix_now() + tz_offset_secs()
}

/// Howard Hinnant 的 days_from_civil 逆运算（纯整数，无依赖）。
///
/// 配额周期要按「每月某日」算起止（见 modem/model/quota.rs），所以对外暴露。
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 当前 Unix 秒（**已含 NTP 校准**，全插件共用这一个时间源）。
///
/// 校准偏移由 `runtime/ntp.rs` 维护；没同步上就是纯系统时间（要求的降级行为）。
/// 需要「未校准的原始系统时间」时用 [`system_now`]。
pub(crate) fn unix_now() -> i64 {
    system_now() + clock_offset()
}

/// Unix **毫秒**（含 NTP 校时偏移）。
///
/// 流量采样要按毫秒给每个点打时间戳：曲线横轴是 1 秒一个点，
/// 用秒级时间戳会让同一秒内的两次采样撞在一起、前端画竖线。
pub(crate) fn unix_now_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    ms + clock_offset() * 1000
}

/// Unix 秒 → RFC3339（UTC，形如 `2026-10-18T17:36:01Z`）。
///
/// 与 [`timestamp`] 的区别：这个接受任意时刻，供采样历史/波动记录序列化。
/// 统一按 UTC 输出：前端用 `new Date(...)` 解析，绝对时刻一致即可。
pub(crate) fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 距 `timestamp()` 生成的 RFC3339 时间戳已过多少秒（解析失败返回 None）。
pub(crate) fn seconds_since(ts: &str) -> Option<u64> {
    let stamp = parse_timestamp(ts)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(now.saturating_sub(stamp))
}

/// 解析 `YYYY-MM-DDTHH:MM:SSZ` 为 Unix 秒。
fn parse_timestamp(ts: &str) -> Option<u64> {
    let s = ts.trim().trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let m: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hh: u64 = t.next()?.parse().ok()?;
    let mm: u64 = t.next()?.parse().ok()?;
    let ss: u64 = t.next()?.parse().ok()?;
    let days = days_from_civil(y, m, day);
    u64::try_from(days)
        .ok()?
        .checked_mul(86_400)?
        .checked_add(hh * 3600 + mm * 60 + ss)
}

/// Howard Hinnant 的 days_from_civil（纯整数，与 [`civil_from_days`] 互逆）。
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, days_from_civil, parse_posix_tz_offset, parse_timestamp};

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn known_date() {
        // 2000-03-01 是霍华德历法变换里的经典测试点
        assert_eq!(civil_from_days(11017), (2000, 3, 1));
    }

    #[test]
    fn roundtrip_and_parse() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp("2000-03-01T00:00:00Z"),
            Some(11017 * 86_400)
        );
        assert_eq!(parse_timestamp("bad"), None);
    }

    #[test]
    fn posix_tz_offsets() {
        // POSIX 西为正：CST-8 = 东八区
        assert_eq!(parse_posix_tz_offset("CST-8"), Some(8 * 3600));
        assert_eq!(parse_posix_tz_offset("GMT-2"), Some(2 * 3600));
        // UTC+3 在 POSIX 里是西三区
        assert_eq!(parse_posix_tz_offset("UTC+3"), Some(-3 * 3600));
        assert_eq!(parse_posix_tz_offset("UTC0"), Some(0));
        assert_eq!(parse_posix_tz_offset("UTC"), Some(0));
        // 带分秒与 DST 规则的完整写法：只取标准偏移
        assert_eq!(
            parse_posix_tz_offset("EST5EDT,M3.2.0/2,M11.1.0/2"),
            Some(-5 * 3600)
        );
        assert_eq!(parse_posix_tz_offset("<+03>-3"), Some(3 * 3600));
        assert_eq!(parse_posix_tz_offset("XXX-9:30"), Some(9 * 3600 + 1800));
        // 认不出的一律 None（调用方按 UTC 降级并 warn）
        assert_eq!(parse_posix_tz_offset(""), None);
        assert_eq!(parse_posix_tz_offset(":Asia/Shanghai"), None);
        assert_eq!(parse_posix_tz_offset("8"), None);
        assert_eq!(parse_posix_tz_offset("UTC+3x"), None);
    }
}
