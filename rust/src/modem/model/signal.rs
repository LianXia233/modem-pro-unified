//! 信号查询。
//!
//! 口径与前端 `renderSignal` 一致：`SignalInfo { rssi, ber, nr_rsrp, nr_rsrq, nr_sinr }`，
//! 其中 `rssi` 是 `AT+CSQ` 的 0..31 索引（99/255 表示未上报），NR 三值来自
//! 服务小区指令（`AT+QENG="servingcell"`），取不到时退回 3GPP 标准的 `AT+CESQ`。
//!
//! 广和通 MTK 平台（FM350 / RW350 系列）是例外：它**没有** `AT+QENG`
//! （移远私有指令，发过去只会白等一个超时），服务小区信息在 `AT+GTCCINFO?`
//! 的行表里（见 [`super::cells::parse_gtccinfo_rows`]）；且 LTE 制式下
//! `AT+CESQ` 不带 SINR 格，SINR 只能取服务小区行的 `<rssnr_value>`。
//!
//! CESQ 与 GTCCINFO 的信号格都是**阶梯索引**（不是 dBm / dB），换算统一走本模块
//! 的索引函数：口径取 3GPP 区间下界（索引 1 = [-140,-139) dBm → -140 dBm），与
//! 3GPP TS 27.007 §8.69 和《FM350 AT Commands》§11.1.15 一致。
//!
//! ⚠️ 与上游 Go 的 1 dB 差（有意保留，不改）：Go `module/cell/cell.go:573`
//! `ParseCESQSignal` 按区间**上界**算（`rsrp = v-140`、`rsrq = (v-39)/2`），
//! 本仓库取下界（`v-141` / `(v-40)/2`），因此同一个索引本仓库比上游低 1 dB。
//! 改它会让页面与历史曲线的所有信号值整体平移，属输出口径变更，见 A03 报告。
//!
//! 无效值哨兵：`AT+CSQ` 的 99 / 255、`AT+CESQ` 各格的 255 都表示「未上报」；
//! 本模块把它们换算成 0，再由 `valid()` 按量纲滤掉 —— 前端
//! （`rust/web/index.html` 的 `sampleFromPush`）用同一套区间判定。

use serde::Serialize;

use crate::error::Result;

use super::super::at::{csv, extract_all, extract_field, field_part, is_ok, Prio};
use super::modem::Modem;

/// 信号信息（含 5G NR 小区信号）。
///
/// 字段名带 `nr_` 前缀是历史遗留（沿用 Go 上游 `SignalInfo` 的 JSON 契约）：
/// 三格装的是**当前驻留制式**的服务小区信号，纯 LTE 驻网时也是它，不是只装 NR。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SignalInfo {
    /// `AT+CSQ` 的 `<rssi>` 索引 0~31（对客户显示时按 `idx*2-113` 换算成 dBm）；
    /// `99` / `255` = 未上报。
    pub rssi: i32,
    /// `AT+CSQ` 的 `<ber>` 0~7（`99` = 未上报）；只在 2G 下有实际含义。
    pub ber: i32,
    /// RSRP dBm（量纲 -156~-44）；`0` = 未上报。
    #[serde(rename = "nr_rsrp", skip_serializing_if = "is_zero")]
    pub nr_rsrp: i32,
    /// RSRQ dB（量纲 -43~20）；`0` = 未上报。
    #[serde(rename = "nr_rsrq", skip_serializing_if = "is_zero")]
    pub nr_rsrq: i32,
    /// SINR dB（量纲 -23~40）；`0` = 未上报。
    #[serde(rename = "nr_sinr", skip_serializing_if = "is_zero")]
    pub nr_sinr: i32,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

impl Modem {
    /// 查询一次信号（CSQ + 服务小区/扩展信号）。
    pub fn signal(&self) -> Result<SignalInfo> {
        let (rssi, ber) = self
            .with_at_prio(Prio::Signal, |p| p.command("AT+CSQ", self.cmd_timeout))
            .map(|raw| parse_csq(&raw))
            .unwrap_or((99, 99));

        let (nr_rsrp, nr_rsrq, nr_sinr) = self.query_serving_signal();

        Ok(SignalInfo {
            rssi,
            ber,
            nr_rsrp,
            nr_rsrq,
            nr_sinr,
        })
    }

