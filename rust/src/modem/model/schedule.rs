//! 定时器（`GET /api/schedule` 与 `set|delete|toggle|run`）。
//!
//! 需求：到点自动锁频段 / 锁基站。一条任务含**两个时间点**——开始时间执行
//! 「开始动作」（如锁到 n78），结束时间执行「结束动作」（如解除锁定）；每个
//! 时间点可带**一组动作**（按顺序依次下发，"锁频段 + 锁基站"要一起做才有意义）。
//!
//! 判定口径（简单可预期，**不支持 cron 表达式**，只认"星期 + HH:MM"；
//! 依据 Go module/kernel/sched_view.go:110-118、api_schedule.go:8-12）：
//!   · 每天（或指定星期）的 HH:MM 到点后，在 [`CATCH_UP`] 窗口内执行一次；
//!   · 同一"日期+时间"只执行一次（`slots` 里记录，随 schedule.json 落盘），所以
//!     进程重启后只要还在窗口内就补做（不漏也不重复），超出窗口不补——
//!     避免"关机一整天，开机把昨天的动作全跑一遍"；
//!   · 时间判定全用**本机时区**（见 `logging::time::tz_offset_secs`，与界面说明一致）。
//!
//! 边界语义（上面三条的直接推论，改判定前先看）：
//!   · **跨日**：两个槽位各自在"自己那一天"判定（23:00 锁 / 07:00 解锁 = 当晚触发
//!     start、次日早上触发 end），没有"跨零点的时段"这回事（依据 Go
//!     module/kernel/store_test.go:747 `TestDuePhasesCrossDay`）；
//!   · **补做不跨零点**：23:00 的槽位过了 00:00 就永远不再补（窗口是同一天内的
//!     [到点, 到点+15min)），所以也不会出现"隔天早上补昨天的动作"；
//!   · **时区 / 夏令时**：本机时区只认 POSIX std 写法（`CST-8`），不解析 TZif 区域
//!     库、不套 DST 规则（见 `logging::time::tz_offset_secs`）；认不出时降级按 UTC
//!     并 warn。闰秒不处理——Unix 秒本身不含闰秒；
//!   · **补偿策略**：只补偿"晚一点"（进程重启 / 串口忙），不补偿"错过整天"；
//!     补偿发生在窗口内的下一次 `http::schedule::tick`，由 `slots` 保证幂等；
//!   · **幂等**：`slots[phase] == 本槽位` 即视为已执行，重复 tick / 反复重启都不会
//!     重复下发（依据 Go module/kernel/sched_view.go:922-935 `SchedStore.Record`）。
//!
//! 分层：本文件是纯逻辑（模型 / 校验 / 时间判定 / 视图组装）+ `Modem` 的执行
//! 入口；台账落盘在 `runtime::store::SchedStore`（去重标记与落盘节流的落点）；
//! 循环与 handler 在 `http::schedule`。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{msg, Result};
use crate::logging::{civil_from_days, local_unix_now, unix_now};

use super::modem::Modem;

// ------------------------------------------------------------------ 常量 ---

pub const KIND_BAND_LOCK: &str = "band_lock"; // 锁频段（指定频段列表）
pub const KIND_BAND_UNLOCK: &str = "band_unlock"; // 解除频段锁定（按 policy 恢复全量）
pub const KIND_CELL_LOCK: &str = "cell_lock"; // 锁基站（频点 [+PCI]）
pub const KIND_CELL_UNLOCK: &str = "cell_unlock"; // 解除基站锁定
pub const KIND_UNLOCK_ALL: &str = "unlock_all"; // 一键解除全部锁定（频段+基站）
pub const KIND_MODE_PREF: &str = "mode_pref"; // 组网模式偏好
pub const KIND_AIRPLANE_ON: &str = "airplane_on"; // 开飞行模式（AT+CFUN=4）
pub const KIND_AIRPLANE_OFF: &str = "airplane_off"; // 关飞行模式（AT+CFUN=1）

pub const PHASE_START: &str = "start";
pub const PHASE_END: &str = "end";
/// 手动执行的相位标记。**当前没有调用方会产生它**：界面的「立即执行」走
/// [`PHASE_START`]，记的是真实日期槽位（见 [`Run::slot`]）。
/// `runtime::store::SchedStore::record` 保留了它的豁免分支（收到它就不推进去重
/// 标记），与 Go module/kernel/sched_view.go:932 `if run.Slot != "manual"` 同口径。
pub const PHASE_MANUAL: &str = "manual";

// 以下 6 个阈值与 Go 版逐项对齐（module/kernel/sched_view.go:132-145），改任何一个
// 都会动到实机行为（触发时机 / 上限 / 刷屏频率），改动前先对上游说明原因。

