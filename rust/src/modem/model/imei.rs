//! 模组身份信息（`GET /api/modem/info`）：厂商、型号、IMEI、固件。

use std::time::Duration;

use serde::Serialize;

use crate::error::{msg, Result};

use super::super::at::is_ok;
use super::super::vendor::Vendor;
use super::modem::Modem;

/// 广和通 MTK 平台 `AT+EGMREXT` 族的**只读**形式（与写入指令
/// `AT+EGMREXT=1,7,"<imei>"` 同族同参位），用于写前快照与写后回读。
///
/// 实机验证（FM350，固件 <固件版本>）：
/// - `AT+EGMREXT=0,7` → `+EGMREXT: "<imei>"` + `OK`；
/// - `AT+EGMREXT?` → `+CME ERROR: 100` —— 查询形式不可用，只能 `=0,7` 这一种写法。
pub const EGMREXT_READ: &str = "AT+EGMREXT=0,7";

/// 模组身份信息。
#[derive(Debug, Clone, Serialize)]
pub struct ModemInfo {
    pub modem_id: u32,
    pub vendor: &'static str,
    pub vendor_name: &'static str,
    pub at_port: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imei: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
}

impl Modem {
    /// 读取一次身份信息（IMEI / 固件）。
    pub fn info(&self) -> ModemInfo {
        ModemInfo {
            modem_id: self.id,
            vendor: self.vendor().as_str(),
            vendor_name: self.vendor().label(),
            at_port: self.port(),
            model: self.model(),
            imei: self.imei(),
            firmware: self.firmware(),
        }
    }

    /// IMEI（`AT+CGSN`，退回 `AT+GSN`）。
    pub fn imei(&self) -> Option<String> {
        let resp = self
            .with_at(|p| p.command("AT+CGSN", self.cmd_timeout))
            .unwrap_or_default();
        parse_imei(&resp).or_else(|| {
            let resp = self
                .with_at(|p| p.command("AT+GSN", self.cmd_timeout))
                .unwrap_or_default();
            parse_imei(&resp)
        })
    }

    /// 固件版本（`AT+CGMR` 原文）。
    pub fn firmware(&self) -> Option<String> {
        let resp = self
            .with_at(|p| p.command("AT+CGMR", self.cmd_timeout))
            .unwrap_or_default();
        resp.lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && *line != "OK" && *line != "AT+CGMR")
            .map(str::to_string)
    }
}

