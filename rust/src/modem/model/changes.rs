//! 波动记录（网络 / 信号变化时间线）。
//!
//! 把每次查询的结果与上一次比对，记成一条时间线：时间 · 项目 · 之前 → 之后 ·
//! 变化量 · 旧值保持了多久。本模块只做**存储 + 比对 + 阈值/节流 + 品级文案**，
//! 是纯逻辑（不认识 [`super::Modem`]），转换与挂钩见 `modem/metrics.rs`。
//!
//! 三条设计原则：
//!  1. **不额外发 AT**：记录点挂在已有查询上（HTTP 接口 / 推送循环 / 基站扫描）；
//!  2. **阈值**：数值型信号量只有变化超过阈值、或跨了等级才记，避免抖动刷屏；
//!  3. **只在内存里环形缓冲**（[`GLOBAL_MAX`] 条，不落盘），重启即清空。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

use crate::logging::{rfc3339, unix_now};

/// 分组名（前端按这个分栏）。
pub const GROUP_NET: &str = "网络";
/// 信号分组名。
pub const GROUP_SIG: &str = "信号";

/// 环形缓冲上限（每条约 200 字节，500 条足够回顾最近的情况）。
pub const GLOBAL_MAX: usize = 500;

/// 同一字段两条记录之间的最小间隔（秒）。
///
/// 信号在几秒内抖 5 dB 是常态，没有这个节流会「一秒一条」把时间线刷满；
/// 不同字段各自计时，所以换频段/换小区这类事件不会被拖慢。
pub const MIN_INTERVAL_SECS: i64 = 30;

/// RSRP/RSRQ/SINR 的记录阈值：差了这么多才值得记一条。
pub const SIG_THRESHOLD: i32 = 3;
/// RSSI（CSQ 换算，步进 2 dB）的记录阈值。
pub const RSSI_THRESHOLD: i32 = 5;

fn is_false(v: &bool) -> bool {
    !*v
}

/// 一条波动记录。
#[derive(Debug, Clone, Serialize)]
pub struct ChangeEntry {
    /// 变化发生时间。
    pub at: String,
    pub modem_id: u32,
    pub group: String,
    /// 机器可读字段名（前端筛选用）。
    pub field: String,
    /// 展示名（频点 / 频段 / 信号强度 RSRP…）。
    pub label: String,
    /// 变化前的值。
    pub from: String,
    /// 变化后的值。
    pub to: String,
    /// 数值型的变化量（`+4 dBm`）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub delta: String,
    /// 数值是否朝「更好」的方向变（正的 dB 就是变好）。
    #[serde(skip_serializing_if = "is_false")]
    pub up: bool,
    /// 旧值维持了多久（距上一条记录；首条以开始观测为基准）。
    pub held_s: i64,
}

/// 某字段的当前值（对应 `GET /api/changes` 的 `current` 列表）。
#[derive(Debug, Clone, Serialize)]
pub struct ChangeState {
    pub group: String,
    pub field: String,
    pub label: String,
    pub value: String,
    /// 该值是从什么时候开始的。
    pub since: String,
    /// 已保持了多久（秒）。
    pub held_s: i64,
}

/// 内部状态：除展示用的值，还留着数值与等级用于阈值判断。
///
/// 要区分两组值：`value/num/grade/since` 是最近一次**采样**（前端的「当前值」
/// 面板用它）；`rec_*` 是最近一条**记录**的内容。真实信号几秒内就能来回抖 5 dB，
/// 若拿「上一条记录」与「当前采样」直接比，1 秒采样就会一秒记一条 ——
/// 所以阈值与 `from` 都以 `rec_*` 为基准，被压掉的采样只更新 `value`。
#[derive(Debug, Clone)]
struct FieldState {
    modem_id: u32,
    group: String,
    field: String,
    label: String,
    value: String,
    num: i32,
    grade: String,
    since: i64,
    /// 首次观测时间（第一条记录的「保持时长」以它为基准）。
    base_at: i64,
    rec_value: String,
    rec_num: i32,
    rec_grade: String,
    /// 0 表示还没记过（此时不做节流）。
    rec_at: i64,
}