    /// 依次尝试服务小区指令与 `AT+CESQ`，取第一份有效值。
    ///
    /// 广和通 MTK 平台（FM350 / RW350 系列）没有 `AT+QENG`，服务小区改读
    /// `AT+GTCCINFO?` 的行表；该行表的 `<rsrp>` / `<rsrq>` 与 CESQ 同表，
    /// 在 CESQ 取不到时兜底，而 LTE 的 SINR（`<rssnr_value>`，本身就是 dB）
    /// 只有这里有 —— CESQ 的 LTE 分支不带 SINR 格。
    ///
    /// 鼎桥 MT5700（`^MONSC`）与芯讯通（`+CPSI`）同样不回 `+QENG`，但不必在这里
    /// 加分支：`+QENG` 在它们上面只是回 ERROR / 无应答，解不出自然落到 CESQ，
    /// 与扫小区（`cells.rs` 的 `Modem::serving_cell_live`）的分厂商分支不重复。
    fn query_serving_signal(&self) -> (i32, i32, i32) {
        let fibo_mtk = self.is_fibocom_mtk();
        if !fibo_mtk {
            let qeng = self
                .with_at_prio(Prio::Signal, |p| {
                    p.command("AT+QENG=\"servingcell\"", self.cmd_timeout)
                })
                .unwrap_or_default();
            if let Some(v) = parse_qeng_servingcell(&qeng) {
                return v;
            }
        }

        let cesq = self
            .with_at_prio(Prio::Signal, |p| p.command("AT+CESQ", self.cmd_timeout))
            .unwrap_or_default();
        let mut out = parse_cesq(&cesq).unwrap_or((0, 0, 0));

        if fibo_mtk && (out.0 == 0 || out.1 == 0 || out.2 == 0) {
            let gtcc = self
                .with_at_prio(Prio::Signal, |p| {
                    p.command("AT+GTCCINFO?", self.cmd_timeout)
                })
                .unwrap_or_default();
            if let Some(cell) = super::cells::parse_gtccinfo_rows(&gtcc)
                .into_iter()
                .find(|c| c.serving)
            {
                if out.0 == 0 {
                    out.0 = cell.rsrp;
                }
                if out.1 == 0 {
                    out.1 = cell.rsrq;
                }
                if out.2 == 0 {
                    out.2 = cell.sinr;
                }
            }
        }
        out
    }
}