/// IMEI 管理状态（`GET /api/imei`）。
#[derive(Debug, Clone, Serialize)]
pub struct ImeiStatus {
    pub supported: bool,
    /// 本机写入指令（不支持时为空）
    pub cmd: String,
    /// 本机读取指令
    pub read_cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imei: Option<String>,
    /// 备份的原始值（没有就是还没改过）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_saved_at: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub writes: i64,
    /// 备份存在且当前值与它不同 ⇒ 可以一键恢复
    pub can_restore: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_at: Option<String>,
    /// 当前 IMEI 是否通过 Luhn 校验（IMEI 第 15 位是校验位）。
    ///
    /// **仅作提示，不阻断任何操作**：部分厂商的 IMEI 并不严格遵循 Luhn
    /// 校验位规则，拿它当准入门会把正常模组挡在门外。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub luhn_ok: Option<bool>,
    /// 本次操作的附加提示（如广和通 MTK 平台「需重启模组后生效」）。
    ///
    /// 只由 `/api/imei/set`、`/api/imei/restore` 的当次响应携带，
    /// `GET /api/imei` 不带（没有正在进行的操作）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// Luhn 校验（IMEI 第 15 位为校验位）。
///
/// 纯函数，无副作用；长度不是 15 位或含非数字时返回 false。
pub fn luhn_ok(imei: &str) -> bool {
    let digits: Vec<u32> = match imei.chars().map(|c| c.to_digit(10)).collect() {
        Some(v) => v,
        None => return false,
    };
    if digits.len() != 15 {
        return false;
    }
    let mut sum = 0u32;
    for (i, d) in digits.iter().enumerate() {
        // 从右往左第 2 位起，每隔一位乘 2
        if (14 - i) % 2 == 1 {
            let mut v = d * 2;
            if v > 9 {
                v -= 9;
            }
            sum += v;
        } else {
            sum += *d;
        }
    }
    sum % 10 == 0
}

impl Modem {
    /// 读取 IMEI 管理状态（`backup` 为 `imei.json` 里这台模组的备份）。
    ///
    /// `warning` 是当次写操作带回的提示（仅 set/restore 响应携带），只透传、
    /// 不参与任何判定。
    pub fn imei_status(
        &self,
        backup: Option<&crate::runtime::store::ImeiBackup>,
        warning: Option<&str>,
    ) -> ImeiStatus {
        let imei = self.imei();
        let writable = self.imei_write_cmd().is_some();
        let original = backup.map(|b| b.original.clone());
        let can_restore = match (&original, &imei) {
            (Some(o), Some(cur)) => !o.is_empty() && o != cur,
            _ => false,
        };
        ImeiStatus {
            supported: writable,
            cmd: self.imei_write_cmd().unwrap_or_default().to_string(),
            read_cmd: "AT+CGSN".to_string(),
            note: Some(if writable {
                if self.is_fibocom_mtk() {
                    // 与写入指令同族的只读快照；该平台写入需重启模组后才生效
                    "只接受 15 位数字；写前用同族只读命令 AT+EGMREXT=0,7 快照备份，写后回读确认（本平台需重启模组后才生效）"
                        .to_string()
                } else {
                    "只接受 15 位数字；写入后回读 AT+CGSN 确认，没生效会如实报错".to_string()
                }
            } else {
                "本机不支持改写 IMEI（只读）：本程序不发未经验证的厂商私有指令".to_string()
            }),
            imei: imei.clone(),
            original: original.clone(),
            original_saved_at: backup.map(|b| b.saved_at.clone()),
            writes: backup.map(|b| b.writes).unwrap_or(0),
            can_restore,
            restored_at: backup.and_then(|b| b.restored_at.clone()),
            luhn_ok: imei.as_deref().filter(|s| !s.is_empty()).map(luhn_ok),
            warning: warning.map(str::to_string),
        }
    }

    /// 本机的 IMEI 写入指令（认不出来的机型返回 None —— 不乱发私有指令）。
    ///
    /// 三条命令族的取证程度不同，注释里写清楚，便于排障时判断该不该信：
    /// - 移远 / 美格：`AT+EGMR`，高通平台通用，手册收录；
    /// - 广和通 MTK 平台（FM350 / RW350 系列）：`AT+EGMREXT`，
    ///   **未被广和通 FM350 AT 手册（v2.2 / v2.10）收录**，属社区与实机
    ///   验证可用的扩展命令。同族只读形式 `AT+EGMREXT=0,7` 已在本机实机
    ///   验证（`AT+EGMREXT?` 报 `+CME ERROR: 100`，查询写法不可用），
    ///   写前快照与写后回读都走它。本程序已有的三重保护仍然生效：15 位数字
    ///   校验、改写前备份、下发后回读确认，所以它的风险与其它两家同级。
    pub fn imei_write_cmd(&self) -> Option<&'static str> {
        match self.vendor() {
            // 移远 / 美格走高通那套 EGMR
            Vendor::Quectel | Vendor::Meig => Some("AT+EGMR=1,7,"),
            // 广和通 MTK 平台走扩展版 EGMREXT，且必须先过机型闸
            Vendor::Fibocom if self.is_fibocom_mtk() => Some("AT+EGMREXT=1,7,"),
            _ => None,
        }
    }

