//! 选网方式（`AT+COPS?`）。
//!
//! 应答：`+COPS: <mode>[,<format>,"<oper>",<AcT>]`，未选网时只有 `+COPS: <mode>`。

use serde::Serialize;

use super::super::at::{csv, extract_field};

/// 选网方式。
#[derive(Debug, Clone, Serialize)]
pub struct NetSelect {
    /// 0 自动 / 1 手动 / 2 注销 / 4 手动转自动 / 5 自动转手动
    pub mode: i32,
    /// 中文说明
    pub mode_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plmn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl Default for NetSelect {
    fn default() -> Self {
        Self {
            mode: 0,
            mode_text: mode_text(0).to_string(),
            plmn: None,
            operator: None,
            act: None,
            raw: None,
        }
    }
}

/// 解析 `AT+COPS?` 的应答。
pub fn parse(resp: &str) -> NetSelect {
    let mut out = NetSelect {
        raw: Some(resp.trim().to_string()),
        ..Default::default()
    };
    let Some(field) = extract_field(resp, "+COPS") else {
        return out;
    };
    let parts = csv(&field);
    out.mode = parts
        .first()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    out.mode_text = mode_text(out.mode).to_string();
    if let Some(format) = parts.get(1) {
        let oper = parts.get(2).cloned().filter(|s| !s.is_empty());
        if format == "2" {
            out.plmn = oper;
        } else {
            out.operator = oper.filter(|s| s != "????");
        }
    }
    out.act = parts.get(3).and_then(|s| s.parse::<i32>().ok());
    out
}

/// `mode` 的中文说明。
pub fn mode_text(mode: i32) -> &'static str {
    match mode {
        0 => "自动选网",
        1 => "手动选网",
        2 => "已注销",
        4 => "手动转自动",
        5 => "自动转手动",
        _ => "未知",
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, NetSelect};

    #[test]
    fn manual_with_plmn() {
        let s = parse("+COPS: 1,2,\"46011\",7\r\nOK");
        assert_eq!(s.mode, 1);
        assert_eq!(s.plmn.as_deref(), Some("46011"));
        assert_eq!(s.act, Some(7));
        assert_eq!(s.mode_text, "手动选网");
    }

    #[test]
    fn auto_placeholders_ignored() {
        let s = parse("+COPS: 0,0,\"????\",11\r\nOK");
        assert_eq!(s.mode, 0);
        assert_eq!(s.operator, None);
        assert_eq!(s.plmn, None);
    }

    #[test]
    fn mode_only() {
        let s = parse("+COPS: 0\r\nOK");
        assert_eq!(s.mode, 0);
        assert_eq!(NetSelect::default().mode_text, "自动选网");
    }
}