#[derive(Default)]
struct Inner {
    /// 时间升序，最新在末尾。
    entries: Vec<ChangeEntry>,
    states: HashMap<String, FieldState>,
}

/// 波动记录器（并发安全）。
#[derive(Default)]
pub struct Tracker {
    inner: Mutex<Inner>,
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 观测一个非数值字段；与上一条记录不同时记一条。
    ///
    /// 首次观测只建立基线（没有「之前」，不算变化）；空值一律不参与
    /// —— 空值通常代表「没上报/查询失败」，不是真的状态变化。
    pub fn observe(
        &self,
        modem_id: u32,
        group: &str,
        field: &str,
        label: &str,
        value: &str,
    ) -> Option<ChangeEntry> {
        if value.is_empty() {
            return None;
        }
        self.observe_value(modem_id, group, field, label, value, 0, "", "", 0)
    }

    /// 观测一个数值型字段（信号量）。
    ///
    /// `threshold` 是「值得记一条」的最小变化量；`grade` 为 `None` 时只用阈值判断。
    #[allow(clippy::too_many_arguments)] // 观测接口要带阈值/分级，拆结构体反而更难读
    pub fn observe_numeric(
        &self,
        modem_id: u32,
        group: &str,
        field: &str,
        label: &str,
        num: i32,
        unit: &str,
        threshold: i32,
        grade: Option<fn(i32) -> &'static str>,
    ) -> Option<ChangeEntry> {
        let mut value = format!("{num}{unit}");
        let g = grade.map(|f| f(num)).unwrap_or("");
        if !g.is_empty() {
            value.push('（');
            value.push_str(g);
            value.push('）');
        }
        self.observe_value(
            modem_id, group, field, label, &value, num, unit, g, threshold,
        )
    }