    /// 写前快照：用**与写入指令同族**的只读形式读当前值，读不到再退回 `AT+CGSN`。
    ///
    /// 返回 `(值, 来源命令)`；值可能为空串 —— 模组明确应答空 IMEI 是合法的
    /// 出厂 / 维修状态（空 IMEI 模组正是最需要改写的场景）。返回 `None` 表示
    /// 两条路都没读到可识别的值，此时**不许当备份**（备份必须是读到的值或
    /// 明确的空，不能是「读不到」）。
    fn imei_snapshot(&self) -> Option<(String, &'static str)> {
        if self.is_fibocom_mtk() {
            let resp = self
                .with_at(|p| p.command(EGMREXT_READ, self.cmd_timeout))
                .unwrap_or_default();
            if let Some(v) = parse_egmrext(&resp) {
                return Some((v, EGMREXT_READ));
            }
        }
        self.imei().map(|v| (v, "AT+CGSN"))
    }

    /// 写 IMEI：先备份当前值（第一次改写时它就是原始值）→ 下发 → 回读确认。
    ///
    /// 返回 `(写入值, 提示, 备份)`；提示非空表示「已下发但需重启模组后复查」。
    /// `backup` 由调用方负责落盘。
    pub fn set_imei(
        &self,
        imei: &str,
        backup: Option<crate::runtime::store::ImeiBackup>,
    ) -> Result<(String, Option<String>, crate::runtime::store::ImeiBackup)> {
        let imei = imei.trim();
        if !is_imei_text(imei) {
            return Err(msg("IMEI 必须是 15 位数字"));
        }
        let Some(cmd) = self.imei_write_cmd() else {
            return Err(msg("本机不支持改写 IMEI"));
        };
        // 改写前先留一份：第一次改写时读到的就是原始值
        let mut bak = backup.unwrap_or_default();
        if bak.saved_at.is_empty() {
            match self.imei_snapshot() {
                Some((v, src)) => record_original(&mut bak, &v, src),
                None => return Err(msg("读不到当前 IMEI，无法备份原始值（拒绝改写）")),
            }
        }
        let warning = self.write_imei(cmd, imei)?;
        bak.writes += 1;
        Ok((imei.to_string(), warning, bak))
    }

    /// 恢复备份里的原始 IMEI。
    pub fn restore_imei(
        &self,
        backup: Option<crate::runtime::store::ImeiBackup>,
    ) -> Result<(String, Option<String>, crate::runtime::store::ImeiBackup)> {
        let Some(cmd) = self.imei_write_cmd() else {
            return Err(msg("本机不支持改写 IMEI"));
        };
        let mut bak = backup.ok_or_else(|| msg("这台模组还没有保存过原始 IMEI"))?;
        if bak.saved_at.is_empty() {
            return Err(msg("这台模组还没有保存过原始 IMEI"));
        }
        if bak.original.is_empty() {
            // 原值就是空（模组出厂 / 维修状态）：把 IMEI 写成空没有意义
            return Err(msg("备份的原始值为空（模组出厂即无 IMEI），无可恢复"));
        }
        let warning = self.write_imei(cmd, &bak.original.clone())?;
        bak.restored_at = Some(crate::logging::timestamp());
        Ok((bak.original.clone(), warning, bak))
    }

    /// 下发写入指令并回读确认（模组回 OK 不代表真写进去了）。
    ///
    /// 回读口径按平台分流：
    /// - **广和通 MTK**：`AT+EGMREXT` 写入**需模组重启后才在射频 / 网络侧生效**
    ///   （参考实现 1.0.14-r2 的实机记录），所以回读到空值 / 旧值都可能是
    ///   「还没生效」而非「写失败」—— 只把「模组明确报错」当失败，其余情况
    ///   附重启提示返回成功，让调用方如实告知用户；
    /// - **其它厂商**（高通 `AT+EGMR`）：维持原语义，回读不一致即报错。
    fn write_imei(&self, cmd: &str, imei: &str) -> Result<Option<String>> {
        let resp =
            self.with_at(|p| p.command(&format!("{cmd}\"{imei}\""), Duration::from_secs(5)))?;
        if !is_ok(&resp) {
            return Err(msg(format!("写入失败：{}", resp.trim())));
        }
        let back = self.imei_snapshot().map(|(v, _)| v).unwrap_or_default();
        if back == imei {
            return Ok(None);
        }
        if self.is_fibocom_mtk() {
            return Ok(Some(format!(
                "写入命令已下发且模组未报错，但回读为「{}」—— 广和通 MTK 平台的 AT+EGMREXT 需重启模组后才生效，请重启后复查",
                if back.is_empty() { "空" } else { &back }
            )));
        }
        Err(msg(format!(
            "已下发但回读为「{}」（目标 {imei}）",
            if back.is_empty() { "空" } else { &back }
        )))
    }
}

