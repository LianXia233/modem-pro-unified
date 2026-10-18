//! USB 网卡模式（`GET /api/usbnet`）。
//!
//! 移远 `AT+QCFG="usbnet"` → `+QCFG: "usbnet",<value>`：
//! 0=RMNET / 1=ECM / 2=MBIM / 3=RNDIS。改动需重启模组生效。

use std::time::Duration;

use serde::Serialize;

use crate::error::{msg, Result};

use super::super::at::{csv, extract_field, is_ok};
use super::modem::Modem;

/// 一个可选值。
#[derive(Debug, Clone, Serialize)]
pub struct UsbNetOption {
    pub value: i32,
    pub label: String,
}

/// USB 网卡模式。
#[derive(Debug, Clone, Serialize)]
pub struct UsbNetMode {
    pub value: i32,
    pub value_text: String,
    pub options: Vec<UsbNetOption>,
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Modem {
    /// 读取 USB 网卡模式。
    pub fn usbnet(&self) -> UsbNetMode {
        let resp = self
            .with_at(|p| p.command("AT+QCFG=\"usbnet\"", self.cmd_timeout))
            .unwrap_or_default();
        if is_ok(&resp) {
            if let Some(value) = parse_usbnet(&resp) {
                return UsbNetMode {
                    value,
                    value_text: label_of(value),
                    options: options(),
                    supported: true,
                    raw: Some(resp.trim().to_string()),
                    note: Some("修改后需重启模组生效".to_string()),
                };
            }
        }
        UsbNetMode {
            value: -1,
            value_text: String::new(),
            options: Vec::new(),
            supported: false,
            raw: Some(resp.trim().to_string()),
            note: Some("固件未提供 USB 网卡模式指令".to_string()),
        }
    }

    /// 设置 USB 网卡模式（`AT+QCFG="usbnet",<value>`），写完回读确认。
    ///
    /// ⚠️ 只有移远/美格这套 `AT+QCFG` 能写；**广和通 RNDIS 不允许执行**
    /// （切过去会整机掉 USB，只能看不能动），其余厂商没有可写指令就如实报错。
    /// 写完**要重启模组**才生效 —— 这是模组侧的硬约束，后端不代劳。
    pub fn set_usbnet(&self, value: i32) -> Result<UsbNetMode> {
        if !(0..=3).contains(&value) {
            return Err(msg(
                "USB 网卡模式取值只认 0~3（RMNET / ECM / MBIM / RNDIS）",
            ));
        }
        match self.vendor().spec().usbnet_write(value as u8) {
            crate::modem::vendor::UsbnetWrite::Allow => {}
            crate::modem::vendor::UsbnetWrite::Deny(why) => return Err(msg(why)),
        }
        let cmd = format!("AT+QCFG=\"usbnet\",{value}");
        let resp = self.with_at(|p| p.command(&cmd, Duration::from_secs(5)))?;
        if !is_ok(&resp) {
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        let st = self.usbnet();
        if st.value != value {
            return Err(msg(format!(
                "已下发但回读为 {}{}（目标 {value}）",
                st.value,
                if st.value_text.is_empty() {
                    String::new()
                } else {
                    format!(" {}", st.value_text)
                }
            )));
        }
        Ok(st)
    }
}

/// 从 `+QCFG: "usbnet",<value>` 取取值。
pub fn parse_usbnet(resp: &str) -> Option<i32> {
    let field = extract_field(resp, "+QCFG")?;
    let parts = csv(&field);
    if parts.first().map(|s| s == "usbnet") != Some(true) {
        return None;
    }
    parts.get(1)?.parse().ok()
}

fn label_of(value: i32) -> String {
    match value {
        0 => "RMNET",
        1 => "ECM",
        2 => "MBIM",
        3 => "RNDIS",
        4 => "NCM",
        _ => "未知",
    }
    .to_string()
}

fn options() -> Vec<UsbNetOption> {
    vec![
        UsbNetOption {
            value: 0,
            label: "RMNET".into(),
        },
        UsbNetOption {
            value: 1,
            label: "ECM".into(),
        },
        UsbNetOption {
            value: 2,
            label: "MBIM".into(),
        },
        UsbNetOption {
            value: 3,
            label: "RNDIS".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::parse_usbnet;

    #[test]
    fn qcfg_usbnet() {
        assert_eq!(parse_usbnet("+QCFG: \"usbnet\",1\r\nOK"), Some(1));
        assert_eq!(parse_usbnet("+QCFG: \"usbnet\",2\r\nOK"), Some(2));
        assert_eq!(parse_usbnet("+QCFG: \"other\",1\r\nOK"), None);
    }
}
