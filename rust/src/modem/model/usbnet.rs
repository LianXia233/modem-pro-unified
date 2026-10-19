//! USB 网卡模式（`GET /api/usbnet`、`POST /api/usbnet/set`）。
//!
//! ## 契约（与 WebUI 一致）
//!
//! ```json
//! {"value":"3","name":"RNDIS","supported":true,"need_reboot":true,
//!  "modes":[{"value":"0","name":"RMNET"},…],
//!  "mode_cmds":{"0":"AT+QCFG=\"usbnet\",0",…},
//!  "blocked_modes":{"3":"切 RNDIS 会整机掉 USB"},
//!  "cmd":"AT+QCFG=\"usbnet\"","note":"…","raw":"+QCFG: \"usbnet\",3"}
//! ```
//!
//! 取值一律用**字符串**：页面下拉的 `option.value` 就是它，直接比较/回填，
//! 不会出现 `"3" !== 3` 这种「明明有值却选不中」的问题。
//!
//! ## 方言差异
//!
//! 目前只有移远 / 美格那套 `AT+QCFG="usbnet"` 可读可写（0=RMNET / 1=ECM /
//! 2=MBIM / 3=RNDIS；移远另有 5=NCM）。其余厂商：
//!
//! - 读：没有这条指令 → 仍给出可选档位（页面只展示），`cmd` 为空；
//! - 写：`VendorSpec::usbnet_write()` 决定，拒绝的原因进 `blocked_modes`，
//!   页面把那一档置灰并写明为什么（例如广和通切 RNDIS 会整机掉 USB）。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

use crate::error::{msg, Result};

use super::super::at::{csv, extract_field, is_ok};
use super::super::vendor::{UsbnetWrite, Vendor};
use super::modem::Modem;

/// 一个可选档位。
#[derive(Debug, Clone, Serialize)]
pub struct UsbNetOption {
    /// 取值（字符串：与页面 `option.value` 同类型）
    pub value: String,
    /// 展示名（RMNET / ECM / MBIM / RNDIS / NCM）
    pub name: String,
    /// 该档的补充说明（不可执行时写原因）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// USB 网卡模式状态。
#[derive(Debug, Clone, Serialize)]
pub struct UsbNetMode {
    /// 当前取值（读不到为 `""`）
    pub value: String,
    /// 当前档位的展示名（表外取值为空）
    pub name: String,
    /// 可选档位（按取值升序）
    pub modes: Vec<UsbNetOption>,
    /// 本机固件是否提供这条查询指令
    pub supported: bool,
    /// 修改后需重启模组才生效
    pub need_reboot: bool,
    /// 只展示、不允许执行的取值 → 原因
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub blocked_modes: BTreeMap<String, String>,
    /// 本机实际使用的查询指令（现场核对用）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cmd: String,
    /// 取值 → 该档的完整切换指令
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mode_cmds: BTreeMap<String, String>,
    /// 本机说明（取值含义 / 需要重启 / 证据来源）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// 固件原始应答
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// 查询指令（只有 QCFG 方言有）。
const QCFG: &str = "AT+QCFG=\"usbnet\"";

impl Modem {
    /// 读取 USB 网卡模式。
    pub fn usbnet(&self) -> UsbNetMode {
        let vendor = self.vendor();
        let resp = self
            .with_at(|p| p.command(QCFG, self.cmd_timeout))
            .unwrap_or_default();
        let parsed = is_ok(&resp).then(|| parse_usbnet(&resp)).flatten();

        // 档位与写策略**与读成功与否无关**：读不到也要让页面知道有哪些档、
        // 哪些档在这台机器上不能动（否则下拉是空的，用户以为功能坏了）。
        let (modes, blocked) = plan_modes(vendor);
        let supported = parsed.is_some() || !modes.is_empty();
        let value = parsed.map(|v| v.to_string()).unwrap_or_default();
        let name = modes
            .iter()
            .find(|m| m.value == value)
            .map(|m| m.name.clone())
            .unwrap_or_default();

        let mut mode_cmds = BTreeMap::new();
        if supports_qcfg(vendor) {
            for m in &modes {
                if !blocked.contains_key(&m.value) {
                    mode_cmds.insert(m.value.clone(), format!("AT+QCFG=\"usbnet\",{}", m.value));
                }
            }
        }

        let note = if parsed.is_none() {
            if supports_qcfg(vendor) {
                format!("读不到当前取值（固件未应答 {QCFG}），档位来自本机取值表")
            } else {
                "本机固件没有 AT+QCFG=\"usbnet\" 查询指令；下面只是该厂商支持的档位（只读）"
                    .to_string()
            }
        } else if value == "3" {
            "RNDIS 模式下 USB 组成可能没有 AT 口，改动前请确认还能用串口/网口进机".to_string()
        } else {
            "修改后需重启模组生效".to_string()
        };

        UsbNetMode {
            value,
            name,
            modes,
            supported,
            need_reboot: true,
            blocked_modes: blocked,
            cmd: if supports_qcfg(vendor) {
                QCFG.into()
            } else {
                String::new()
            },
            mode_cmds,
            note,
            raw: (!resp.trim().is_empty()).then(|| resp.trim().to_string()),
        }
    }