/// 到点后仍允许补做的窗口（秒）。进程重启 / 串口忙导致晚几秒是常态，
/// 但"关机一整天"不应该开机就补跑，所以窗口取 15 分钟。
/// 依据 Go module/kernel/sched_view.go:132-134 `CatchUp = 15 * time.Minute`。
pub const CATCH_UP: i64 = 15 * 60;
/// 任务数上限（主要是防止前端误批量导入）。
/// 依据 Go module/kernel/sched_view.go:135-136 `MaxTasks = 50`。
pub const MAX_TASKS: usize = 50;
/// 单个时间点最多几个动作。常见组合是"锁频段 + 锁基站"，4 个足够。
/// 依据 Go module/kernel/sched_view.go:137-139 `MaxActions = 4`。
pub const MAX_ACTIONS: usize = 4;
/// 落盘节流（秒）：每次执行都会改状态，但不必秒级写盘。
/// 依据 Go module/kernel/sched_view.go:140-141 `SchedSaveEvery = 5 * time.Second`。
pub const SCHED_SAVE_EVERY: i64 = 5;
/// 「下次执行」展示几条。
/// 依据 Go module/kernel/sched_view.go:142-143 `NextShow = 3`。
pub const NEXT_SHOW: usize = 3;
/// 同一任务执行失败后的日志节流（秒），避免坏任务每 10 秒刷屏。
/// 依据 Go module/kernel/sched_view.go:144-145 `LogEvery = 5 * time.Minute`。
pub const LOG_EVERY: i64 = 5 * 60;

/// 语义档位（与前端 MODE_PREF_CN 一致的 7 档）。
pub const MODE_PREFS: [&str; 7] = [
    "AUTO",
    "WCDMA",
    "LTE",
    "NR5G",
    "WCDMA:LTE",
    "WCDMA:NR5G",
    "LTE:NR5G",
];

// ------------------------------------------------------------------ 模型 ---

/// 一条要执行的动作（含参数；提交格式与视图内嵌格式一致，前端可直接回传）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    pub kind: String,
    /// 锁频段 / 锁基站 / 解锁用：LTE / NR
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tech: String,
    /// "1:3:5:8" 或 "78"
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bands: String,
    /// 锁基站用
    #[serde(skip_serializing_if = "is_zero")]
    pub earfcn: i32,
    #[serde(skip_serializing_if = "is_zero")]
    pub pcid: i32,
    /// 1=频点+PCI 2=仅频点
    #[serde(skip_serializing_if = "is_zero")]
    pub lock_type: i32,
    /// 写 NV（重启后仍生效）
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub persist: bool,
    /// 组网模式偏好（语义档位或本机原码）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

/// 一次执行记录（界面「上次结果」一栏）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    /// start / end / manual
    pub phase: String,
    /// 触发槽位："本地日期 + 该相位 HH:MM"，如 "2026-09-30 08:00"（见 [`slot`]）。
    ///
    /// 手动执行记的也是这种真实槽位（走 [`PHASE_START`]），所以手动跑完之后循环
    /// 不会再补一遍；字面量 "manual" 只在 [`PHASE_MANUAL`] 里保留，当前无人产生。
    pub slot: String,
    /// 执行时刻（RFC3339 UTC；空串 = 没执行过）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub at: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub msg: String,
}

/// 一条定时任务。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub enabled: bool,
    /// 目标模组：`port:/dev/ttyUSB2`（与配额同一套键，避免 IMEI 相同的两台模组串台）
    pub modem_key: String,
    /// 仅展示
    #[serde(skip_serializing_if = "String::is_empty")]
    pub port: String,
    /// 仅用于换端口后的迁移（IMEI 唯一匹配才迁）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub imei: String,
    /// 生效的星期：0=周日 … 6=周六；空 = 每天
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<i64>,
    /// "HH:MM"
    pub start_time: String,
    /// 非空（至少 1 个）
    pub start_actions: Vec<Action>,
    /// 可空（不需要结束动作就留空）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub end_time: String,
    /// 可空；end_time 非空时必须有值
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub end_actions: Vec<Action>,
    /// 各槽位"最后一次执行的槽位标识"，用于去重与补做判定。
    ///
    /// key 是相位（start / end），value 是 [`slot`] 的产出；随 schedule.json 落盘。
    /// 幂等就靠它：`slots[phase]` 等于当前槽位时判定为"已执行"，补做与重启都安全
    /// （依据 Go module/kernel/sched_view.go:922-935 `SchedStore.Record`）。
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub slots: HashMap<String, String>,
    /// 最近一次执行结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<Run>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
}

/// 动作类型的展示信息（前端下拉用）。
#[derive(Debug, Clone, Serialize)]
pub struct KindInfo {
    pub kind: &'static str,
    pub label: &'static str,
    /// 需要模组支持锁网指令
    pub needs_locker: bool,
    /// 需要选择制式
    pub has_tech: bool,
    pub needs_bands: bool,
    pub needs_cell: bool,
    pub needs_mode: bool,
}