    /// 记录器的统一入口：先更新「当前采样值」，再判断是否值得记一条。
    ///
    /// 两道闸门防止刷屏：`threshold`（跨等级一定记）与 [`MIN_INTERVAL_SECS`] 节流。
    #[allow(clippy::too_many_arguments)]
    pub fn observe_value(
        &self,
        modem_id: u32,
        group: &str,
        field: &str,
        label: &str,
        value: &str,
        num: i32,
        unit: &str,
        grade: &str,
        threshold: i32,
    ) -> Option<ChangeEntry> {
        let key = change_key(modem_id, group, field);
        let now = unix_now();
        let mut inner = self.inner.lock().unwrap();

        // 首次观测：建立基线（它既是「当前值」，也是「上一条记录」的基准）。
        // 用 Entry 一次查找拿到 &mut，避免 contains_key + insert 两次哈希。
        let st = match inner.states.entry(key) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(FieldState {
                    modem_id,
                    group: group.to_string(),
                    field: field.to_string(),
                    label: label.to_string(),
                    value: value.to_string(),
                    num,
                    grade: grade.to_string(),
                    since: now,
                    base_at: now,
                    rec_value: value.to_string(),
                    rec_num: num,
                    rec_grade: grade.to_string(),
                    rec_at: 0,
                });
                return None;
            }
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
        };

        if st.value == value && st.grade == grade {
            return None; // 采样值没变
        }
        // 采样值变了：先更新「当前值」（即使这轮不记录，前端也该看到最新值）
        st.value = value.to_string();
        st.num = num;
        st.grade = grade.to_string();
        st.since = now;

        if st.rec_value == value {
            return None; // 又抖回上一条记录的值，不算新事件
        }
        let grade_changed = st.rec_grade != grade;
        let diff = (num - st.rec_num).abs();
        if !grade_changed && threshold > 0 && diff < threshold {
            return None; // 阈值内的小抖动
        }
        if st.rec_at != 0 && now - st.rec_at < MIN_INTERVAL_SECS {
            return None; // 节流：同一字段记太密
        }
        let held_from = if st.rec_at != 0 {
            st.rec_at
        } else {
            st.base_at
        };
        // 只有数值字段（带单位）才给出变化量与方向：文本字段的 num 恒为 0，
        // 直接相减会得到「▼ 0」这种假变化量。
        let (delta, up) = if unit.is_empty() {
            (String::new(), false)
        } else {
            let sign = if num > st.rec_num { "+" } else { "" };
            (
                format!("{sign}{}{unit}", num - st.rec_num),
                num > st.rec_num,
            )
        };
        let entry = ChangeEntry {
            at: rfc3339(now),
            modem_id,
            group: group.to_string(),
            field: field.to_string(),
            label: label.to_string(),
            from: st.rec_value.clone(),
            to: value.to_string(),
            delta,
            up,
            held_s: now - held_from,
        };
        st.rec_value = value.to_string();
        st.rec_num = num;
        st.rec_grade = grade.to_string();
        st.rec_at = now;
        inner.entries.push(entry.clone());
        if inner.entries.len() > GLOBAL_MAX {
            let drop = inner.entries.len() - GLOBAL_MAX;
            inner.entries.drain(..drop);
        }
        Some(entry)
    }

    /// 取记录（最新在前）。`modem_id < 0` 表示不限模组；`limit <= 0` 表示不限条数。
    pub fn snapshot(&self, modem_id: i64, limit: usize) -> Vec<ChangeEntry> {
        let inner = self.inner.lock().unwrap();
        let mut out = Vec::new();
        for e in inner.entries.iter().rev() {
            if modem_id >= 0 && e.modem_id as i64 != modem_id {
                continue;
            }
            out.push(e.clone());
            if limit > 0 && out.len() >= limit {
                break;
            }
        }
        out
    }

    /// 取各字段的当前值（按分组 + 展示名排序，便于前端成组展示）。
    pub fn current(&self, modem_id: i64) -> Vec<ChangeState> {
        let now = unix_now();
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<ChangeState> = inner
            .states
            .values()
            .filter(|st| modem_id < 0 || st.modem_id as i64 == modem_id)
            .map(|st| ChangeState {
                group: st.group.clone(),
                field: st.field.clone(),
                label: st.label.clone(),
                value: st.value.clone(),
                since: rfc3339(st.since),
                held_s: now - st.since,
            })
            .collect();
        // 固定顺序：先组后展示名，前端表格不会随哈希迭代顺序跳来跳去
        out.sort_by(|a, b| (&a.group, &a.label).cmp(&(&b.group, &b.label)));
        out
    }

    /// 记录总条数（`modem_id < 0` 表示全部）。
    pub fn count_for(&self, modem_id: i64) -> usize {
        let inner = self.inner.lock().unwrap();
        if modem_id < 0 {
            return inner.entries.len();
        }
        inner
            .entries
            .iter()
            .filter(|e| e.modem_id as i64 == modem_id)
            .count()
    }

    /// 清空记录（`modem_id < 0` 表示全部），返回清掉的条数。
    ///
    /// 同时清掉基线，这样清空后第一轮采样只是重建基线、不会立刻冒出「变化」。
    pub fn clear(&self, modem_id: i64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.entries.len();
        if modem_id < 0 {
            inner.entries.clear();
            inner.states.clear();
            return before;
        }
        inner.entries.retain(|e| e.modem_id as i64 != modem_id);
        inner.states.retain(|_, st| st.modem_id as i64 != modem_id);
        before - inner.entries.len()
    }

    /// 清掉某字段的节流时间戳（绕过 [`MIN_INTERVAL_SECS`] 连续制造变化，测试用）。
    #[cfg(test)]
    fn unthrottle(&self, modem_id: u32, group: &str, field: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(st) = inner.states.get_mut(&change_key(modem_id, group, field)) {
            st.rec_at = 0;
        }
    }

    /// 把某字段的时间戳往前拨（模拟时间流逝，测试用）。
    #[cfg(test)]
    fn age_field(&self, modem_id: u32, group: &str, field: &str, base: i64, rec: i64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(st) = inner.states.get_mut(&change_key(modem_id, group, field)) {
            if base != 0 {
                st.base_at -= base;
            }
            if rec != 0 {
                st.rec_at -= rec;
            }
        }
    }
}

