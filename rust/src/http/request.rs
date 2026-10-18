//! 请求解析：路径、query、请求体。

use std::collections::HashMap;

use tiny_http::Request;

/// 取出不含 query 的路径。
pub fn path_of(url: &str) -> String {
    url.split('?').next().unwrap_or("/").to_string()
}

/// 解析 URL query 为 map（百分号解码）。
pub fn parse_query(url: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(q) = url.split('?').nth(1) {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(percent_decode(k), percent_decode(v));
            }
        }
    }
    map
}

/// 表单/query 的百分号解码（`+` → 空格，`%XX` → 字节；UTF-8 容错）。
///
/// query 与表单都会百分号解码，前端用
/// `encodeURIComponent` 编码过的模板指令（含 `"` `=` `,` 空格）才能原样到达
/// handler；不解码的话 `AT+CGSN` 会变成字面 `AT%2BCGSN` 发给模组。
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 把 POST 体（form 或单层 JSON）合并进 query 参数。
pub fn merge_form(query: &HashMap<String, String>, body: &str) -> HashMap<String, String> {
    let mut map = query.clone();
    let body = body.trim();
    if body.is_empty() {
        return map;
    }
    if body.starts_with('{') {
        if let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(body)
        {
            for (k, v) in obj {
                let s = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                map.insert(k, s);
            }
            return map;
        }
    }
    for pair in body.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(percent_decode(k), percent_decode(v));
        }
    }
    map
}

/// 取布尔参数（表单 / JSON 混用，JSON 的 `true` 经 `merge_form` 变成 `"true"`）。
pub fn bool_param(params: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match params.get(key).map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes" | "y"),
        None => default,
    }
}

/// 取整数参数（空串 / 非法 → 缺省值）。
pub fn i64_param(params: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    params
        .get(key)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(default)
}

/// 取浮点参数（空串 / 非法 / 非有限 → None）。
pub fn f64_param(params: &HashMap<String, String>, key: &str) -> Option<f64> {
    let v = params.get(key)?.trim().parse::<f64>().ok()?;
    if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

/// 读取请求体为字符串（用于 POST 表单/JSON）。
pub fn read_body(req: &mut Request) -> String {
    let mut s = String::new();
    let _ = req.as_reader().read_to_string(&mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::{bool_param, f64_param, i64_param, merge_form, parse_query, percent_decode};

    #[test]
    fn query_parsing() {
        let q = parse_query("/api/signal?modem_id=2&x=1");
        assert_eq!(q.get("modem_id").map(String::as_str), Some("2"));
        assert_eq!(q.get("x").map(String::as_str), Some("1"));
    }

    #[test]
    fn typed_params() {
        let map = parse_query("/x?on=1&off=false&n=42&bad=x&f=1.5&nan=zz");
        assert!(bool_param(&map, "on", false));
        assert!(!bool_param(&map, "off", true));
        assert!(bool_param(&map, "missing", true));
        assert_eq!(i64_param(&map, "n", 0), 42);
        assert_eq!(i64_param(&map, "bad", 7), 7);
        assert_eq!(f64_param(&map, "f"), Some(1.5));
        assert_eq!(f64_param(&map, "nan"), None);
    }

    #[test]
    fn form_and_json_bodies() {
        let base = parse_query("/api/at?modem_id=0");
        // 表单值要百分号解码：+ → 加号原文的编码还原
        let f = merge_form(&base, "cmd=AT%2BCGSN&other=1");
        assert_eq!(f.get("cmd").map(String::as_str), Some("AT+CGSN"));
        assert_eq!(f.get("other").map(String::as_str), Some("1"));
        let j = merge_form(&base, r#"{"cmd":"AT+CGSN"}"#);
        assert_eq!(j.get("cmd").map(String::as_str), Some("AT+CGSN"));
    }

    #[test]
    fn percent_decode_cases() {
        // query 与表单同口径：+ → 空格
        let q = parse_query("/api/x?ssid=My+Net&pwd=a%21b");
        assert_eq!(q.get("ssid").map(String::as_str), Some("My Net"));
        assert_eq!(q.get("pwd").map(String::as_str), Some("a!b"));
        // UTF-8 百分号序列（中文「网络」）
        assert_eq!(percent_decode("%E7%BD%91%E7%BB%9C"), "网络");
        // 畸形序列原样保留，不炸
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
        // 模板指令里的引号/等号/逗号（前端 encodeURIComponent 后能原样到达）
        assert_eq!(
            percent_decode("AT%2BQNWPREFCFG%3D%22policy_band%22%2C%22lte_band%22"),
            "AT+QNWPREFCFG=\"policy_band\",\"lte_band\""
        );
    }
}