/// 支持的动作类型（顺序即界面下拉顺序）。
pub fn kinds() -> Vec<KindInfo> {
    vec![
        KindInfo {
            kind: KIND_BAND_LOCK,
            label: "锁频段",
            needs_locker: true,
            has_tech: true,
            needs_bands: true,
            needs_cell: false,
            needs_mode: false,
        },
        KindInfo {
            kind: KIND_BAND_UNLOCK,
            label: "解除锁频段",
            needs_locker: true,
            has_tech: true,
            needs_bands: false,
            needs_cell: false,
            needs_mode: false,
        },
        KindInfo {
            kind: KIND_CELL_LOCK,
            label: "锁基站（频点/PCI）",
            needs_locker: true,
            has_tech: true,
            needs_bands: false,
            needs_cell: true,
            needs_mode: false,
        },
        KindInfo {
            kind: KIND_CELL_UNLOCK,
            label: "解除锁基站",
            needs_locker: true,
            has_tech: true,
            needs_bands: false,
            needs_cell: false,
            needs_mode: false,
        },
        KindInfo {
            kind: KIND_UNLOCK_ALL,
            label: "解除全部锁定",
            needs_locker: true,
            has_tech: false,
            needs_bands: false,
            needs_cell: false,
            needs_mode: false,
        },
        KindInfo {
            kind: KIND_MODE_PREF,
            label: "组网模式偏好",
            needs_locker: true,
            has_tech: false,
            needs_bands: false,
            needs_cell: false,
            needs_mode: true,
        },
        KindInfo {
            kind: KIND_AIRPLANE_ON,
            label: "开飞行模式",
            needs_locker: false,
            has_tech: false,
            needs_bands: false,
            needs_cell: false,
            needs_mode: false,
        },
        KindInfo {
            kind: KIND_AIRPLANE_OFF,
            label: "关飞行模式",
            needs_locker: false,
            has_tech: false,
            needs_bands: false,
            needs_cell: false,
            needs_mode: false,
        },
    ]
}

/// 按类型取元信息。
pub fn kind_info_for(kind: &str) -> Option<KindInfo> {
    kinds().into_iter().find(|k| k.kind == kind)
}

/// 动作类型的展示名（找不到时原样返回 kind）。
pub fn kind_label(kind: &str) -> String {
    kind_info_for(kind)
        .map(|k| k.label.to_string())
        .unwrap_or_else(|| kind.to_string())
}

fn or_dash(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        "-".to_string()
    } else {
        t.to_string()
    }
}

/// 动作的可读描述（列表里显示"做了什么"）。
pub fn action_text(a: &Action) -> String {
    match a.kind.as_str() {
        KIND_BAND_LOCK => format!("锁频段 {} {}", or_dash(&a.tech), or_dash(&a.bands)),
        KIND_BAND_UNLOCK => format!("解除锁频段 {}", or_dash(&a.tech)),
        KIND_CELL_LOCK => {
            if a.lock_type == 2 {
                format!("锁基站 {} 频点 {}", or_dash(&a.tech), a.earfcn)
            } else {
                format!("锁基站 {} {}/PCI {}", or_dash(&a.tech), a.earfcn, a.pcid)
            }
        }
        KIND_CELL_UNLOCK => format!("解除锁基站 {}", or_dash(&a.tech)),
        KIND_UNLOCK_ALL => "解除全部锁定".to_string(),
        KIND_MODE_PREF => format!("组网模式 → {}", or_dash(&super::mode::mode_text(&a.mode))),
        KIND_AIRPLANE_ON => "开飞行模式".to_string(),
        KIND_AIRPLANE_OFF => "关飞行模式".to_string(),
        other => or_dash(other),
    }
}

/// 一组动作的可读描述（多个动作按执行顺序用 " + " 连接）。
pub fn actions_label(actions: &[Action]) -> String {
    match actions.len() {
        0 => "-".to_string(),
        1 => action_text(&actions[0]),
        _ => actions
            .iter()
            .map(action_text)
            .collect::<Vec<_>>()
            .join(" + "),
    }
}

/// 去掉完全重复的动作（保留第一次出现的位置）；顺序保持不变。
pub fn dedupe_actions(actions: Vec<Action>) -> Vec<Action> {
    let mut out = Vec::with_capacity(actions.len());
    let mut seen = std::collections::HashSet::new();
    for a in actions {
        let key = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            a.kind, a.tech, a.bands, a.earfcn, a.pcid, a.lock_type, a.persist, a.mode
        );
        if seen.insert(key) {
            out.push(a);
        }
    }
    out
}

// ------------------------------------------------------------------ 校验 ---

/// 解析 "HH:MM"（容忍 "8:00" / 前后空格）。
pub fn parse_hhmm(s: &str) -> Result<(u32, u32)> {
    let s = s.trim();
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(msg(format!("时间格式应为 HH:MM，得到 {s:?}")));
    }
    let h: i64 = parts[0]
        .trim()
        .parse()
        .map_err(|_| msg(format!("时间格式应为 HH:MM（00:00~23:59），得到 {s:?}")))?;
    let m: i64 = parts[1]
        .trim()
        .parse()
        .map_err(|_| msg(format!("时间格式应为 HH:MM（00:00~23:59），得到 {s:?}")))?;
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return Err(msg(format!(
            "时间格式应为 HH:MM（00:00~23:59），得到 {s:?}"
        )));
    }
    Ok((h as u32, m as u32))
}

