//! 注册状态（`AT+CREG?` / `AT+CGREG?` / `AT+CEREG?` / `AT+C5GREG?`）。
//!
//! 应答格式（3GPP TS 27.007）：
//!   `+CREG: <n>,<stat>` 或 `+CREG: <n>,<stat>,"<lac>","<ci>"[,<AcT>]`
//! 四类域的字段完全一致，共用一份解析。

use serde::Serialize;

use super::super::at::extract_field;

/// 一个域（CS / PS / 5GS / EPS）的注册状态。
#[derive(Debug, Clone, Default, Serialize)]
pub struct RegState {
    /// 0-5（3GPP TS 27.007）
    pub stat: i32,
    /// 中文描述
    pub text: String,
    /// stat 为 1 或 5
    pub registered: bool,
    /// stat == 5
    pub roaming: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// 解析一条 `+CREG`/`+CGREG`/`+CEREG`/`+C5GREG` 应答。
pub fn parse(resp: &str, prefix: &str) -> RegState {
    let field = extract_field(resp, prefix);
    let mut state = RegState {
        raw: Some(resp.trim().to_string()),
        ..Default::default()
    };
    let Some(field) = field else {
        state.text = "未注册".to_string();
        return state;
    };
    let parts = super::super::at::csv(&field);
    state.stat = parts
        .get(1)
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    state.lac = parts.get(2).filter(|s| !s.is_empty()).cloned();
    state.cell_id = parts.get(3).filter(|s| !s.is_empty()).cloned();
    state.act = parts.get(4).and_then(|s| s.parse::<i32>().ok());
    state.registered = state.stat == 1 || state.stat == 5;
    state.roaming = state.stat == 5;
    state.text = text(state.stat).to_string();
    state
}

/// `stat` 的中文描述。
pub fn text(stat: i32) -> &'static str {
    match stat {
        0 => "未注册，未在搜索",
        1 => "已注册，本地网络",
        2 => "未注册，正在搜索",
        3 => "注册被拒绝",
        4 => "未知",
        5 => "已注册，漫游",
        _ => "未知",
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn short_form() {
        let s = parse("\r\n+CGREG: 0,1\r\n\r\nOK\r\n", "+CGREG");
        assert_eq!(s.stat, 1);
        assert!(s.registered);
        assert!(!s.roaming);
        assert_eq!(s.text, "已注册，本地网络");
    }

    #[test]
    fn full_form_roaming() {
        let s = parse("+CREG: 0,5,\"1A2B\",\"1234\",7\r\nOK", "+CREG");
        assert_eq!(s.stat, 5);
        assert!(s.roaming);
        assert_eq!(s.lac.as_deref(), Some("1A2B"));
        assert_eq!(s.cell_id.as_deref(), Some("1234"));
        assert_eq!(s.act, Some(7));
    }

    #[test]
    fn missing_field() {
        let s = parse("+CREG: ERROR\r\n", "+CREG");
        assert_eq!(s.stat, 0);
        assert!(!s.registered);
    }
}
