//! 系统时区：IANA 名 ↔ OpenWrt 的 POSIX TZ 写法，读写 `/etc/config/system`。
//!
//! ## 为什么必须转换，而不是直接写 IANA 名
//!
//! 这台固件（ImmortalWrt SNAPSHOT，精简版）**没有装 zoneinfo 数据库**：
//! `/usr/share/zoneinfo/` 不存在，`/etc/localtime` 只是一个指向 `/tmp/localtime`
//! 的符号链接。也就是说「把 IANA 名当文件路径去 link」这条路根本走不通。
//!
//! OpenWrt 一直以来的做法是**双字段**：
//!
//! ```text
//! option timezone 'CST-8'          ← POSIX TZ 串，真正生效的那个（写进 /etc/TZ）
//! option zonename 'Asia/Shanghai'  ← 给人看的 IANA 名，仅用于界面显示
//! ```
//!
//! 所以本模块做三件事：
//! 1. 界面用 IANA 名（好看、无歧义）；
//! 2. 落盘时同时写 `zonename`（原样）与 `timezone`（映射成 POSIX，含 DST 规则）；
//! 3. 把 POSIX 串同步进 `/etc/TZ` —— BusyBox 的 `date`/`logread` 读的就是它。
//!
//! ## 关于 DST
//!
//! `America/New_York` 这类必须带夏令时规则，否则夏天会差一小时。POSIX 的
//! `EST5EDT,M3.2.0,M11.1.0` 写法就是干这个的（3 月第 2 个周日 2:00 起、
//! 11 月第 1 个周日 2:00 止）。表里的规则都是标准的，不是猜的。
//!
//! ## 不在表里的时区
//!
//! **保留原值、不覆盖**：用户可能用 `uci` 手工设过奇怪的值，界面照原样显示，
//! 只在真的选了某项时才改写。这条是用户明确要求的。

use std::process::Command;

use crate::error::{msg, Result};

/// 当前系统时区（读 `/etc/config/system`）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TzState {
    /// IANA 名（`zonename`）；固件没配时为空。
    pub zonename: String,
    /// POSIX TZ 串（`timezone`），真正生效的那个。
    pub timezone: String,
    /// 偏移（秒，东为正）。从 POSIX 串解析；解析不出为 `None`。
    pub offset_secs: Option<i64>,
    /// 偏移的人话（`UTC+8`）。
    pub offset_label: String,
    /// `/etc/TZ` 的实际内容（用于发现「配置与生效值不一致」）。
    pub etc_tz: String,
    /// 该 IANA 名是否在预置列表里（不在时界面照原样显示，不自动改）。
    pub in_preset: bool,
}

/// 预置选项：`(IANA 名, POSIX TZ, 中文名)`。
///
/// POSIX 串取自各区域的法定规则（含 DST 起止），与 OpenWrt 默认配置同格式。
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("Asia/Shanghai", "CST-8", "中国标准时间"),
    ("Asia/Hong_Kong", "HKT-8", "香港时间"),
    ("Asia/Taipei", "CST-8", "台北时间"),
    ("Asia/Singapore", "SGT-8", "新加坡时间"),
    ("Asia/Tokyo", "JST-9", "日本标准时间"),
    ("Asia/Seoul", "KST-9", "韩国标准时间"),
    ("UTC", "UTC0", "协调世界时"),
    (
        "America/Los_Angeles",
        "PST8PDT,M3.2.0,M11.1.0",
        "洛杉矶时间（含夏令时）",
    ),
    (
        "America/New_York",
        "EST5EDT,M3.2.0,M11.1.0",
        "纽约时间（含夏令时）",
    ),
    (
        "Europe/London",
        "GMT0BST,M3.5.0/1,M10.5.0",
        "伦敦时间（含夏令时）",
    ),
    (
        "Europe/Paris",
        "CET-1CEST,M3.5.0,M10.5.0/3",
        "巴黎时间（含夏令时）",
    ),
    (
        "Australia/Sydney",
        "AEST-10AEDT,M10.1.0,M4.1.0/3",
        "悉尼时间（含夏令时）",
    ),
];

