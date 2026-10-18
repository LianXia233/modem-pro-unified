//! 飞行模式（`GET /api/airplane`）。
//!
//! 判据只有 `AT+CFUN?`：`+CFUN: 4` = 飞行模式（射频关闭），`1` = 正常在线。
//! 个别机型（如 F50）未提供该指令，则 `supported=false`，界面隐藏开关。

use serde::Serialize;

use super::super::at::{csv, extract_field, is_ok};
use super::modem::Modem;

/// 「该机型不支持关射频」的统一说明（执行报错与定时器预检共用一句，别写岔）。
pub fn airplane_blocked_reason() -> &'static str {
    "该机型不支持关闭射频（关掉会连设备自身的管理页 / 内网一起断）"
}

/// 飞行模式状态。
#[derive(Debug, Clone, Serialize)]
pub struct Airplane {
    pub supported: bool,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl Modem {
    /// 读取飞行模式状态。
    pub fn airplane(&self) -> Airplane {
        let resp = self
            .with_at(|p| p.command("AT+CFUN?", self.cmd_timeout))
            .unwrap_or_default();
        if is_ok(&resp) {
            if let Some(level) = parse_cfun(&resp) {
                return Airplane {
                    supported: true,
                    enabled: level == 4,
                    reason: None,
                    raw: Some(resp.trim().to_string()),
                };
            }
        }
        Airplane {
            supported: false,
            enabled: false,
            reason: Some("固件未提供 AT+CFUN? 查询".to_string()),
            raw: Some(resp.trim().to_string()),
        }
    }
}

/// `+CFUN: <fun>` → 功能级别。
pub fn parse_cfun(resp: &str) -> Option<i32> {
    let field = extract_field(resp, "+CFUN")?;
    csv(&field).first()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_cfun;

    #[test]
    fn cfun_levels() {
        assert_eq!(parse_cfun("+CFUN: 1\r\nOK"), Some(1));
        assert_eq!(parse_cfun("+CFUN: 4\r\nOK"), Some(4));
        assert_eq!(parse_cfun("ERROR"), None);
    }
}
