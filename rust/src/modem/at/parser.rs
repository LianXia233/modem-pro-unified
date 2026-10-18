//! AT 应答解析（终结符判定与字段提取）。

/// 是否收到了 AT 应答的终结符。
pub(super) fn has_terminator(text: &str) -> bool {
    const TERMS: [&str; 6] = [
        "\r\nOK\r\n",
        "\r\nERROR",
        "OK\r\n",
        "ERROR",
        "+CME ERROR",
        "+CMS ERROR",
    ];
    TERMS.iter().any(|t| text.contains(t))
}

/// 从 AT 应答里取一个 `+XXX: <值>` 形式的字段值。
pub fn extract_field(resp: &str, prefix: &str) -> Option<String> {
    for line in resp.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some(rest.trim().trim_start_matches(':').trim().to_string());
        }
    }
    None
}

/// 取 `+XXX: a,b,c` 里的第 `idx` 个子字段（去空白与引号）。
pub fn field_part(value: &str, idx: usize) -> Option<String> {
    value
        .split(',')
        .nth(idx)
        .map(|s| s.trim().trim_matches('"').to_string())
}

/// 把所有以 `prefix` 开头的行取出（去掉前缀与冒号），用于多行应答（如 `+CGDCONT`）。
pub fn extract_all(resp: &str, prefix: &str) -> Vec<String> {
    resp.lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix(prefix)
                .map(|rest| rest.trim().trim_start_matches(':').trim().to_string())
        })
        .collect()
}

/// 按逗号切分并逐段去空白/引号（空段保留为空串）。
pub fn csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .collect()
}

/// 解析整数（容忍前后空白与引号）。
pub fn to_i32(value: &str) -> Option<i32> {
    value.trim().trim_matches('"').parse::<i32>().ok()
}

/// 是否收到成功终结符（`OK`）且没有 `ERROR`。
pub fn is_ok(resp: &str) -> bool {
    resp.contains("OK") && !resp.contains("ERROR")
}

/// 取出应答里的错误行（`ERROR` / `+CME ERROR: xxx` / `+CMS ERROR: xxx`）。
///
/// 有些接口必须把「模组回 ERROR」和「确实没有数据」区分开（例如读短信：模式没切过去时
/// `AT+CMGL` 直接报错，此时把空列表当成"卡里没短信"会让界面显示 0 条）。
pub fn error_line(resp: &str) -> Option<String> {
    for line in resp.lines() {
        let line = line.trim();
        let up = line.to_uppercase();
        if up == "ERROR" || up.starts_with("+CME ERROR") || up.starts_with("+CMS ERROR") {
            return Some(line.to_string());
        }
    }
    None
}

/// 是否为「重复激活」错误（`+CME ERROR: 5847`）。
///
/// 这个码的含义是上下文其实已经激活，不该判失败 —— 调用方应当重新读一遍
/// 上下文、以「是否拿到地址」为准。实测在已激活的 cid 上再次下发
/// `AT+CGACT=1,<cid>` 就会拿到它。
pub fn is_duplicate_activate(resp: &str) -> bool {
    resp.contains("5847")
}

#[cfg(test)]
mod tests {
    use super::{
        csv, error_line, extract_all, extract_field, field_part, has_terminator,
        is_duplicate_activate, is_ok, to_i32,
    };

    #[test]
    fn terminators() {
        assert!(has_terminator("AT\r\r\nOK\r\n"));
        assert!(has_terminator("+CME ERROR: 3\r\n"));
        assert!(!has_terminator("AT\r\r\n"));
    }

    #[test]
    fn duplicate_activate_is_recognized() {
        assert!(is_duplicate_activate("\r\n+CME ERROR: 5847\r\n"));
        assert!(is_duplicate_activate("+CME ERROR: 5847"));
        // 别的 CME 码与纯 OK 都不算重复激活
        assert!(!is_duplicate_activate("\r\n+CME ERROR: 100\r\n"));
        assert!(!is_duplicate_activate("\r\nOK\r\n"));
    }

    #[test]
    fn fields() {
        let resp = "\r\n+CSQ: 22,99\r\n\r\nOK\r\n";
        let v = extract_field(resp, "+CSQ").unwrap();
        assert_eq!(v, "22,99");
        assert_eq!(field_part(&v, 0).unwrap(), "22");
        assert_eq!(field_part(&v, 1).unwrap(), "99");
    }

    #[test]
    fn multi_lines_and_csv() {
        let resp =
            "\r\n+CGDCONT: 1,\"IPV4V6\",\"3GNET\"\r\n+CGDCONT: 2,\"IP\",\"ims\"\r\n\r\nOK\r\n";
        let all = extract_all(resp, "+CGDCONT");
        assert_eq!(all.len(), 2);
        let first = csv(&all[0]);
        assert_eq!(first[0], "1");
        assert_eq!(first[1], "IPV4V6");
        assert_eq!(first[2], "3GNET");
        assert_eq!(to_i32(" 42 "), Some(42));
        assert_eq!(to_i32("x"), None);
    }

    #[test]
    fn ok_and_error() {
        assert!(is_ok("\r\nOK\r\n"));
        assert!(!is_ok("\r\n+CME ERROR: 3\r\n"));
    }

    #[test]
    fn error_lines() {
        assert_eq!(error_line("\r\nOK\r\n"), None);
        assert_eq!(error_line("\r\nERROR\r\n").as_deref(), Some("ERROR"));
        assert_eq!(
            error_line("\r\n+CMS ERROR: 313\r\n").as_deref(),
            Some("+CMS ERROR: 313")
        );
    }
}