fn change_key(modem_id: u32, group: &str, field: &str) -> String {
    format!("{modem_id}|{group}|{field}")
}

/// 物理范围校验。
pub fn int_in_range(v: i32, lo: i32, hi: i32) -> bool {
    v >= lo && v <= hi
}

/// 把空值归一化成「未上报」。
///
/// 对 IPv6 / APN 这类字段，「由有到无」本身就是一次有意义的变化，
/// 不能像其它空值那样直接丢弃。
pub fn not_reported(v: &str) -> String {
    if v.trim().is_empty() {
        "未上报".to_string()
    } else {
        v.to_string()
    }
}

/// RSRP → 等级（与前端 `rsrpLevel` 一致）。
pub fn rsrp_grade(v: i32) -> &'static str {
    match v {
        v if v >= -80 => "极好",
        v if v >= -90 => "良好",
        v if v >= -100 => "一般",
        v if v >= -110 => "偏弱",
        v if v >= -120 => "很弱",
        _ => "无信号",
    }
}

/// SINR → 等级（与前端 `sinrLevel` 一致）。
pub fn sinr_grade(v: i32) -> &'static str {
    match v {
        v if v >= 20 => "优秀",
        v if v >= 13 => "良好",
        v if v >= 7 => "一般",
        v if v >= 0 => "偏差",
        v if v >= -3 => "很差",
        _ => "无数据",
    }
}

/// RSRQ → 等级（与前端 `rsrqLevel` 一致；0 在前端是「无数据」）。
pub fn rsrq_grade(v: i32) -> &'static str {
    match v {
        0 => "无数据",
        v if v >= -10 => "优秀",
        v if v >= -15 => "良好",
        v if v >= -20 => "一般",
        v if v >= -25 => "偏差",
        _ => "很差",
    }
}

#[cfg(test)]
mod tests {
    use super::{rsrp_grade, rsrq_grade, sinr_grade, GROUP_SIG, SIG_THRESHOLD};

    #[test]
    fn first_observation_only_builds_baseline() {
        let t = super::Tracker::new();
        assert!(t
            .observe(0, GROUP_SIG, "tech", "网络类型", "5G NR")
            .is_none());
        assert_eq!(t.count_for(-1), 0);
        assert_eq!(t.current(0).len(), 1);
        assert_eq!(t.current(0)[0].value, "5G NR");
    }

    #[test]
    fn same_value_is_not_a_change_but_empty_is_ignored() {
        let t = super::Tracker::new();
        t.observe(0, GROUP_SIG, "tech", "网络类型", "5G NR");
        assert!(t
            .observe(0, GROUP_SIG, "tech", "网络类型", "5G NR")
            .is_none());
        assert!(t.observe(0, GROUP_SIG, "tech", "网络类型", "").is_none());
        let e = t
            .observe(0, GROUP_SIG, "tech", "网络类型", "4G LTE")
            .unwrap();
        assert_eq!(e.from, "5G NR");
        assert_eq!(e.to, "4G LTE");
        assert!(e.delta.is_empty());
        assert!(!e.up);
    }

    #[test]
    fn numeric_threshold_and_delta() {
        let t = super::Tracker::new();
        t.observe_numeric(
            0,
            GROUP_SIG,
            "rsrp",
            "信号强度 RSRP",
            -100,
            " dBm",
            SIG_THRESHOLD,
            Some(rsrp_grade),
        );
        // 1 dB 抖动不记
        assert!(t
            .observe_numeric(
                0,
                GROUP_SIG,
                "rsrp",
                "信号强度 RSRP",
                -99,
                " dBm",
                SIG_THRESHOLD,
                Some(rsrp_grade)
            )
            .is_none());
        // 4 dB 变化记一条，并带变化量与方向
        let e = t
            .observe_numeric(
                0,
                GROUP_SIG,
                "rsrp",
                "信号强度 RSRP",
                -95,
                " dBm",
                SIG_THRESHOLD,
                Some(rsrp_grade),
            )
            .unwrap();
        assert_eq!(e.from, "-100 dBm（一般）");
        assert_eq!(e.to, "-95 dBm（一般）");
        assert_eq!(e.delta, "+5 dBm");
        assert!(e.up);
    }

