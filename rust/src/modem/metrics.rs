//! 指标挂钩：把一次查询的结果喂给「采样历史」与「波动记录」。
//!
//! 分工：`model/changes.rs` 与 `model/history.rs` 是纯存储；本模块负责
//! 「从查询结果里取出哪些字段」以及「哪些值算有效」——这两件事要用到
//! 领域类型（[`SignalInfo`] / [`NetworkStatus`] / [`CellScanResult`]）。
//!
//! 设计原则：**不额外发 AT**。记录点挂在已有的查询上（HTTP 接口 / 基站扫描），
//! 拿到数据顺手比对一次，零额外串口开销。

use super::model::{
    changes::{self, ChangeEntry},
    history::HistoryStore,
    CellInfo, CellScanResult, NetworkStatus, SignalInfo,
};

/// 信号量物理范围（模组偶尔回无效哨兵值，超出范围一律当「未上报」）。
const RSRP_LO: i32 = -156;
const RSRP_HI: i32 = -44;
const RSRQ_LO: i32 = -43;
const RSRQ_HI: i32 = 20;
const SINR_LO: i32 = -23;
const SINR_HI: i32 = 40;

/// 波动记录 + 采样历史（进程内唯一，重启即清空）。
#[derive(Default)]
pub struct Metrics {
    pub changes: changes::Tracker,
    pub samples: HistoryStore,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次信号观测：存入采样历史 + 比对出波动记录。
    ///
    /// 两者用同一次查询的数据，不额外发 AT。
    pub fn note_signal(&self, modem_id: u32, s: &SignalInfo) -> Vec<ChangeEntry> {
        let now = crate::logging::unix_now();
        let (rsrp, rsrq, sinr, rssi) = valid_signal(s);
        self.samples.add(modem_id, now, rsrp, rsrq, sinr, rssi);
        self.signal_changes(modem_id, s)
    }

    /// 记录一次网络状态观测。
    pub fn note_network(&self, modem_id: u32, st: &NetworkStatus) -> Vec<ChangeEntry> {
        self.network_changes(modem_id, st)
    }

    /// 记录一次基站扫描结果（服务小区 / 邻区数量）。
    pub fn note_cells(&self, modem_id: u32, res: &CellScanResult) -> Vec<ChangeEntry> {
        self.cell_scan_changes(modem_id, res)
    }

    /// 信号量的波动记录。
    fn signal_changes(&self, modem_id: u32, s: &SignalInfo) -> Vec<ChangeEntry> {
        let (rsrp, rsrq, sinr, rssi) = valid_signal(s);
        let mut out = Vec::new();
        if let Some(v) = rsrp {
            push(
                &mut out,
                self.changes.observe_numeric(
                    modem_id,
                    changes::GROUP_SIG,
                    "rsrp",
                    "信号强度 RSRP",
                    v,
                    " dBm",
                    changes::SIG_THRESHOLD,
                    Some(changes::rsrp_grade),
                ),
            );
        }
        if let Some(v) = rsrq {
            push(
                &mut out,
                self.changes.observe_numeric(
                    modem_id,
                    changes::GROUP_SIG,
                    "rsrq",
                    "信号质量 RSRQ",
                    v,
                    " dB",
                    changes::SIG_THRESHOLD,
                    Some(changes::rsrq_grade),
                ),
            );
        }
        if let Some(v) = sinr {
            push(
                &mut out,
                self.changes.observe_numeric(
                    modem_id,
                    changes::GROUP_SIG,
                    "sinr",
                    "信号质量 SINR",
                    v,
                    " dB",
                    changes::SIG_THRESHOLD,
                    Some(changes::sinr_grade),
                ),
            );
        }
        if let Some(v) = rssi {
            push(
                &mut out,
                self.changes.observe_numeric(
                    modem_id,
                    changes::GROUP_SIG,
                    "rssi",
                    "RSSI",
                    v,
                    " dBm",
                    changes::RSSI_THRESHOLD,
                    None,
                ),
            );
        }
        out
    }