/// 预置列表的 JSON 形态（`GET /api/timezone` 用）。
pub fn presets_payload() -> Vec<serde_json::Value> {
    PRESETS
        .iter()
        .map(|(name, tz, label)| {
            serde_json::json!({ "zonename": name, "timezone": tz, "label": label })
        })
        .collect()
}

/// IANA 名 → POSIX TZ 串。不在表里返回 `None`（调用方据此「保留原值」）。
pub fn iana_to_posix(name: &str) -> Option<&'static str> {
    let n = name.trim();
    PRESETS
        .iter()
        .find(|(iana, _, _)| *iana == n)
        .map(|(_, tz, _)| *tz)
}

/// 从 POSIX TZ 串里解出「标准时」偏移（秒，东为正）。
///
/// 例如 `CST-8` → `+28800`；`EST5EDT,...` → `-18000`；`UTC0` → `0`。
/// 只解析**前半段**（标准时），DST 那部分不影响这里的展示。
pub fn posix_offset(tz: &str) -> Option<i64> {
    let s = tz.trim();
    if s.is_empty() {
        return None;
    }
    // 跳过字母缩写（CST / JST / AEST…），取紧随其后的数字
    let digits: String = s
        .chars()
        .skip_while(|c| c.is_ascii_alphabetic())
        .take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '+' || *c == '-')
        .collect();
    if digits.is_empty() {
        return None;
    }
    // POSIX 的符号与直觉相反：`CST-8` 表示「UTC+8」（负号 = 东）。
    let (sign, rest) = match digits.chars().next() {
        Some('-') => (1i64, &digits[1..]),
        Some('+') => (-1i64, &digits[1..]),
        _ => (-1i64, digits.as_str()),
    };
    let mut parts = rest.split(':');
    let h = parts.next()?.parse::<i64>().ok()?;
    let m = parts
        .next()
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(0);
    Some(sign * (h * 3600 + m * 60))
}

/// 偏移（秒）→ `UTC+8` / `UTC-5:30` 这样的标签。
pub fn offset_label(secs: i64) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    let h = a / 3600;
    let m = (a % 3600) / 60;
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