    #[test]
    fn grade_change_beats_threshold() {
        let t = super::Tracker::new();
        t.observe_numeric(
            0,
            GROUP_SIG,
            "rsrp",
            "信号强度 RSRP",
            -101,
            " dBm",
            SIG_THRESHOLD,
            Some(rsrp_grade),
        );
        // 只差 1 dB，但跨了「偏弱 → 一般」，必须记
        let e = t
            .observe_numeric(
                0,
                GROUP_SIG,
                "rsrp",
                "信号强度 RSRP",
                -100,
                " dBm",
                SIG_THRESHOLD,
                Some(rsrp_grade),
            )
            .unwrap();
        assert_eq!(e.to, "-100 dBm（一般）");
    }

    #[test]
    fn throttle_blocks_rapid_records() {
        let t = super::Tracker::new();
        t.observe(0, GROUP_SIG, "serving", "服务小区", "PCI 1");
        t.observe(0, GROUP_SIG, "serving", "服务小区", "PCI 2")
            .unwrap();
        // 同字段紧接着再变一次：被节流
        assert!(t
            .observe(0, GROUP_SIG, "serving", "服务小区", "PCI 3")
            .is_none());
        // 但「当前值」已经更新
        let cur = t.current(0);
        assert_eq!(cur[0].value, "PCI 3");
        t.unthrottle(0, GROUP_SIG, "serving");
        assert!(t
            .observe(0, GROUP_SIG, "serving", "服务小区", "PCI 4")
            .is_some());
    }

    #[test]
    fn held_seconds_uses_baseline_then_last_record() {
        let t = super::Tracker::new();
        t.observe(0, GROUP_SIG, "plmn", "PLMN", "46011");
        t.age_field(0, GROUP_SIG, "plmn", 120, 0);
        let e = t.observe(0, GROUP_SIG, "plmn", "PLMN", "46000").unwrap();
        assert_eq!(e.held_s, 120);
    }

    #[test]
    fn snapshot_is_newest_first_and_filtered_by_limit() {
        let t = super::Tracker::new();
        t.observe(0, GROUP_SIG, "a", "甲", "1");
        t.observe(0, GROUP_SIG, "a", "甲", "2").unwrap();
        t.observe(0, GROUP_SIG, "b", "乙", "1");
        t.observe(0, GROUP_SIG, "b", "乙", "2").unwrap();
        let all = t.snapshot(-1, 0);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].field, "b");
        assert_eq!(t.snapshot(-1, 1).len(), 1);
        assert_eq!(t.snapshot(1, 0).len(), 0);
    }

    #[test]
    fn clear_removes_entries_and_baseline() {
        let t = super::Tracker::new();
        t.observe(0, GROUP_SIG, "a", "甲", "1");
        t.observe(0, GROUP_SIG, "a", "甲", "2").unwrap();
        assert_eq!(t.clear(0), 1);
        assert_eq!(t.count_for(-1), 0);
        assert!(t.current(0).is_empty());
        // 清空后第一轮采样只重建基线
        assert!(t.observe(0, GROUP_SIG, "a", "甲", "3").is_none());
    }

    #[test]
    fn grades_match_frontend() {
        assert_eq!(rsrp_grade(-80), "极好");
        assert_eq!(rsrp_grade(-140), "无信号");
        assert_eq!(rsrq_grade(0), "无数据");
        assert_eq!(sinr_grade(25), "优秀");
    }
}
