//! 时长字符串解析：`"4s"` / `"300ms"` / `"-1s"` / 裸秒数。

use std::time::Duration;

/// 解析时长；空串用 `default`，负值/0 归零。
pub fn parse_duration(s: &str, default: Duration) -> Duration {
    let s = s.trim();
    if s.is_empty() {
        return default;
    }
    if let Some(v) = s.strip_suffix("ms") {
        if let Ok(n) = v.trim().parse::<i64>() {
            return if n < 0 {
                Duration::ZERO
            } else {
                Duration::from_millis(n as u64)
            };
        }
    }
    if let Some(v) = s.strip_suffix('s') {
        if let Ok(n) = v.trim().parse::<i64>() {
            return if n <= 0 {
                Duration::ZERO
            } else {
                Duration::from_secs(n as u64)
            };
        }
    }
    if let Ok(n) = s.parse::<i64>() {
        return if n <= 0 {
            Duration::ZERO
        } else {
            Duration::from_secs(n as u64)
        };
    }
    default
}

#[cfg(test)]
mod tests {
    use super::parse_duration;
    use std::time::Duration;

    #[test]
    fn units() {
        assert_eq!(
            parse_duration("4s", Duration::from_secs(9)),
            Duration::from_secs(4)
        );
        assert_eq!(
            parse_duration("300ms", Duration::from_secs(9)),
            Duration::from_millis(300)
        );
        assert_eq!(
            parse_duration("7", Duration::from_secs(9)),
            Duration::from_secs(7)
        );
    }

    #[test]
    fn empty_and_negative() {
        assert_eq!(
            parse_duration("", Duration::from_secs(9)),
            Duration::from_secs(9)
        );
        assert_eq!(
            parse_duration("-1s", Duration::from_secs(9)),
            Duration::ZERO
        );
        assert_eq!(
            parse_duration("garbage", Duration::from_secs(9)),
            Duration::from_secs(9)
        );
    }
}