/// 把「第一次改写前读到的原值」记进备份台账。
///
/// 以 `saved_at` 为空判定「还没备份过」—— **不能以 `original` 为空判定**：
/// 空 IMEI 模组的原值就是空串，那样第二次改写会把原值覆盖成上一次写入的值，
/// 一键恢复从此失效。
fn record_original(bak: &mut crate::runtime::store::ImeiBackup, original: &str, source: &str) {
    if bak.saved_at.is_empty() {
        bak.original = original.to_string();
        bak.saved_at = crate::logging::timestamp();
        bak.source = source.to_string();
    }
}

/// IMEI 文本校验：15 位数字。
pub fn is_imei_text(s: &str) -> bool {
    s.len() == 15 && s.chars().all(|c| c.is_ascii_digit())
}

/// 从 `AT+CGSN` 应答里取 15 位 IMEI。
pub fn parse_imei(resp: &str) -> Option<String> {
    if let Some(field) = super::super::at::extract_field(resp, "+CGSN") {
        let digits: String = field.chars().filter(char::is_ascii_digit).collect();
        if digits.len() == 15 {
            return Some(digits);
        }
    }
    for line in resp.lines() {
        let line = line.trim();
        if line.len() == 15 && line.chars().all(|c| c.is_ascii_digit()) {
            return Some(line.to_string());
        }
    }
    None
}