/// 解析 API / 定时任务传入的技术域参数（4g/5g 别名一并支持）。
pub fn parse_lock_tech(s: &str) -> Result<String> {
    match s.trim().to_ascii_uppercase().as_str() {
        "LTE" | "4G" => Ok("LTE".to_string()),
        "NR" | "5G" | "NR5G" | "NR5G-SA" => Ok("NR".to_string()),
        "NRNSA" | "NR-NSA" | "NSA" | "5G-NSA" | "NR5G-NSA" => Ok("NRNSA".to_string()),
        other => Err(msg(format!(
            "tech 只认 LTE / NR（另支持 NRNSA），得到 {other:?}"
        ))),
    }
}

/// 频点号是否落在该技术域的合法范围：
/// LTE EARFCN 0~65535（TS 36.101）、NR ARFCN 0~3279165（TS 38.104，含 FR1/FR2）。
pub fn earfcn_valid(tech: &str, v: i32) -> bool {
    if v <= 0 {
        return false;
    }
    if tech == "NR" || tech == "NRNSA" {
        v <= 3_279_165
    } else {
        v <= 65_535
    }
}

/// 「原码形态」判定：拦住 5G 这类无意义输入，放行真实码形态。
///
/// 规则：只允许 ASCII 字母数字 + `:`/`_`，且满足其一——
/// 纯数字（"00"/"38"/"71"）、0x 十六进制（"0x00000109"）、
/// 带分隔符（"LTE_ONLY"/"NR5G:SA"）、纯字母缩写（"GSM"/"TDS"）。
/// 混合字母数字又没分隔符的（"5G"/"4g"）一律拒绝。
pub fn looks_like_mode_code(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 32 {
        return false;
    }
    let ok_char = |c: char| c.is_ascii_alphanumeric() || c == ':' || c == '_';
    if !s.chars().all(ok_char) {
        return false;
    }
    if s.to_ascii_lowercase().starts_with("0x") {
        return s[2..].chars().all(|c| c.is_ascii_hexdigit()) && s.len() > 2;
    }
    // 全数字
    if s.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // 带分隔符（":" / "_"）：每段非空即可 —— "NR5G:SA" 这类段内混字母数字的真码
    if s.contains(':') || s.contains('_') {
        return s.split([':', '_']).all(|w| !w.is_empty());
    }
    // 纯字母缩写
    s.chars().all(|c| c.is_ascii_alphabetic())
}

/// 校验单个动作，并顺手归一化参数（制式大写、频段排序、模式大写）。
/// 用 `&mut`：归一化的结果必须写回调用方。
pub fn validate_action(a: &mut Action) -> Result<()> {
    match a.kind.as_str() {
        KIND_BAND_LOCK => {
            let tech =
                parse_lock_tech(&a.tech).map_err(|_| msg("锁频段需要选择制式（LTE / NR）"))?;
            a.tech = tech;
            let bands = super::lock::parse_band_list(&a.bands)
                .ok_or_else(|| msg("锁频段需要填写频段号，如 78 或 1:3:5:8"))?;
            if bands.len() > 32 {
                return Err(msg("频段最多 32 个"));
            }
            a.bands = bands
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(":");
        }
        KIND_BAND_UNLOCK | KIND_CELL_UNLOCK => {
            a.tech = parse_lock_tech(&a.tech).map_err(|_| msg("需要选择制式（LTE / NR）"))?;
        }
        KIND_CELL_LOCK => {
            let tech =
                parse_lock_tech(&a.tech).map_err(|_| msg("锁基站需要选择制式（LTE / NR）"))?;
            a.tech = tech.clone();
            if a.lock_type != 1 && a.lock_type != 2 {
                return Err(msg("锁基站需要选择锁定方式（频点+PCI / 仅频点）"));
            }
            // 5G 的 QNWLOCK 语法没有「仅频点」档位（PCI 是必填位），保存时就拦住，
            // 别等到定时任务跑起来才失败（见 model::lock::set_cell_lock）。
            if tech == "NR" && a.lock_type != 1 {
                return Err(msg("5G NR 锁小区只支持「频点+PCI」，没有「仅频点」档位"));
            }
            if !earfcn_valid(&a.tech, a.earfcn) {
                return Err(msg(format!("频点号 {} 不合法", a.earfcn)));
            }
            if a.lock_type == 1 && !(0..=1007).contains(&a.pcid) {
                return Err(msg("PCI 应为 0~1007"));
            }
        }
        KIND_UNLOCK_ALL => {} // 无参数
        KIND_MODE_PREF => {
            // 语义档位（全 7 档）或**本机原码**（未取证厂商靠它）；具体能不能写由方言决定
            let m = a.mode.trim().to_ascii_uppercase();
            if !MODE_PREFS.contains(&m.as_str()) && !looks_like_mode_code(&m) {
                return Err(msg(format!(
                    "组网模式应为 {} 之一，或本机原码（数字/十六进制/自报短码，见锁网页下拉）",
                    MODE_PREFS.join(" / ")
                )));
            }
            a.mode = m;
        }
        KIND_AIRPLANE_ON | KIND_AIRPLANE_OFF => {} // 无参数
        other => return Err(msg(format!("未知动作类型 {other:?}"))),
    }
    Ok(())
}

