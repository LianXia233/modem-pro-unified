//! USSD 应答与下发（`GET /api/ussd` 读最近一次，`POST /api/ussd` 下发）。
//!
//! `+CUSD` 是 URC，异步到达，由 URC 消费循环（`http::loops::spawn_urc`）解析后
//! 写入 [`super::modem::Modem`] 的缓存并推送；少数固件把它放在 `OK` 之前一起回来，
//! 那种情况下发时就地收下（见 [`Modem::send_ussd`]）。
//! `+SPERROR`（展锐私有，CS 类业务失败时上报）在时间窗内附到本次结果上。
//!
//! ## 两条收取路径为什么不会重复推
//!
//! `+CUSD` 现在会被 `at/urc.rs` 按形态判据吞进 URC 队列，所以「同步到达」与
//! 「异步到达」**都从同一个队列出**：`send_ussd` 用 `take_urc_of([Cusd])` 只取
//! `+CUSD`（不碰其它类别），取走即清；消费循环下一拍再来取时已经没有这条了。
//! 谁先拿到谁推，天然单推。

use serde::Serialize;

use crate::error::{msg, Result};

use super::super::at::{error_line, extract_field};
use super::modem::Modem;

/// 一次 USSD 应答。
#[derive(Debug, Clone, Serialize)]
pub struct UssdResult {
    /// 0 无需后续操作 / 1 需要继续 / 2 会话被网络结束 / 4 不支持 / 5 超时
    pub status: i32,
    /// 已尽力解码的可读文本
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sperror: Option<String>,
    /// 结果到达时间（RFC3339）
    pub at: String,
}

impl UssdResult {
    /// 新建一条结果（`status` 与 `text` 来源于 `+CUSD: <status>,"<text>",<dcs>`）。
    pub fn new(status: i32, text: String, raw: Option<String>) -> Self {
        Self {
            status,
            text,
            raw,
            sperror: None,
            at: crate::logging::timestamp(),
        }
    }
}

/// USSD 码合法性：只允许数字与 `*` `#` `+`，长度 1~64。
///
/// 它会原样拼进 AT 指令参数里，所以必须严格限制（防止注入别的指令）。
pub fn valid_ussd_code(code: &str) -> bool {
    let code = code.trim();
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'*' | b'#' | b'+'))
}

/// DCS 是否表示 UCS2 正文（`0x48`，或 bit6/7 组合出的 `0x0C` 位）。
fn cusd_text_is_ucs2(dcs: i32) -> bool {
    dcs == 0x48 || (dcs & 0x0C) == 0x08
}

/// 解析一行 `+CUSD: <status>[,"<text>"[,<dcs>]]`（URC 读协程与同步回包共用）。
///
/// 正文用引号定界（里面可以带逗号），所以不能按逗号硬切；UCS2 正文按短信同一套判据还原
/// （纯数字串不会被误解码，见 `super::sms::decode_sms_text`）。
pub fn parse_cusd_line(line: &str) -> Option<UssdResult> {
    let line = line.trim();
    let field = extract_field(line, "+CUSD")?;
    let (status_s, rest) = match field.split_once(',') {
        Some(p) => p,
        None => (field.as_str(), ""),
    };
    let status: i32 = status_s.trim().parse().ok()?;
    let rest = rest.trim_start();
    let (text, dcs) = match rest.strip_prefix('"') {
        Some(inner) => match inner.split_once('"') {
            Some((text, tail)) => {
                let dcs = tail.trim_start().trim_start_matches(',').trim();
                (text.to_string(), dcs.parse::<i32>().unwrap_or(0))
            }
            // 引号没闭合：剩下的全当正文（容错，别把应答丢了）
            None => (inner.to_string(), 0),
        },
        // 网络结束会话时只有状态码，没有正文
        None => (String::new(), 0),
    };
    let text = if text.is_empty() || !cusd_text_is_ucs2(dcs) {
        text
    } else {
        super::sms::decode_sms_text(&text)
    };
    Some(UssdResult {
        status,
        text,
        raw: Some(line.to_string()),
        sperror: None,
        at: crate::logging::timestamp(),
    })
}

impl Modem {
    /// SIM 是不是锁着 PIN（`AT+CPIN?` 回 `SIM PIN`）：锁着时发不了 USSD。
    pub fn sim_pin_locked(&self) -> bool {
        let resp = self.at_cmd("AT+CPIN?");
        extract_field(&resp, "+CPIN")
            .map(|s| s.trim().eq_ignore_ascii_case("SIM PIN"))
            .unwrap_or(false)
    }

