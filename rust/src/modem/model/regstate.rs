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
    /// `stat` 是**真实解析出来**的（应答里有可解析的数字），而不是失败后的占位值。
    ///
    /// 指令回 ERROR、超时、或应答里没有数字时为 `false` —— 此时 `stat=0` 只是
    /// 占位，不代表模组真的「未注册」。消费方（自动拨号门禁的周期缓存）据此
    /// 区分「明确未注册」与「查询失败」，后者不能用来拦拨号。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub known: bool,
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
    let parsed_stat = parts
        .get(1)
        .and_then(|s| s.parse::<i32>().ok())
        .map(|v| (v, true));
    if let Some((v, known)) = parsed_stat {
        state.stat = v;
        state.known = known;
    }
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
        // ERROR 应答 → stat 是占位值，不算「明确未注册」（门禁据此不拦拨号）
        assert!(!s.known);
    }

    #[test]
    fn parsed_stat_is_known() {
        let s = parse("+CREG: 0,0\r\n", "+CREG");
        assert_eq!(s.stat, 0);
        assert!(!s.registered);
        // 应答里真的有数字（哪怕 stat=0）→ 明确
        assert!(s.known);
    }

    #[test]
    fn unknown_stat_is_not_serialized() {
        // 序列化后没有 `known` 字段：门禁缓存里查不到 `known` 即视为不明确
        let s = parse("+CEREG: ERROR\r\n", "+CEREG");
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("known").is_none());
        let s = parse("+CREG: 0,2\r\n", "+CREG");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v.get("known"), Some(&serde_json::json!(true)));
    }
}