/// 校验一组动作（数量上限 + 逐个校验）；`what` 是错误前缀（"开始"/"结束"）。
fn validate_actions(actions: &mut [Action], what: &str) -> Result<()> {
    if actions.is_empty() {
        return Err(msg(if what == "结束" {
            "填了结束时间就要选结束动作（不需要结束动作请把结束时间留空）".to_string()
        } else {
            format!("{what}动作至少要有一个")
        }));
    }
    if actions.len() > MAX_ACTIONS {
        return Err(msg(format!(
            "一个时间点最多 {MAX_ACTIONS} 个动作（{what}动作给的是 {} 个）",
            actions.len()
        )));
    }
    let single = actions.len() == 1;
    for (i, a) in actions.iter_mut().enumerate() {
        validate_action(a).map_err(|e| {
            if single {
                msg(format!("{what}动作：{e}"))
            } else {
                msg(format!("{what}动作 {}（{}）：{e}", i + 1, action_text(a)))
            }
        })?;
    }
    Ok(())
}

/// 校验任务；会顺手把时间补成 "HH:MM" 规范写法、去重动作、排序星期、补任务名。
pub fn validate_task(t: &mut Task) -> Result<()> {
    if t.id.trim().is_empty() {
        return Err(msg("缺少任务 id"));
    }
    t.id = t.id.trim().to_string();
    if t.modem_key.trim().is_empty() {
        return Err(msg("请选择目标模组（或直接填串口名，如 COM32）"));
    }
    t.modem_key = t.modem_key.trim().to_string();
    // 开始
    let (h, m) = parse_hhmm(&t.start_time).map_err(|e| msg(format!("开始时间：{e}")))?;
    t.start_time = format!("{h:02}:{m:02}");
    validate_actions(&mut t.start_actions, "开始")?;
    t.start_actions = dedupe_actions(std::mem::take(&mut t.start_actions));
    // 结束（可选）
    if t.end_time.trim().is_empty() {
        // 没结束时间就不保留结束动作，避免"看起来会执行其实不会"
        t.end_time.clear();
        t.end_actions.clear();
    } else {
        let (h2, m2) = parse_hhmm(&t.end_time).map_err(|e| msg(format!("结束时间：{e}")))?;
        t.end_time = format!("{h2:02}:{m2:02}");
        if t.end_time == t.start_time {
            return Err(msg("结束时间不能与开始时间相同"));
        }
        validate_actions(&mut t.end_actions, "结束")?;
        t.end_actions = dedupe_actions(std::mem::take(&mut t.end_actions));
    }
    // 星期：0(周日)~6(周六)，去重排序
    let mut seen = std::collections::HashSet::new();
    let mut days = Vec::with_capacity(t.days.len());
    for d in &t.days {
        if !(0..=6).contains(d) {
            return Err(msg(format!("星期取值应为 0(周日)~6(周六)，得到 {d}")));
        }
        if seen.insert(*d) {
            days.push(*d);
        }
    }
    days.sort_unstable();
    t.days = days;
    if t.name.trim().is_empty() {
        t.name = format!("{} {}", actions_label(&t.start_actions), t.start_time);
    } else {
        t.name = t.name.trim().to_string();
    }
    Ok(())
}

// ------------------------------------------------------------------ 时间判定 ---

/// 槽位标识：把"某天某个 HH:MM"写成一个稳定字符串（本地日期 + 时间），用于去重
/// （依据 Go module/kernel/sched_view.go:513-516 `Slot`）。
pub fn slot(day: i64, hhmm: &str) -> String {
    format!("{} {}", day_text(day), hhmm)
}

/// 本地"日"序号 → "YYYY-MM-DD"。
fn day_text(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 本地"日"序号 → 星期（0=周日 … 6=周六；1970-01-01 是周四）。
///
/// 0=周日 的口径与 Go `time.Weekday` 一致（依据 Go module/kernel/sched_view.go:518-529
/// `DayOK`），`days` 里的取值也是这一套，两边不能各改一半。
pub fn weekday_of(day: i64) -> i64 {
    (day + 4).rem_euclid(7)
}

/// 星期几的中文缩写。
pub fn weekday_cn(w: i64) -> &'static str {
    ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][(w.rem_euclid(7)) as usize]
}

/// 今天是否该执行（`days` 为空 = 每天）。
pub fn day_ok(days: &[i64], day: i64) -> bool {
    if days.is_empty() {
        return true;
    }
    days.contains(&weekday_of(day))
}

/// 任务的一个时间槽。
#[derive(Debug, Clone, Copy)]
pub struct Phase<'a> {
    /// PHASE_START / PHASE_END
    pub name: &'static str,
    /// "HH:MM"
    pub hhmm: &'a str,
}