    /// 下发一条 USSD 码（`POST /api/ussd` 的写侧）。
    ///
    /// 只负责校验与下发：真正的应答是**异步 URC**（`+CUSD`），由消费循环写缓存
    /// 并推送；少数固件把 `+CUSD` 放在 `OK` 之前一起回来，那种情况这里顺手从
    /// URC 队列收下并返回，调用方当场就能推给页面，不必等下一轮。
    ///
    /// ⚠️ 不等待异步应答：同步 HTTP 请求线程等一条可能几分钟才来的 URC，会把
    /// 请求线程整个挂住（见 `PROGRESS.md` §15.3 对 `CFUN=4` 自恢复的同一判据）。
    /// 只收「下发这条命令期间正好已经到了」的那条。
    pub fn send_ussd(&self, code: &str) -> Result<Option<UssdResult>> {
        let code = code.trim();
        if !valid_ussd_code(code) {
            return Err(msg(format!(
                "USSD 码只能包含数字与 * # +，且长度 1~64（当前 {code:?}）"
            )));
        }
        if self.sim_pin_locked() {
            return Err(msg("SIM 卡处于锁定状态，无法发送 USSD"));
        }
        // 清掉上一次结果：否则页面会把旧应答当成这次的
        self.clear_ussd();
        // 15 = 7bit 默认字符集；会话模式 1 = 需要应答
        let cmd = format!("AT+CUSD=1,\"{code}\",15");
        let resp = self.at_cmd(&cmd);
        if resp.trim().is_empty() {
            return Err(msg("AT 口无应答（串口未打开或模组不应答）"));
        }
        if let Some(e) = error_line(&resp) {
            log::debug!("USSD 被拒（模组 {}）: {e}", self.id);
            // 固件常在 AT+CUSD=? 里声明支持、真发时却被拒：USSD 是电路域(CS)业务，
            // 5G SA 数据卡 / 物联网卡多半没有 CS 注册，这种组合下 USSD 实际不可用
            return Err(msg(format!(
                "模组拒绝 {cmd}：USSD 是电路域(CS)业务，当前 CS 未注册或卡不支持（可用「概览→网络」里的 CS 域状态确认）"
            )));
        }
        log::info!("USSD 已下发: 模组 {} 码 {code}", self.id);
        // 同步到达的 `+CUSD` 已被过滤层吞进 URC 队列（同前缀的 `AT+CUSD=?`
        // 应答要放行，所以不能按前缀一刀切），这里只把 CUSD 这一类取走。
        // 极端情况下仍有固件把它当普通应答行放回来，两处都查，谁先命中算谁。
        let hit = resp
            .lines()
            .find_map(parse_cusd_line)
            .or_else(|| self.take_pending_cusd());
        if let Some(r) = &hit {
            self.set_ussd(r.clone());
        }
        Ok(hit)
    }

    /// 取走队列里已经到达的 `+CUSD`（只取这一类，别的类别原样留下）。
    fn take_pending_cusd(&self) -> Option<UssdResult> {
        let lines = self.take_urc_of(&[super::super::at::UrcKind::Cusd]);
        lines.iter().rev().find_map(|u| parse_cusd_line(&u.line))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_cusd_line, valid_ussd_code};

    #[test]
    fn ussd_code_charset() {
        assert!(valid_ussd_code("*100#"));
        assert!(valid_ussd_code("+8613800138000"));
        assert!(valid_ussd_code("  *101#  "));
        assert!(!valid_ussd_code(""));
        assert!(!valid_ussd_code("   "));
        // 会拼进 AT 参数，别的字符一律拒绝（含分号/引号/中文/空格）
        assert!(!valid_ussd_code("*100;ATD"));
        assert!(!valid_ussd_code("*10\"0#"));
        assert!(!valid_ussd_code("查余额"));
        assert!(!valid_ussd_code("*1 0#"));
        assert!(!valid_ussd_code(&"*".repeat(65)));
        assert!(valid_ussd_code(&"1".repeat(64)));
    }

    #[test]
    fn cusd_status_only() {
        // 网络结束会话：只有状态码
        let r = parse_cusd_line("+CUSD: 2").unwrap();
        assert_eq!(r.status, 2);
        assert_eq!(r.text, "");
        assert_eq!(r.raw.as_deref(), Some("+CUSD: 2"));
        // 不是 +CUSD 行
        assert!(parse_cusd_line("+CMTI: \"SM\",1").is_none());
        assert!(parse_cusd_line("OK").is_none());
    }

    #[test]
    fn cusd_text_and_dcs() {
        let r = parse_cusd_line("+CUSD: 0,\"余额为 12.5 元\",15").unwrap();
        assert_eq!(r.status, 0);
        assert_eq!(r.text, "余额为 12.5 元");
        assert_eq!(r.raw.as_deref(), Some("+CUSD: 0,\"余额为 12.5 元\",15"));
        // 无 dcs
        let r = parse_cusd_line("+CUSD: 1,\"继续\"").unwrap();
        assert_eq!(r.status, 1);
        assert_eq!(r.text, "继续");
        // 正文里有逗号：按引号定界，不能按逗号硬切
        let r = parse_cusd_line("+CUSD: 0,\"余,额 12,5\",15").unwrap();
        assert_eq!(r.text, "余,额 12,5");
        // 空正文
        let r = parse_cusd_line("+CUSD: 2,\"\",15").unwrap();
        assert_eq!(r.status, 2);
        assert_eq!(r.text, "");
    }

    #[test]
    fn cusd_ucs2_decoded() {
        // DCS=72(0x48) → UCS2 十六进制，按短信同一套判据还原（含 CJK 才还原）
        let hex = "你好"
            .encode_utf16()
            .map(|u| format!("{u:04X}"))
            .collect::<String>();
        let line = format!("+CUSD: 0,\"{hex}\",72");
        let r = parse_cusd_line(&line).unwrap();
        assert_eq!(r.text, "你好");
        // 纯数字串是合法十六进制，但判据要求含 CJK，不能误解码
        let r = parse_cusd_line("+CUSD: 0,\"12345678\",72").unwrap();
        assert_eq!(r.text, "12345678");
    }

    #[test]
    fn cusd_line_tolerances() {
        // 带 \r\n 前缀（URC 行的常见形态）与多余空格
        let r = parse_cusd_line("\r\n+CUSD:   0 ,  \"hi\" , 15 ").unwrap();
        assert_eq!(r.status, 0);
        assert_eq!(r.text, "hi");
        // 引号没闭合：剩下的全当正文
        let r = parse_cusd_line("+CUSD: 0,\"半截").unwrap();
        assert_eq!(r.text, "半截");
    }
}