/// 读一个 `uci get` 值（失败返回空串）。
fn uci_get(key: &str) -> String {
    Command::new("uci")
        .args(["get", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// 当前状态（只读，不改任何东西）。
pub fn current() -> TzState {
    let zonename = uci_get("system.@system[0].zonename");
    let timezone = uci_get("system.@system[0].timezone");
    let etc_tz = std::fs::read_to_string("/etc/TZ")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    // 优先按配置里的 timezone 算偏移；它空着时退回 /etc/TZ
    let src = if timezone.trim().is_empty() {
        etc_tz.as_str()
    } else {
        timezone.as_str()
    };
    let offset_secs = posix_offset(src);
    TzState {
        in_preset: iana_to_posix(&zonename).is_some(),
        offset_label: offset_secs.map(offset_label).unwrap_or_default(),
        offset_secs,
        zonename,
        timezone,
        etc_tz,
    }
}

/// 设置系统时区（写 `uci` + `/etc/TZ`，然后**回读验证**）。
///
/// 只接受预置列表里的 IANA 名：列表之外的字符串没法可靠映射成 POSIX 串，
/// 硬写会得到一个「界面显示东京、实际还是北京时间」的假象。
pub fn set(name: &str) -> Result<TzState> {
    let name = name.trim();
    if name.is_empty() {
        return Err(msg("缺少 zonename（如 Asia/Shanghai）"));
    }
    let posix = iana_to_posix(name).ok_or_else(|| {
        msg(format!(
            "不支持的时区 {name}：只接受预置列表里的 IANA 名（见 GET /api/timezone 的 presets）"
        ))
    })?;

    // ① uci 写两个字段。`system.@system[0]` 是 OpenWrt 的固定 section。
    for (key, val) in [
        ("system.@system[0].zonename", name),
        ("system.@system[0].timezone", posix),
    ] {
        let out = Command::new("uci")
            .args(["set", &format!("{key}={val}")])
            .output()
            .map_err(|e| msg(format!("跑不了 uci set {key}: {e}")))?;
        if !out.status.success() {
            return Err(msg(format!(
                "uci set {key}={val} 失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    // ② commit 才真正落盘（uci set 只改内存里的 tree）
    let out = Command::new("uci")
        .args(["commit", "system"])
        .output()
        .map_err(|e| msg(format!("跑不了 uci commit system: {e}")))?;
    if !out.status.success() {
        return Err(msg(format!(
            "uci commit system 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    // ③ `/etc/TZ` 必须跟着改：BusyBox 的 date / logread 读的是它，
    //    只改 uci 不重启服务的话，当前进程看到的还是旧时区。
    if let Err(e) = std::fs::write("/etc/TZ", format!("{posix}\n")) {
        return Err(msg(format!(
            "已写入 uci，但更新 /etc/TZ 失败（当前进程仍是旧时区）: {e}"
        )));
    }

    // ④ 回读验证 —— 用户明确要求「应用后重新读取系统时区进行验证」。
    let now = current();
    if now.zonename != name {
        return Err(msg(format!(
            "写入后回读不一致：期望 zonename={name}，实际为「{}」",
            now.zonename
        )));
    }
    if now.etc_tz != posix {
        return Err(msg(format!(
            "写入后回读不一致：期望 /etc/TZ={posix}，实际为「{}」",
            now.etc_tz
        )));
    }
    Ok(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_offset_handles_common_shapes() {
        // 东八区：POSIX 写成 CST-8（负号 = 东）
        assert_eq!(posix_offset("CST-8"), Some(8 * 3600));
        assert_eq!(posix_offset("JST-9"), Some(9 * 3600));
        assert_eq!(posix_offset("UTC0"), Some(0));
        // 西五区：EST5 是正号（= UTC-5）
        assert_eq!(posix_offset("EST5EDT,M3.2.0,M11.1.0"), Some(-5 * 3600));
        assert_eq!(posix_offset("PST8PDT,M3.2.0,M11.1.0"), Some(-8 * 3600));
        // 半小时时区
        assert_eq!(posix_offset("IST-5:30"), Some(5 * 3600 + 1800));
        // 认不出就是 None，不瞎猜
        assert_eq!(posix_offset(""), None);
        assert_eq!(posix_offset("Asia/Shanghai"), None);
    }

    #[test]
    fn offset_label_formats() {
        assert_eq!(offset_label(8 * 3600), "UTC+8");
        assert_eq!(offset_label(-5 * 3600), "UTC-5");
        assert_eq!(offset_label(19800), "UTC+5:30");
        assert_eq!(offset_label(0), "UTC+0");
    }

    #[test]
    fn presets_are_all_convertible() {
        // 表里每一项都必须能从 IANA 名映射回 POSIX，且能解析出偏移。
        // 这条防的是「表里加了一项但忘了填 POSIX 串」。
        for (iana, posix, label) in PRESETS {
            assert!(!iana.is_empty() && !posix.is_empty() && !label.is_empty());
            assert_eq!(iana_to_posix(iana), Some(*posix), "{iana} 映射不一致");
            assert!(
                posix_offset(posix).is_some(),
                "{iana} 的 POSIX 串解析不出偏移"
            );
        }
    }

    #[test]
    fn unknown_zone_is_rejected_not_guessed() {
        // 不在表里 → None（调用方保留原值，不覆盖）
        assert_eq!(iana_to_posix("Mars/Olympus"), None);
        assert_eq!(iana_to_posix(""), None);
    }
}
