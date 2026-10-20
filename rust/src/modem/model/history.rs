//! 信号采样历史（给界面画曲线用）。
//!
//! 与「波动记录」（[`super::changes`]）的分工：波动记录只存**变化**，适合当时间线读；
//! 这里存**连续采样**，适合看趋势。两者共用同一个挂钩点（`modem/metrics.rs` 的
//! `note_signal`），一次查询喂两个用途，**不额外发 AT**。
//!
//! 本模块是纯存储：只知道模组编号与采样值，不认识 [`super::Modem`]。
//!
//! 存储上的关键技巧：**值没变时只把最后一个点的时间往后推，不新增点**。
//! 这样稳定信号不会把缓冲塞满，曲线上依然是一条一直画到「现在」的平线
//! （若改成「没变就不记」，曲线会在最后一次变化处断掉，看起来像数据过期）。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

use crate::logging::rfc3339;

/// 每个模组保留的采样点上限。
///
/// 存的是**采样点**不是「变化次数」：一次 `note_signal` 最多产生一个点，
/// 值没变时连点都不新增（只把末点时间往后推），所以实际能覆盖多长一段时间
/// 取决于查询频率（HTTP 查询 / 推送 / 任务刷新都会喂，没有固定周期），
/// 这里只保证「不会无限长」。
pub const SAMPLE_MAX: usize = 1800;

/// 一次信号采样。空值用 `None` 表示「该次未上报」，让前端把曲线断开，
/// 而不是把「未上报」当成 0 画出来。
#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    #[serde(skip_serializing_if = "is_zero")]
    pub modem_id: u32,
    /// RFC3339（UTC）。
    pub at: String,
    /// dBm。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsrp: Option<i32>,
    /// dB。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsrq: Option<i32>,
    /// dB。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sinr: Option<i32>,
    /// dBm（由 CSQ 换算）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi: Option<i32>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// 内部采样点（时间用 Unix 秒，便于比较与过滤）。
#[derive(Debug, Clone)]
struct Point {
    modem_id: u32,
    at_secs: i64,
    rsrp: Option<i32>,
    rsrq: Option<i32>,
    sinr: Option<i32>,
    rssi: Option<i32>,
}

impl Point {
    /// 两次采样的值是否相同（时间不算）。
    fn same_value_as(&self, other: &Point) -> bool {
        self.rsrp == other.rsrp
            && self.rsrq == other.rsrq
            && self.sinr == other.sinr
            && self.rssi == other.rssi
    }

    fn to_sample(&self) -> Sample {
        Sample {
            modem_id: self.modem_id,
            at: rfc3339(self.at_secs),
            rsrp: self.rsrp,
            rsrq: self.rsrq,
            sinr: self.sinr,
            rssi: self.rssi,
        }
    }
}

/// 采样历史（按模组分组）。
#[derive(Default)]
pub struct HistoryStore {
    inner: Mutex<HashMap<u32, Vec<Point>>>,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次采样。与上一点的值相同则只把它的时间推到 `at_secs`。
    pub fn add(
        &self,
        modem_id: u32,
        at_secs: i64,
        rsrp: Option<i32>,
        rsrq: Option<i32>,
        sinr: Option<i32>,
        rssi: Option<i32>,
    ) {
        let point = Point {
            modem_id,
            at_secs,
            rsrp,
            rsrq,
            sinr,
            rssi,
        };
        let mut inner = self.inner.lock().unwrap();
        let list = inner.entry(modem_id).or_default();
        if let Some(last) = list.last_mut() {
            if last.same_value_as(&point) {
                if at_secs >= last.at_secs {
                    last.at_secs = at_secs; // 平线一直画到「现在」
                }
                return;
            }
        }
        list.push(point);
        if list.len() > SAMPLE_MAX {
            let drop = list.len() - SAMPLE_MAX;
            list.drain(..drop); // 丢掉最老的
        }
    }