    /// 网络状态的波动记录。
    ///
    /// 服务小区 / 邻区数量来自**扫描缓存**，由 [`Self::note_cells`] 负责，
    /// 这里不重复记：缓存为空时会把「未上报/0」当成基线，第一次扫描就冒出
    /// 一条没有意义的「变化」。
    fn network_changes(&self, modem_id: u32, st: &NetworkStatus) -> Vec<ChangeEntry> {
        let mut out = Vec::new();
        fn opt(v: &Option<String>) -> &str {
            v.as_deref().unwrap_or("")
        }

        self.observe_if(&mut out, modem_id, "tech", "网络类型", opt(&st.net_tech));
        match (opt(&st.band_label), opt(&st.band_text)) {
            (label, text) if !label.is_empty() && !text.is_empty() => {
                self.observe(
                    &mut out,
                    modem_id,
                    "band",
                    "频段",
                    &format!("{label}（{text}）"),
                );
            }
            (label, _) if !label.is_empty() => {
                self.observe(&mut out, modem_id, "band", "频段", label);
            }
            _ => self.observe_if(&mut out, modem_id, "band", "频段", opt(&st.band_text)),
        }
        if st.earfcn > 0 {
            self.observe(&mut out, modem_id, "earfcn", "频点", &st.earfcn.to_string());
        }
        self.observe_if(&mut out, modem_id, "plmn", "PLMN", opt(&st.plmn));
        self.observe_if(&mut out, modem_id, "operator", "运营商", opt(&st.operator));
        self.observe_if(&mut out, modem_id, "cs_reg", "CS 域", &st.cs_reg.text);
        self.observe_if(&mut out, modem_id, "ps_reg", "数据域", &st.ps_reg.text);
        // APN / IP 用「未上报」归一化：地址消失（掉线）也是一次重要变化
        self.observe(
            &mut out,
            modem_id,
            "apn",
            "APN",
            &changes::not_reported(opt(&st.apn)),
        );
        self.observe(
            &mut out,
            modem_id,
            "ipv4",
            "IPv4",
            &changes::not_reported(opt(&st.ipv4)),
        );
        self.observe(
            &mut out,
            modem_id,
            "ipv6",
            "IPv6",
            &changes::not_reported(opt(&st.ipv6)),
        );
        out
    }

    /// 记一条网络字段（空值也参与）。
    fn observe(
        &self,
        out: &mut Vec<ChangeEntry>,
        modem_id: u32,
        field: &str,
        label: &str,
        value: &str,
    ) {
        push(
            out,
            self.changes
                .observe(modem_id, changes::GROUP_NET, field, label, value),
        );
    }

    /// 记一条网络字段，值为空时跳过（空值通常代表「没上报」而非状态变化）。
    fn observe_if(
        &self,
        out: &mut Vec<ChangeEntry>,
        modem_id: u32,
        field: &str,
        label: &str,
        value: &str,
    ) {
        if !value.trim().is_empty() {
            self.observe(out, modem_id, field, label, value);
        }
    }

    /// 基站扫描完成后的波动记录：服务小区 / 邻区数量。
    fn cell_scan_changes(&self, modem_id: u32, res: &CellScanResult) -> Vec<ChangeEntry> {
        let mut out = Vec::new();
        if let Some(summary) = res.serving.first().map(cell_summary) {
            push(
                &mut out,
                self.changes.observe(
                    modem_id,
                    changes::GROUP_NET,
                    "serving",
                    "服务小区",
                    &summary,
                ),
            );
        }
        push(
            &mut out,
            self.changes.observe(
                modem_id,
                changes::GROUP_NET,
                "neighbour",
                "邻区数量",
                &res.neighbour_count.to_string(),
            ),
        );
        out
    }
}

fn push(out: &mut Vec<ChangeEntry>, entry: Option<ChangeEntry>) {
    if let Some(e) = entry {
        out.push(e);
    }
}

/// 判定哪些信号值有效，并换算 RSSI（`validSignal`）。
///
/// `0 / 99 / 255` 是 3GPP 的「未上报」；CSQ 255 是扩展 URC 的哨兵值，
/// 采样时也会被挡掉。
fn valid_signal(s: &SignalInfo) -> (Option<i32>, Option<i32>, Option<i32>, Option<i32>) {
    let take = |v: i32, lo: i32, hi: i32| {
        if changes::int_in_range(v, lo, hi) {
            Some(v)
        } else {
            None
        }
    };
    let rsrp = take(s.nr_rsrp, RSRP_LO, RSRP_HI);
    let rsrq = if s.nr_rsrq != 0 {
        take(s.nr_rsrq, RSRQ_LO, RSRQ_HI)
    } else {
        None
    };
    let sinr = take(s.nr_sinr, SINR_LO, SINR_HI);
    let rssi = if s.rssi != 0 && s.rssi != 99 && s.rssi != 255 {
        Some(s.rssi * 2 - 113)
    } else {
        None
    };
    (rsrp, rsrq, sinr, rssi)
}