/// 解析 `AT+CSQ` → `(rssi, ber)`。
///
/// `rssi` 是**索引不是 dBm**：0~31 对应 -113 ~ -51 dBm（`idx*2-113`），
/// `99` / `255` = 未上报；`ber` 0~7（`99` = 未上报）。换算留给展示层（前端与
/// `metrics.rs` 的 `valid_signal` 各自按 `idx*2-113` 算），这里保留原始索引。
pub fn parse_csq(resp: &str) -> (i32, i32) {
    let field = match extract_field(resp, "+CSQ") {
        Some(v) => v,
        None => return (99, 99),
    };
    let rssi = field_part(&field, 0)
        .and_then(|s| s.parse().ok())
        .unwrap_or(99);
    let ber = field_part(&field, 1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(99);
    (rssi, ber)
}

/// 解析 `AT+QENG="servingcell"` 的服务小区信号（NR 或 LTE）。
///
/// NR5G：`"servingcell","CONNECT","NR5G-SA","TDD",460,11,<cell>,702,<tac>,633984,78,100,<rsrp>,<rsrq>,<sinr>,…`
/// LTE ：`"servingcell","NOCONN","LTE",<tdd>,460,11,<cell>,<pcid>,<earfcn>,<band>,<ul>,<dl>,<tac>,<rsrp>,<rsrq>,<rssi>,<sinr>,…`
///
/// ⚠️ 与上游口径不同：Go 走的是量纲定位（`module/cell/cell.go:530`
/// `ParseServingCellSignal`，先整行解成 `CellInfo` 再取 RSRP/RSRQ/SINR），
/// 这里是移远的**固定列下标**（NR 12/13/14，LTE 13/14/16）。
/// 好处是对移远文档里的两种制式都准，代价是固件一旦挪列就整体取不到
/// （那时会退到 `AT+CESQ`，见 `Modem::query_serving_signal`）；
/// 不在本轮擅自改成量纲定位 —— 会改变所有机型的取值，见 A03 报告。
pub fn parse_qeng_servingcell(resp: &str) -> Option<(i32, i32, i32)> {
    let line = extract_all(resp, "+QENG")
        .into_iter()
        .find(|v| v.contains("servingcell"))?;
    let f = csv(&line);
    let tech = f.get(2)?.to_ascii_uppercase();
    if tech.contains("NR") {
        let rsrp = at_i32(f.get(12))?;
        let rsrq = at_i32(f.get(13)).unwrap_or(0);
        let sinr = at_i32(f.get(14)).unwrap_or(0);
        return valid(rsrp, rsrq, sinr);
    }
    if tech == "LTE" {
        let rsrp = at_i32(f.get(13))?;
        let rsrq = at_i32(f.get(14)).unwrap_or(0);
        let sinr = at_i32(f.get(16)).unwrap_or(0);
        return valid(rsrp, rsrq, sinr);
    }
    None
}

/// 解析 `AT+CESQ`（3GPP TS 27.007 §8.69，含广和通扩展的第 7~9 格 SSB 测量值）。
///
/// 依据 Go `module/cell/cell.go:573` `ParseCESQSignal`（同一份换算法）。
///
/// 字段序：`<rxlev>,<ber>,<rscp>,<ecno>,<rsrq>,<rsrp>,<ss_rsrq>,<ss_rsrp>,<ss_sinr>`。
/// 前 6 格是 3GPP 标准量，第 7~9 格是广和通的 NR SSB 扩展；制式由「哪几格
/// 不是 255」隐式给出：LTE 量有效走 LTE 表，否则走 NR 表。
/// `<rxlev>` / `<rscp>` / `<ecno>` 在 LTE/NR 模组上恒为 99 / 255，不参与换算；
/// 真正在 WCDMA 上要用它们（RSCP / EcIo）时，本函数已按「两项都为 0」提前退出。
///
/// 与上游的一处小差异：上游要求扩展格 `v > 0` 才换算，本实现允许 `v == 0`
/// （0 换算后落在量纲外，会被 `valid()` 滤成 0，结果一致）。
pub fn parse_cesq(resp: &str) -> Option<(i32, i32, i32)> {
    if !is_ok(resp) {
        return None;
    }
    let field = extract_field(resp, "+CESQ")?;
    let f = csv(&field);
    if f.len() < 6 {
        return None;
    }
    let mut rsrp = 0;
    let mut rsrq = 0;
    let mut sinr = 0;
    if let (Some(lte_rsrq), Some(lte_rsrp)) = (f[4].parse::<i32>().ok(), f[5].parse::<i32>().ok()) {
        rsrp = lte_rsrp_index_dbm(lte_rsrp).unwrap_or(0);
        rsrq = lte_rsrq_index_db(lte_rsrq).unwrap_or(0);
    }
    if rsrp == 0 && rsrq == 0 && f.len() >= 9 {
        if let Ok(v) = f[7].parse::<i32>() {
            rsrp = nr_ss_rsrp_index_dbm(v).unwrap_or(0);
        }
        if let Ok(v) = f[6].parse::<i32>() {
            rsrq = nr_ss_rsrq_index_db(v).unwrap_or(0);
        }
        if let Ok(v) = f[8].parse::<i32>() {
            sinr = nr_ss_sinr_index_db(v).unwrap_or(0);
        }
    }
    if rsrp == 0 && rsrq == 0 {
        return None;
    }
    valid(rsrp, rsrq, sinr)
}

/// LTE `<rsrp>` / `<rxlev>` 索引 → dBm。
///
/// 3GPP TS 27.007 §8.69 与《FM350 AT Commands》§11.1.15 同表：
/// `0` = 低于 -140 dBm，`1` = [-140,-139) dBm，步长 1 dB，`97` = ≥ -44 dBm。
/// 取区间下界，即索引 1 → -140 dBm（`idx - 141`）。
pub fn lte_rsrp_index_dbm(idx: i32) -> Option<i32> {
    (0..=97).contains(&idx).then(|| idx - 141)
}

/// LTE `<rsrq>` 索引 → dB：`0` = 低于 -19.5 dB，`1` = [-19.5,-19) dB，
/// 步长 0.5 dB，`34` = ≥ -3 dB。
///
/// `SignalInfo` / `CellInfo` 的信号格都是整数字段，半步值向零截断
/// （索引 9 → -15.5 dB → -15）。
pub fn lte_rsrq_index_db(idx: i32) -> Option<i32> {
    (0..=34).contains(&idx).then(|| (idx - 40) / 2)
}

/// NR `<ss_rsrp>` 索引 → dBm：`0` = 低于 -156 dBm，`1` = [-156,-155) dBm，
/// 步长 1 dB，`126` = ≥ -31 dBm。
pub fn nr_ss_rsrp_index_dbm(idx: i32) -> Option<i32> {
    (0..=126).contains(&idx).then(|| idx - 157)
}

/// NR `<ss_rsrq>` 索引 → dB：`0` = 低于 -43 dB，`1` = [-43,-42.5) dB，
/// 步长 0.5 dB，`126` = [19.5,20) dB。
pub fn nr_ss_rsrq_index_db(idx: i32) -> Option<i32> {
    (0..=126)
        .contains(&idx)
        .then(|| (idx as f64 * 0.5 - 43.5).round() as i32)
}

/// NR `<ss_sinr>` 索引 → dB（只有 `AT+CESQ` 的广和通扩展格是索引；
/// `AT+GTCCINFO?` 的 `<rssnr_value>` 本身就是 dB，不走这张表）。
pub fn nr_ss_sinr_index_db(idx: i32) -> Option<i32> {
    (0..=127)
        .contains(&idx)
        .then(|| (idx as f64 * 0.5 - 23.5).round() as i32)
}

fn at_i32(v: Option<&String>) -> Option<i32> {
    v.and_then(|s| s.parse::<i32>().ok())
}

/// 过滤物理范围外的哨兵值（区间与前端 `rust/web/index.html` 的 `sampleFromPush`
/// 一致，否则曲线上的点会和面板上的数字对不上）。
///
/// 三个值中 `rsrp` 是「有没有信号」的判据：它为 0 就整份丢弃（`None`），
/// 让调用方继续退到下一路查询；RSRQ / SINR 取不到时只把该项置 0，不牵连 RSRP。
fn valid(rsrp: i32, rsrq: i32, sinr: i32) -> Option<(i32, i32, i32)> {
    let rsrp = if (-156..=-31).contains(&rsrp) {
        rsrp
    } else {
        0
    };
    let rsrq = if (-43..=20).contains(&rsrq) && rsrq != 0 {
        rsrq
    } else {
        0
    };
    let sinr = if (-23..=40).contains(&sinr) && sinr != 0 {
        sinr
    } else {
        0
    };
    if rsrp == 0 {
        return None;
    }
    Some((rsrp, rsrq, sinr))
}

#[cfg(test)]
mod tests {
    use super::{
        lte_rsrp_index_dbm, lte_rsrq_index_db, nr_ss_rsrp_index_dbm, nr_ss_rsrq_index_db,
        nr_ss_sinr_index_db, parse_cesq, parse_csq, parse_qeng_servingcell,
    };

    #[test]
    fn csq() {
        assert_eq!(parse_csq("\r\n+CSQ: 22,99\r\n\r\nOK\r\n"), (22, 99));
        assert_eq!(parse_csq("+CSQ: 255,99\r\nOK"), (255, 99));
    }

    #[test]
    fn qeng_nr_servingcell() {
        let resp = "+QENG: \"servingcell\",\"CONNECT\",\"NR5G-SA\",\"TDD\",460,11,760BF0114,702,76010F,633984,78,100,-89,-8,15,18,34,1\r\n\r\nOK\r\n";
        assert_eq!(parse_qeng_servingcell(resp), Some((-89, -8, 15)));
    }

    #[test]
    fn qeng_lte_servingcell() {
        let resp = "+QENG: \"servingcell\",\"NOCONN\",\"LTE\",\"FDD\",460,11,1A2B3C4,301,1850,3,20,20,1A2B,-95,-11,-63,12,50,40\r\nOK\r\n";
        assert_eq!(parse_qeng_servingcell(resp), Some((-95, -11, 12)));
    }

    #[test]
    fn cesq_lte() {
        let resp = "+CESQ: 99,99,255,255,16,25,255,255,255\r\nOK\r\n";
        assert_eq!(parse_cesq(resp), Some((-116, -12, 0)));
    }

    #[test]
    fn cesq_lte_real_device_sample() {
        // FM350-GL 实机抓包（LTE 制式）：<rsrq> 索引 11、<rsrp> 索引 22
        let resp = "+CESQ: 12,99,255,255,11,22,255,255,255\r\nOK\r\n";
        assert_eq!(parse_cesq(resp), Some((-119, -14, 0)));
    }

    #[test]
    fn cesq_fibocom_ssb() {
        let resp = "+CESQ: 99,99,255,255,255,255,30,60,21\r\nOK\r\n";
        assert_eq!(parse_cesq(resp), Some((-97, -29, -13)));
    }

    #[test]
    fn index_helpers_take_3gpp_lower_bound() {
        // 3GPP TS 27.007 §8.69 / FM350 手册 §11.1.15：索引 1 取区间下界
        assert_eq!(lte_rsrp_index_dbm(0), Some(-141)); // 低于 -140
        assert_eq!(lte_rsrp_index_dbm(1), Some(-140));
        assert_eq!(lte_rsrp_index_dbm(97), Some(-44));
        assert_eq!(lte_rsrp_index_dbm(98), None);
        assert_eq!(lte_rsrq_index_db(0), Some(-20)); // 低于 -19.5
        assert_eq!(lte_rsrq_index_db(9), Some(-15)); // -15.5 向零截断
        assert_eq!(lte_rsrq_index_db(34), Some(-3));
        assert_eq!(lte_rsrq_index_db(35), None);
        assert_eq!(nr_ss_rsrp_index_dbm(1), Some(-156));
        assert_eq!(nr_ss_rsrp_index_dbm(70), Some(-87));
        assert_eq!(nr_ss_rsrq_index_db(65), Some(-11));
        assert_eq!(nr_ss_sinr_index_db(77), Some(15));
        assert_eq!(nr_ss_sinr_index_db(128), None);
    }
}
