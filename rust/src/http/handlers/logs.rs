//! `GET /api/logs?lines=200` —— 守护进程日志尾部（不占串口）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

/// 行数缺省值（<=0 按 200）。
const DEFAULT_LINES: usize = 200;
/// 单次最多回多少行。
const MAX_LINES: usize = 5000;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let lines = match parse_lines(query) {
        Ok(n) => n,
        Err(msg) => return response::fail(req, -1, &msg),
    };
    let rows = tail_file(&state.cfg.server.log_file, lines);
    response::ok(
        req,
        serde_json::json!({
            "file": state.cfg.server.log_file,
            "lines": rows,
            "count": rows.len(),
        }),
    )
}

/// 解析 `lines`：给了但解析不了要如实报错（「lines must be an integer」）。
fn parse_lines(query: &HashMap<String, String>) -> std::result::Result<usize, String> {
    let Some(raw) = query.get("lines") else {
        return Ok(DEFAULT_LINES);
    };
    if raw.trim().is_empty() {
        return Ok(DEFAULT_LINES);
    }
    match raw.trim().parse::<i64>() {
        Ok(n) if n > MAX_LINES as i64 => Ok(MAX_LINES),
        Ok(n) if n <= 0 => Ok(DEFAULT_LINES),
        Ok(n) => Ok(n as usize),
        Err(_) => Err("lines must be an integer".to_string()),
    }
}

/// 读取文件尾部最多 `lines` 行；文件不存在按空列表（不当作错误）。
fn tail_file(path: &str, lines: usize) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            let start = all.len().saturating_sub(lines);
            all[start..]
                .iter()
                .map(|l| l.trim_end_matches('\r').to_string())
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_lines, tail_file};
    use std::collections::HashMap;

    #[test]
    fn lines_param_is_validated() {
        let q = |v: &str| {
            let mut m = HashMap::new();
            m.insert("lines".to_string(), v.to_string());
            m
        };
        // 缺省 / 空串 → 200
        assert_eq!(parse_lines(&HashMap::new()).unwrap(), 200);
        assert_eq!(parse_lines(&q("")).unwrap(), 200);
        // 正常值
        assert_eq!(parse_lines(&q("50")).unwrap(), 50);
        // <=0 按 200
        assert_eq!(parse_lines(&q("0")).unwrap(), 200);
        assert_eq!(parse_lines(&q("-5")).unwrap(), 200);
        // 超上限夹到 5000
        assert_eq!(parse_lines(&q("99999")).unwrap(), 5000);
        // 非整数要报错
        assert!(parse_lines(&q("abc")).is_err());
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let rows = tail_file("不存在的日志文件.log", 10);
        assert!(rows.is_empty());
    }
}