/// 服务小区一行摘要：PCI / 频段 / 频点 / Cell（有的就带上）。
fn cell_summary(c: &CellInfo) -> String {
    let mut parts = Vec::new();
    if c.pcid > 0 {
        parts.push(format!("PCI {}", c.pcid));
    }
    if c.band > 0 {
        // NR 用 n78，LTE/WCDMA 用 B3
        let label = if c.tech.to_ascii_uppercase().contains("NR") {
            format!("n{}", c.band)
        } else {
            format!("B{}", c.band)
        };
        parts.push(label);
    }
    if c.earfcn > 0 {
        parts.push(format!("频点 {}", c.earfcn));
    }
    if !c.cell_id.is_empty() {
        parts.push(format!("Cell {}", c.cell_id));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::{cell_summary, valid_signal, Metrics};
    use crate::modem::model::{CellInfo, CellScanResult, NetworkStatus, RegState, SignalInfo};

    fn sig(rssi: i32, rsrp: i32, rsrq: i32, sinr: i32) -> SignalInfo {
        SignalInfo {
            rssi,
            ber: 99,
            nr_rsrp: rsrp,
            nr_rsrq: rsrq,
            nr_sinr: sinr,
        }
    }

    #[test]
    fn sentinel_values_are_dropped() {
        // CSQ 99/255 与范围外的 NR 值都当「未上报」；SINR=0 在物理范围内，保留
        assert_eq!(valid_signal(&sig(99, 0, 0, 0)), (None, None, Some(0), None));
        assert_eq!(
            valid_signal(&sig(255, 0, 0, 0)),
            (None, None, Some(0), None)
        );
        assert_eq!(
            valid_signal(&sig(20, -1200, 0, 600)),
            (None, None, None, Some(-73))
        );
    }

    #[test]
    fn rssi_is_converted_from_csq_index() {
        assert_eq!(valid_signal(&sig(22, 0, 0, 0)).3, Some(-69));
        // RSRQ=0 表示未上报，不能当成有效值
        assert_eq!(valid_signal(&sig(0, -95, 0, 12)).1, None);
        assert_eq!(
            valid_signal(&sig(0, -95, -11, 12)),
            (Some(-95), Some(-11), Some(12), None)
        );
    }

    #[test]
    fn note_signal_records_sample_and_changes() {
        let m = Metrics::new();
        assert!(m.note_signal(0, &sig(22, -95, -11, 12)).is_empty());
        // 采样入库（一次）
        let samples = m.samples.snapshot(0, 0, 0);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].rsrp, Some(-95));
        // 变化超过阈值才记
        assert!(m.note_signal(0, &sig(22, -96, -11, 12)).is_empty());
        let entries = m.note_signal(0, &sig(22, -104, -11, 12));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].field, "rsrp");
        assert!(entries[0].delta.starts_with('-'));
        assert!(!entries[0].up);
    }

    #[test]
    fn cell_summary_layout() {
        let c = CellInfo {
            tech: "NR".into(),
            pcid: 702,
            band: 78,
            earfcn: 633984,
            cell_id: "760BF0114".into(),
            ..Default::default()
        };
        assert_eq!(
            cell_summary(&c),
            "PCI 702 · n78 · 频点 633984 · Cell 760BF0114"
        );
    }

    #[test]
    fn note_cells_records_serving_and_neighbour() {
        let m = Metrics::new();
        let mut res = CellScanResult::default();
        res.serving.push(CellInfo {
            tech: "LTE".into(),
            pcid: 301,
            band: 3,
            earfcn: 1850,
            ..Default::default()
        });
        res.neighbour_count = 6;
        assert!(m.note_cells(0, &res).is_empty()); // 首次只建基线
        res.neighbour_count = 5;
        let entries = m.note_cells(0, &res);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].field, "neighbour");
        assert_eq!(entries[0].from, "6");
        assert_eq!(entries[0].to, "5");
    }

    #[test]
    fn note_network_marks_unreported_ip_as_change() {
        let m = Metrics::new();
        let st = NetworkStatus {
            modem_id: 0,
            queried_at: String::new(),
            net_tech: Some("TDD NR5G".into()),
            tech_label: None,
            tech: None,
            plmn: Some("46011".into()),
            mcc: None,
            mnc: None,
            band_text: Some("NR5G BAND 78".into()),
            band: 78,
            band_label: Some("n78".into()),
            earfcn: 627264,
            operator: None,
            cs_reg: RegState::default(),
            ps_reg: RegState::default(),
            apn: Some("internet".into()),
            ipv4: Some("10.0.0.2".into()),
            ipv6: None,
            roaming: false,
            traffic: None,
            ..Default::default()
        };
        assert!(m.note_network(0, &st).is_empty()); // 首次建基线
        let mut next = NetworkStatus { ..st.clone() };
        next.ipv4 = None;
        let entries = m.note_network(0, &next);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].field, "ipv4");
        assert_eq!(entries[0].to, "未上报");
    }
}