/// 任务的所有时间槽（start 一定有，end 可能没有）。
///
/// 两个槽位是**独立的**：各自按自己那天的 HH:MM 判定，不做"跨零点的时段"拼接。
/// 所以 23:00 开始 / 07:00 结束 = 当晚锁、次日早上解锁（依据 Go
/// module/kernel/store_test.go:747 `TestDuePhasesCrossDay`）。
pub fn phases(t: &Task) -> Vec<Phase<'_>> {
    let mut out = vec![Phase {
        name: PHASE_START,
        hhmm: t.start_time.trim(),
    }];
    if !t.end_time.trim().is_empty() && !t.end_actions.is_empty() {
        out.push(Phase {
            name: PHASE_END,
            hhmm: t.end_time.trim(),
        });
    }
    out
}

/// 取某个槽位的动作列表（按执行顺序）。
pub fn actions_of<'a>(t: &'a Task, phase: &str) -> &'a [Action] {
    if phase == PHASE_END {
        &t.end_actions
    } else {
        &t.start_actions
    }
}

/// 取该槽位的 "HH:MM"（记录槽位标识用）。
pub fn time_of_phase<'a>(t: &'a Task, phase: &str) -> &'a str {
    if phase == PHASE_END {
        &t.end_time
    } else {
        &t.start_time
    }
}

/// 本地"日"序号 + "HH:MM" → 本地 Unix 秒（解析失败返回 None）。
///
/// 只做"当天 HH:MM"的落点换算：不跨日、不带时区（调用方传的已是本地"日"序号，
/// 时区在 `logging::local_unix_now` 里折过一次）。HH:MM 非法时返回 None，让调用方
/// 跳过该槽位——历史台账里可能存着没过校验的时间串。
fn phase_at(day: i64, hhmm: &str) -> Option<i64> {
    let (h, m) = parse_hhmm(hhmm).ok()?;
    Some(day * 86_400 + h as i64 * 3600 + m as i64 * 60)
}

/// 返回现在该执行的槽位（已去重、已考虑补做窗口）。
///
/// 判定：槽位时间 <= now < 槽位时间 + [`CATCH_UP`]（**左闭右开**，到点那一秒就算、
/// 满 15 分钟就不再补），且今天星期匹配，且 `slots[phase]` 不是本槽位（上次执行记
/// 的就是它）。依据 Go module/kernel/sched_view.go:566-590 `DuePhases`。
///
/// 补做与幂等：本函数只回答"该不该跑"，跑没跑过由 `slots` 记账（落点在
/// `runtime::store::SchedStore::record`），所以进程重启后窗口内会补一次、窗口外不补，
/// 重复调用不会重复返回同一个槽位。停用的任务一律不返回。
pub fn due_phases(t: &Task, now_local: i64) -> Vec<String> {
    if !t.enabled {
        return Vec::new();
    }
    let day = now_local.div_euclid(86_400);
    phases(t)
        .into_iter()
        .filter_map(|p| {
            let at = phase_at(day, p.hhmm)?;
            if now_local < at || now_local >= at + CATCH_UP {
                return None;
            }
            if !day_ok(&t.days, day) {
                return None;
            }
            if t.slots.get(p.name).map(String::as_str) == Some(slot(day, p.hhmm).as_str()) {
                return None; // 这个槽位已经跑过了
            }
            Some(p.name.to_string())
        })
        .collect()
}

/// 计算接下来 `n` 次执行时间（本地 Unix 秒，**只用于展示**，不参与触发判定）。
///
/// 扫未来 8 天（够覆盖"只周日"这类稀疏周期，依据 Go module/kernel/sched_view.go:592-617）、
/// 两个槽位都算，按星期过滤后再排序截断。
///
/// 与上游的一处有意差异：Go 是"收集到 n 条就停"（:599-610），本实现是"收满 8 天再排序
/// 截断"。跨天任务（start 22:00 / end 07:00）在当天的收集顺序是 start→end，上游会先拿到
/// 22:00；本实现排序后给的是真正更早的 07:00。展示口径更正确，故不回退成上游写法。
pub fn next_runs(t: &Task, now_local: i64, n: usize) -> Vec<i64> {
    if !t.enabled || n == 0 {
        return Vec::new();
    }
    let today = now_local.div_euclid(86_400);
    let mut out: Vec<i64> = Vec::new();
    for day in today..today + 8 {
        for p in phases(t) {
            let Some(at) = phase_at(day, p.hhmm) else {
                continue;
            };
            if at <= now_local || !day_ok(&t.days, day) {
                continue;
            }
            out.push(at);
        }
    }
    out.sort_unstable();
    out.truncate(n);
    out
}

/// 把本地 Unix 秒列表格式化成 "09-30 08:00 周三"（与 Go `Format("01-02 15:04")` 同口径：
/// **不含年份**，最长只展示到 8 天内，界面上下文里够用——依据
/// Go module/kernel/sched_view.go:633-639 `TimesText`）。
pub fn times_text(ts: &[i64]) -> Vec<String> {
    ts.iter()
        .map(|t| {
            let day = t.div_euclid(86_400);
            let rem = t.rem_euclid(86_400);
            let (_y, m, d) = civil_from_days(day);
            format!(
                "{m:02}-{d:02} {:02}:{:02} {}",
                rem / 3600,
                (rem % 3600) / 60,
                weekday_cn(weekday_of(day))
            )
        })
        .collect()
}