    /// 取采样点（时间升序）。
    ///
    /// `modem_id < 0` 表示所有模组（按时间归并）；`since_secs <= 0` 表示不限起始时间；
    /// `limit == 0` 表示不限条数（只保留最新的若干条）。
    pub fn snapshot(&self, modem_id: i64, since_secs: i64, limit: usize) -> Vec<Sample> {
        let inner = self.inner.lock().unwrap();
        let mut points: Vec<&Point> = if modem_id >= 0 {
            inner
                .get(&(modem_id as u32))
                .map(|list| list.iter().collect())
                .unwrap_or_default()
        } else {
            inner.values().flatten().collect()
        };
        if since_secs > 0 {
            points.retain(|p| p.at_secs >= since_secs);
        }
        if modem_id < 0 {
            points.sort_by_key(|p| p.at_secs);
        }
        if limit > 0 && points.len() > limit {
            let drop = points.len() - limit;
            points.drain(..drop);
        }
        points.iter().map(|p| p.to_sample()).collect()
    }

    /// 某模组的采样段数（`modem_id < 0` 表示全部）。
    pub fn count_for(&self, modem_id: i64) -> usize {
        let inner = self.inner.lock().unwrap();
        if modem_id < 0 {
            return inner.values().map(Vec::len).sum();
        }
        inner.get(&(modem_id as u32)).map(Vec::len).unwrap_or(0)
    }

    /// 清空采样历史（`modem_id < 0` 表示全部），返回清掉的段数。
    pub fn clear(&self, modem_id: i64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        if modem_id < 0 {
            let n = inner.values().map(Vec::len).sum();
            inner.clear();
            return n;
        }
        inner
            .remove(&(modem_id as u32))
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryStore, SAMPLE_MAX};

    #[test]
    fn same_value_moves_the_last_point_instead_of_appending() {
        let h = HistoryStore::new();
        h.add(0, 100, Some(-95), Some(-11), Some(12), Some(-63));
        h.add(0, 101, Some(-95), Some(-11), Some(12), Some(-63));
        assert_eq!(h.count_for(0), 1);
        let s = h.snapshot(0, 0, 0);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].at, crate::logging::rfc3339(101));
    }

    #[test]
    fn changed_value_appends() {
        let h = HistoryStore::new();
        h.add(0, 100, Some(-95), None, None, None);
        h.add(0, 101, Some(-90), None, None, None);
        assert_eq!(h.count_for(0), 2);
    }

    #[test]
    fn missing_values_serialize_without_keys() {
        let h = HistoryStore::new();
        h.add(1, 100, Some(-95), None, None, None);
        let json = serde_json::to_string(&h.snapshot(1, 0, 0)).unwrap();
        assert!(json.contains("\"modem_id\":1"));
        assert!(json.contains("\"rsrp\":-95"));
        assert!(!json.contains("rsrq"));
    }

    #[test]
    fn ring_buffer_keeps_newest() {
        let h = HistoryStore::new();
        for i in 0..(SAMPLE_MAX + 5) {
            // 每次值都不同，避免被「合并最后一点」压掉
            h.add(0, 1000 + i as i64, Some(-100 + i as i32), None, None, None);
        }
        assert_eq!(h.count_for(0), SAMPLE_MAX);
        let s = h.snapshot(0, 0, 0);
        assert_eq!(s[0].rsrp, Some(-100 + 5));
    }

    #[test]
    fn since_and_limit_filters() {
        let h = HistoryStore::new();
        for i in 0..5 {
            h.add(0, 100 + i, Some(-100 + i as i32), None, None, None);
        }
        assert_eq!(h.snapshot(0, 103, 0).len(), 2);
        assert_eq!(h.snapshot(0, 0, 2).len(), 2);
        // 只保留最新的两条
        let s = h.snapshot(0, 0, 2);
        assert_eq!(s[0].rsrp, Some(-97));
    }

    #[test]
    fn all_modems_are_merged_in_time_order() {
        let h = HistoryStore::new();
        h.add(1, 300, Some(-90), None, None, None);
        h.add(0, 100, Some(-95), None, None, None);
        let s = h.snapshot(-1, 0, 0);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].modem_id, 0);
        assert_eq!(s[1].modem_id, 1);
        assert_eq!(h.count_for(-1), 2);
    }

    #[test]
    fn clear_only_targets_one_modem() {
        let h = HistoryStore::new();
        h.add(0, 100, Some(-95), None, None, None);
        h.add(1, 100, Some(-90), None, None, None);
        assert_eq!(h.clear(0), 1);
        assert_eq!(h.count_for(0), 0);
        assert_eq!(h.count_for(1), 1);
        assert_eq!(h.clear(-1), 1);
    }
}