    /// 设置 USB 网卡模式（`AT+QCFG="usbnet",<value>`），写完回读确认。
    ///
    /// ⚠️ 只有移远/美格这套 `AT+QCFG` 能写；**广和通 RNDIS 不允许执行**
    /// （切过去会整机掉 USB，只能看不能动），其余厂商没有可写指令就如实报错。
    /// 写完**要重启模组**才生效 —— 这是模组侧的硬约束，后端不代劳。
    pub fn set_usbnet(&self, value: i32) -> Result<UsbNetMode> {
        if !(0..=5).contains(&value) {
            return Err(msg(
                "USB 网卡模式取值只认 0~5（RMNET / ECM / MBIM / RNDIS / NCM）",
            ));
        }
        match self.vendor().spec().usbnet_write(value as u8) {
            UsbnetWrite::Allow => {}
            UsbnetWrite::Deny(why) => return Err(msg(why)),
        }
        let cmd = format!("AT+QCFG=\"usbnet\",{value}");
        let resp = self.with_at(|p| p.command(&cmd, Duration::from_secs(5)))?;
        if !is_ok(&resp) {
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        let st = self.usbnet();
        if st.value != value.to_string() {
            return Err(msg(format!(
                "已下发但回读为 {}{}（目标 {value}）",
                if st.value.is_empty() { "-" } else { &st.value },
                if st.name.is_empty() {
                    String::new()
                } else {
                    format!(" {}", st.name)
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

/// 这家厂商有没有 QCFG 这条查询指令（读也只对这一族有意义）。
fn supports_qcfg(vendor: Vendor) -> bool {
    matches!(vendor, Vendor::Quectel | Vendor::Meig)
}

/// 档位表 + 不可执行档位：**由厂商写策略推导**，不在这里写死 `match vendor`。
fn plan_modes(vendor: Vendor) -> (Vec<UsbNetOption>, BTreeMap<String, String>) {
    let spec = vendor.spec();
    let mut modes = Vec::new();
    let mut blocked = BTreeMap::new();
    // 0~3 是跨厂商共同档位；5（NCM）只有移远/美格取值表里认
    let values: &[u8] = if supports_qcfg(vendor) {
        &[0, 1, 2, 3, 5]
    } else {
        &[0, 1, 2, 3]
    };
    for v in values {
        let value = v.to_string();
        let name = label_of(*v).to_string();
        match spec.usbnet_write(*v) {
            UsbnetWrite::Allow => modes.push(UsbNetOption {
                value,
                name,
                note: String::new(),
            }),
            UsbnetWrite::Deny(why) => {
                blocked.insert(value.clone(), why.to_string());
                modes.push(UsbNetOption {
                    value,
                    name,
                    note: why.to_string(),
                });
            }
        }
    }
    (modes, blocked)
}

fn label_of(value: u8) -> String {
    match value {
        0 => "RMNET",
        1 => "ECM",
        2 => "MBIM",
        3 => "RNDIS",
        4 | 5 => "NCM",
        _ => "未知",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{parse_usbnet, plan_modes};
    use crate::modem::vendor::Vendor;

    #[test]
    fn qcfg_usbnet() {
        assert_eq!(parse_usbnet("+QCFG: \"usbnet\",1\r\nOK"), Some(1));
        assert_eq!(parse_usbnet("+QCFG: \"usbnet\",2\r\nOK"), Some(2));
        assert_eq!(parse_usbnet("+QCFG: \"other\",1\r\nOK"), None);
    }

    #[test]
    fn quectel_has_all_modes_including_ncm() {
        let (modes, blocked) = plan_modes(Vendor::Quectel);
        let values: Vec<&str> = modes.iter().map(|m| m.value.as_str()).collect();
        assert_eq!(values, vec!["0", "1", "2", "3", "5"]);
        assert!(blocked.is_empty(), "移远五个档位都可写");
    }

    #[test]
    fn fibocom_rndis_is_blocked_not_hidden() {
        // 不能把 RNDIS 从列表里删掉：页面要显示「当前就在这一档」并解释为什么不能动
        let (modes, blocked) = plan_modes(Vendor::Fibocom);
        assert!(modes.iter().any(|m| m.value == "3"));
        assert!(blocked.contains_key("3"));
        assert!(blocked["3"].contains("RNDIS"));
    }
}
