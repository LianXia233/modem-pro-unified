//! 设备级操作：重启模组（`AT+CFUN=1,1`）与校时（`AT+CCLK="…"`）。
//!
//! 对应 `POST /api/modem/reset` 与 `POST /api/modem/clock`。

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::error::{msg, Result};
use crate::logging::{civil_from_days, unix_now};

use super::super::at::{is_ok, Prio};
use super::modem::Modem;

/// 重启结果。
///
/// `confirmed=false` 表示指令已写进串口但没等到确认 —— 重启本来就会让 AT 口断开，
/// 模组还没回 OK 就掉线是常态，所以这种情况**不算失败**（调用方按成功上报，
/// 由界面提示用户「约 30 秒恢复」）。
#[derive(Debug, Clone, Serialize)]
pub struct RebootResult {
    pub modem_id: u32,
    pub cmd: String,
    pub confirmed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 校时结果：实际下发的那条 `AT+CCLK`（回退过的话是第二条）。
#[derive(Debug, Clone, Serialize)]
pub struct ClockSyncResult {
    pub modem_id: u32,
    /// 实际生效的指令（带回读确认）
    pub cmd: String,
    /// 写进去的时间文本（`yy/MM/dd,HH:mm:ss+00`）
    pub time: String,
    /// 是否走了「不带时区」的回退分支
    pub fallback: bool,
}

impl Modem {
    /// 重启模组：`AT+CFUN=1,1`（设置并复位）。
    ///
    /// 约 30 秒后 AT 口恢复；期间信号 / 注册 / 扫描全部中断。
    pub fn reboot(&self) -> Result<RebootResult> {
        let cmd = "AT+CFUN=1,1";
        let resp = self.with_at_prio(Prio::Data, |p| p.command(cmd, self.cmd_timeout))?;
        if is_ok(&resp) {
            log::info!("模组 {} 重启指令已确认（{cmd}）", self.id);
            Ok(RebootResult {
                modem_id: self.id,
                cmd: cmd.to_string(),
                confirmed: true,
                note: None,
            })
        } else {
            // 写下去了但没读到 OK：多半是已经开始重启、AT 口提前断开
            log::warn!("模组 {} 重启指令已下发但未确认（{}）", self.id, resp.trim());
            Ok(RebootResult {
                modem_id: self.id,
                cmd: cmd.to_string(),
                confirmed: false,
                note: Some("指令已下发但未收到确认（重启会断开 AT 口，属预期）".to_string()),
            })
        }
    }

    /// 把模组时钟校准到主机当前 UTC 时间。
    ///
    /// 3GPP 格式 `AT+CCLK="yy/MM/dd,HH:mm:ss±zz"`，这里统一写 `+00`（UTC）——
    /// 读侧 `parse_cclk` 也是按 UTC 解释字段的，校完 `clock_skew_s` 应该回到 0 附近。
    /// 个别固件不收时区尾巴：先试带 `+00` 的，回 ERROR 再退一格发不带时区的。
    /// 两条都失败才算失败（返回 Err）。
    pub fn sync_clock(&self) -> Result<ClockSyncResult> {
        let (date, hms) = cclk_now();
        let with_tz = format!("{date},{hms}+00");
        let without_tz = format!("{date},{hms}");
        let resp = self.with_at_prio(Prio::Data, |p| {
            p.command(&format!("AT+CCLK=\"{with_tz}\""), self.cmd_timeout)
        })?;
        if is_ok(&resp) {
            return Ok(ClockSyncResult {
                modem_id: self.id,
                cmd: format!("AT+CCLK=\"{with_tz}\""),
                time: with_tz,
                fallback: false,
            });
        }
        log::info!(
            "模组 {} 不接受带时区的 CCLK（{}），回退为不带时区",
            self.id,
            resp.trim()
        );
        let resp2 = self.with_at_prio(Prio::Data, |p| {
            p.command(&format!("AT+CCLK=\"{without_tz}\""), self.cmd_timeout)
        })?;
        if is_ok(&resp2) {
            return Ok(ClockSyncResult {
                modem_id: self.id,
                cmd: format!("AT+CCLK=\"{without_tz}\""),
                time: without_tz,
                fallback: true,
            });
        }
        Err(msg(format!("校时失败：{}", resp2.trim())))
    }
}

/// 当前 UTC 时间 →（`yy/MM/dd`, `HH:mm:ss`）。
fn cclk_now() -> (String, String) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(|_| unix_now());
    let days = now.div_euclid(86_400);
    let rem = now.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    // 两位年份：2000 + (y % 100)
    let yy = y.rem_euclid(100);
    (
        format!("{yy:02}/{m:02}/{d:02}"),
        format!("{:02}:{:02}:{:02}", rem / 3600, (rem % 3600) / 60, rem % 60),
    )
}

#[cfg(test)]
mod tests {
    use super::cclk_now;

    #[test]
    fn cclk_text_shape() {
        let (date, hms) = cclk_now();
        assert_eq!(date.len(), 8, "yy/MM/dd 8 字符: {date}");
        assert_eq!(hms.len(), 8, "HH:mm:ss 8 字符: {hms}");
        assert!(date.chars().nth(2) == Some('/') && date.chars().nth(5) == Some('/'));
        assert!(hms.chars().nth(2) == Some(':') && hms.chars().nth(5) == Some(':'));
        // 年份两位、月日时分秒都在合理量纲
        let parts: Vec<u32> = date
            .split('/')
            .chain(hms.split(':'))
            .map(|p| p.parse().unwrap())
            .collect();
        assert!(parts[1] >= 1 && parts[1] <= 12, "月: {}", parts[1]);
        assert!(parts[2] >= 1 && parts[2] <= 31, "日: {}", parts[2]);
        assert!(parts[3] <= 23 && parts[4] <= 59 && parts[5] <= 59);
    }
}