/// 解析 `AT+EGMREXT=0,7`（广和通 MTK 平台同族只读形式）的应答。
///
/// 三态，区别很重要：
/// - `Some("")`：模组**明确应答空值**（`+EGMREXT: ""`）—— 空 IMEI 是合法的
///   出厂 / 维修状态，备份空值本身就是有效的状态记录；
/// - `Some(imei)`：读到 15 位 IMEI；
/// - `None`：应答里没有 `+EGMREXT` 行、或内容不是 15 位数字（如 `?` 形式回的
///   `+CME ERROR: 100`）—— 读不到，调用方应退回 `AT+CGSN`，**不许**当备份。
pub fn parse_egmrext(resp: &str) -> Option<String> {
    let mut saw_empty = false;
    for line in resp.lines() {
        let line = line.trim();
        if !line.starts_with("+EGMREXT") {
            continue;
        }
        let Some(pos) = line.find(':') else { continue };
        let v = line[pos + 1..].trim().trim_matches('"').trim();
        if v.is_empty() {
            saw_empty = true;
        } else if v.len() == 15 && v.chars().all(|c| c.is_ascii_digit()) {
            return Some(v.to_string());
        }
    }
    if saw_empty {
        Some(String::new())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{is_imei_text, luhn_ok, parse_egmrext, parse_imei};
    use crate::runtime::store::ImeiBackup;

    #[test]
    fn imei_text_must_be_15_digits() {
        assert!(is_imei_text("867698040012349"));
        assert!(!is_imei_text("86769804001234"));
        assert!(!is_imei_text("8676980400123456"));
        assert!(!is_imei_text("86769804001234x"));
    }

    #[test]
    fn imei_digits() {
        assert_eq!(
            parse_imei("867698040012349\r\nOK").as_deref(),
            Some("867698040012349")
        );
        assert_eq!(
            parse_imei("\r\n+CGSN: 867698040012349\r\nOK").as_deref(),
            Some("867698040012349")
        );
        assert_eq!(parse_imei("ERROR"), None);
    }

    #[test]
    fn luhn_of_valid_imei() {
        assert!(luhn_ok("490154203237518"));
        // 另一个合法样例（原为实机读数，已脱敏）
        assert!(luhn_ok("867698040012349"));
    }

    #[test]
    fn luhn_rejects_bad_checksum() {
        assert!(!luhn_ok("490154203237519"));
        assert!(!luhn_ok("4901542032375188"));
        assert!(!luhn_ok("49015420323751"));
        assert!(!luhn_ok("49015420323751X"));
        assert!(!luhn_ok(""));
    }

    // ---- AT+EGMREXT=0,7 只读形式（样本取自实机报文）----

    #[test]
    fn egmrext_reads_quoted_imei() {
        // 实机原样：AT+EGMREXT=0,7 → +EGMREXT: "<imei>" + OK
        assert_eq!(
            parse_egmrext("\r\n+EGMREXT: \"867698040012349\"\r\nOK").as_deref(),
            Some("867698040012349")
        );
        // 引号里有空格也认（trim 之后仍是 15 位）
        assert_eq!(
            parse_egmrext("+EGMREXT: \" 867698040012349 \"").as_deref(),
            Some("867698040012349")
        );
    }

    #[test]
    fn egmrext_empty_is_a_valid_state() {
        // 空 IMEI 模组（维修场景）：明确应答空值 ⇒ Some("")，不是解析失败
        assert_eq!(parse_egmrext("+EGMREXT: \"\"\r\nOK").as_deref(), Some(""));
        assert_eq!(parse_egmrext("+EGMREXT:").as_deref(), Some(""));
    }

    #[test]
    fn egmrext_unrecognized_is_none() {
        // 查询形式 AT+EGMREXT? 实机回 +CME ERROR: 100 ⇒ 读不到
        assert_eq!(parse_egmrext("\r\n+CME ERROR: 100"), None);
        assert_eq!(parse_egmrext("ERROR"), None);
        assert_eq!(parse_egmrext(""), None);
        // 有 +EGMREXT 头但内容不是 15 位数字 ⇒ 同样读不到
        assert_eq!(parse_egmrext("+EGMREXT: \"123\""), None);
        assert_eq!(parse_egmrext("+EGMREXT: \"8676980400123456\""), None);
        assert_eq!(parse_egmrext("+EGMREXT: \"86769804001234x\""), None);
    }

    #[test]
    fn egmrext_first_valid_line_wins() {
        // 多条 +EGMREXT 时取第一条有效值；空行不抢先
        assert_eq!(
            parse_egmrext("+EGMREXT: \"\"\r\n+EGMREXT: \"867698040012349\"").as_deref(),
            Some("867698040012349")
        );
    }

    // ---- 备份台账：以 saved_at 判定「还没备份过」----

    #[test]
    fn record_original_keeps_first_value() {
        let mut bak = ImeiBackup::default();
        super::record_original(&mut bak, "490154203237518", "AT+EGMREXT=0,7");
        assert_eq!(bak.original, "490154203237518");
        assert_eq!(bak.source, "AT+EGMREXT=0,7");
        assert!(!bak.saved_at.is_empty());
        // 第二次改写不许覆盖原值
        super::record_original(&mut bak, "867698040012349", "AT+CGSN");
        assert_eq!(bak.original, "490154203237518");
        assert_eq!(bak.source, "AT+EGMREXT=0,7");
    }

    #[test]
    fn record_original_empty_original_is_kept() {
        // 空 IMEI 模组：原值就是空串，且**必须**被当成「已备份」，
        // 否则下一次改写会把原值覆盖成上一次写进去的值
        let mut bak = ImeiBackup::default();
        super::record_original(&mut bak, "", "AT+EGMREXT=0,7");
        assert!(bak.original.is_empty());
        assert!(!bak.saved_at.is_empty());
        super::record_original(&mut bak, "867698040012349", "AT+EGMREXT=0,7");
        assert!(bak.original.is_empty(), "空原值被下一次写入覆盖了");
    }
}