/// 任务 id（时间戳 + 随机后缀，足够唯一且可读）。
///
/// 上游用 crypto/rand 取 3 字节（Go module/kernel/sched_view.go:619-626 `NewID`）；
/// 这里用 `RandomState`（每进程随机播种）+ 进程内序号，只求**不碰撞**——id 不是
/// 凭据，不需要密码学随机；跨重启的唯一性由时间戳兜底。
pub fn new_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    // RandomState 每进程随机播种；时间戳 + 序号兜底跨重启的唯一性
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(local_unix_now() as u64);
    h.write_u64(n);
    format!("t{}{:06x}", unix_now(), h.finish() & 0xff_ffff)
}

// ------------------------------------------------------------------ 视图 ---

/// 单个动作的展示视图：原始参数 + 可读文案 + 该模组是否支持
/// （`Action` 内嵌进来，JSON 里参数字段与提交格式完全一致，前端可直接回传）。
#[derive(Debug, Clone, Serialize)]
pub struct ActionView {
    #[serde(flatten)]
    pub action: Action,
    pub text: String,
    /// 空串 = 支持；非空是原因（例如固件不支持锁网）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unsupported: String,
}

/// 把动作列表转成带文案/支持性的视图；同时返回第一个"不支持"的原因
/// （供列表胶囊与按钮提示使用）。
pub fn action_views(
    actions: &[Action],
    support: Option<&dyn Fn(&Action) -> String>,
) -> (Vec<ActionView>, String) {
    let mut out = Vec::with_capacity(actions.len());
    let mut first = String::new();
    for a in actions {
        let mut v = ActionView {
            action: a.clone(),
            text: action_text(a),
            unsupported: String::new(),
        };
        if let Some(support) = support {
            let why = support(a);
            if !why.is_empty() {
                v.unsupported = why.clone();
                if first.is_empty() {
                    first = why;
                }
            }
        }
        out.push(v);
    }
    (out, first)
}

/// 给前端的任务视图（含展示用派生字段）。
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    pub target_label: String,
    /// 目标模组当前在线且运行中
    pub target_ok: bool,
    /// 与内嵌的原始字段同名（序列化时覆盖它）：前端拿到的就是带文案的动作列表
    pub start_actions: Vec<ActionView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub end_actions: Vec<ActionView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub next_runs: Vec<String>,
    /// 该侧第一个不支持的动作原因（空串 = 全支持）；列表状态胶囊用它
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start_unsupported: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub end_unsupported: String,
}

/// 目标模组下拉项。
#[derive(Debug, Clone, Serialize)]
pub struct ModemOption {
    pub key: String,
    pub label: String,
    pub id: u32,
}

/// 定时器页面的完整数据。
#[derive(Debug, Clone, Serialize)]
pub struct SchedView {
    /// 服务端当前时刻（RFC3339；界面提示用）
    pub now: String,
    pub tasks: Vec<TaskView>,
    pub modems: Vec<ModemOption>,
    pub kinds: Vec<KindInfo>,
    pub max_tasks: usize,
    /// 至少有一台模组支持锁网（否则页面顶部提示）
    pub supported: bool,
}

/// 组装单个任务的视图。`target_label` / `target_ok` / `support` 都由调用方提供：
/// 这些都要看 `Modem` 的能力，视图层不管模组在不在。
pub fn task_view_of(
    t: &Task,
    now_local: i64,
    target_label: String,
    target_ok: bool,
    support: Option<&dyn Fn(&Action) -> String>,
) -> TaskView {
    let (start_actions, start_unsupported) = action_views(&t.start_actions, support);
    let (end_actions, end_unsupported) = action_views(&t.end_actions, support);
    TaskView {
        task: t.clone(),
        target_label,
        target_ok,
        next_runs: times_text(&next_runs(t, now_local, NEXT_SHOW)),
        start_actions,
        end_actions,
        start_unsupported,
        end_unsupported,
    }
}

// ------------------------------------------------------------------ 执行 ---

/// 该模组的「模组键」（与配额同一套口径：端口优先）。
///
/// 注意：不用 IMEI 当主键——实测两块同型号模组可能回报一模一样的 IMEI，
/// 按 IMEI 存会让两台模组串台（锁错模组比不锁更糟）。端口号在同一条 USB 通路上
/// 是稳定的，所以取端口；没有端口才退到 `id:`（依据 Go module/kernel/schedule.go:143-173
/// `schedResolve` 用同一套键）。
pub fn quota_key(m: &Modem) -> String {
    let port = m.port();
    if port.trim().is_empty() {
        format!("id:{}", m.id)
    } else {
        format!("port:{}", port.trim())
    }
}

impl Modem {
    /// 串口此刻是否已打开。
    pub fn is_running(&self) -> bool {
        self.at.lock().unwrap().is_some()
    }

    /// IMEI（带进程内缓存）。
    ///
    /// [`Self::imei`] 每次都发 AT；定时器的目标解析 / 视图组装会反复问同一台
    /// 模组，这里缓存一份（换口不换机器，IMEI 不变，无需失效）。
    pub fn imei_cached(&self) -> Option<String> {
        let mut slot = self.imei_cache.lock().unwrap();
        if slot.is_none() {
            *slot = self.imei();
        }
        slot.clone().filter(|s| !s.trim().is_empty())
    }

    /// 组网模式指令是否可用（30 秒 TTL 缓存：视图会对每个动作问一遍）。
    pub(super) fn mode_write_supported(&self) -> bool {
        let mut slot = self.mode_supported_slot.lock().unwrap();
        if let Some((v, at)) = *slot {
            if at.elapsed() < std::time::Duration::from_secs(30) {
                return v;
            }
        }
        let v = self.mode_pref().supported;
        *slot = Some((v, std::time::Instant::now()));
        v
    }

    /// 是否支持开飞行模式（30 秒 TTL 缓存，同上）。
    pub(super) fn airplane_supported_cached(&self) -> bool {
        let mut slot = self.airplane_supported_slot.lock().unwrap();
        if let Some((v, at)) = *slot {
            if at.elapsed() < std::time::Duration::from_secs(30) {
                return v;
            }
        }
        let v = self.airplane().supported;
        *slot = Some((v, std::time::Instant::now()));
        v
    }

    /// 该模组的固件是否支持这个动作（用于界面提前提示 + 执行前拦截）。
    ///
    /// 与执行同一套能力口径：锁网看**真被固件接受过**的方言探测（不是猜测兜底），
    /// 组网模式看 mode 指令回读，机型不支持关射频的（F50：模组射频是整机唯一
    /// 上行）「开飞行模式」提前标不支持——「关飞行模式」仍允许（它是恢复手段）。
    /// 依据 Go module/kernel/schedule.go:113-137 `schedActionSupported`。
    pub fn sched_action_supported(&self, kind: &str) -> Result<()> {
        let Some(info) = kind_info_for(kind) else {
            return Err(msg(format!("未知动作类型 {kind:?}")));
        };
        let who = format!("模组 {}（{}）", self.port(), self.vendor().label());
        if info.needs_locker && !self.lock_dialect().probed {
            return Err(msg(format!(
                "{who}的固件不支持锁网指令，无法执行「{}」",
                info.label
            )));
        }
        // 组网模式看的是 mode 指令（与锁网方言是两套表）
        if info.needs_mode && !self.mode_write_supported() {
            return Err(msg(format!(
                "{who}未识别出组网模式指令，无法执行「{}」",
                info.label
            )));
        }
        if kind == KIND_AIRPLANE_ON && !self.airplane_supported_cached() {
            return Err(msg(format!(
                "{who}：{}",
                super::airplane::airplane_blocked_reason()
            )));
        }
        Ok(())
    }

    /// 执行一个定时动作。所有分支都复用已有的锁网 / 组网模式 / 飞行模式实现，
    /// 不为定时器另开一条下发路径（回读确认口径完全一致）。
    /// 依据 Go module/kernel/schedule.go:51-111 `ApplySchedAction`；分支顺序与
    /// 参数处理（含"解除锁频段走全量写回、绝不写 0"）保持一致。
    pub fn apply_sched_action(&self, a: &Action) -> Result<()> {
        match a.kind.as_str() {
            KIND_BAND_LOCK => {
                let tech = parse_lock_tech(&a.tech)?;
                let bands = super::lock::parse_band_list(&a.bands)
                    .ok_or_else(|| msg("锁频段需要填写频段号，如 78 或 1:3:5:8"))?;
                self.set_band_lock(&tech, &bands, false)?;
            }
            KIND_BAND_UNLOCK => {
                let tech = parse_lock_tech(&a.tech)?;
                // unlock=true：按 policy 全量写回，绝不写 0（会清空频段导致脱网）
                self.set_band_lock(&tech, &[], true)?;
            }
            KIND_CELL_LOCK => {
                let tech = parse_lock_tech(&a.tech)?;
                self.set_cell_lock(&tech, a.lock_type, a.earfcn, a.pcid, None, None)?;
                if a.persist {
                    // 附加的持久化指令：best-effort，失败只 warn（锁定本身已生效）
                    self.save_cell_lock();
                }
            }
            KIND_CELL_UNLOCK => {
                let tech = parse_lock_tech(&a.tech)?;
                self.set_cell_lock(&tech, 0, 0, 0, None, None)?;
            }
            KIND_UNLOCK_ALL => {
                self.unlock_all()?;
            }
            KIND_MODE_PREF => {
                // 语义档位（全 7 档）或本机原码，value 由校验层归一化过
                self.set_mode_pref(&a.mode)?;
            }
            KIND_AIRPLANE_ON => {
                self.set_airplane(true)?;
            }
            KIND_AIRPLANE_OFF => {
                self.set_airplane(false)?;
            }
            other => return Err(msg(format!("未知动作类型 {other:?}"))),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
