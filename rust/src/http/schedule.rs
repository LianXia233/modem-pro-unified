//! 定时器的域逻辑：视图组装、后台循环 tick 与「立即执行」（供 handlers/schedule*.rs 复用）。
//!
//! 与 `http/quota.rs` 同级：这里放**域逻辑**，handler 只管取参数与回信封。
//!
//! 口径要点：
//!   · 循环**不看有没有客户端在线**：定时动作（锁频段/锁基站）必须到点就执行；
//!   · 启动先补跑一轮，把"进程重启前几分钟内该做但还没来得及做"的槽位补上
//!     （补做窗口与去重见 modem/model/schedule.rs 的 `due_phases`）；
//!   · 目标解析先按端口键精确匹配；miss 且任务记了 IMEI、且**唯一**一台模组
//!     匹配时迁移键 —— 两台模组 IMEI 相同时不猜（宁可不执行，也不能锁错模组）。

use std::sync::Arc;

use crate::error::{msg, Result};
use crate::http::AppState;
use crate::logging::{local_unix_now, rfc3339, unix_now};
use crate::modem::model::schedule as sched;
use crate::modem::model::{Action, Modem, Run, Task, MAX_TASKS};

/// GET /api/schedule 的完整视图（列表 + 可选动作 + 目标模组下拉，页面一次拉全）。
pub fn view(state: &AppState) -> sched::SchedView {
    let now_local = local_unix_now();
    let mut v = sched::SchedView {
        now: rfc3339(unix_now()),
        tasks: Vec::new(),
        modems: Vec::new(),
        kinds: sched::kinds(),
        max_tasks: MAX_TASKS,
        supported: false,
    };
    for m in state.mgr.all() {
        if m.lock_dialect().probed {
            v.supported = true;
        }
        let mut label = format!("#{} {}", m.id, m.port());
        if let Some(model) = m.model().filter(|s| !s.is_empty()) {
            label.push(' ');
            label.push_str(&model);
        }
        v.modems.push(sched::ModemOption {
            key: sched::quota_key(&m),
            label,
            id: m.id,
        });
    }
    for mut t in state.sched.snapshot() {
        // 目标解析（含换端口迁移）：解析成功才有支持性判定，否则不判
        let target = resolve(state, &mut t);
        let mut label = t.modem_key.clone();
        let mut ok = false;
        if let Some(m) = &target {
            label = format!("#{} {}", m.id, m.port());
            ok = m.is_running();
        }
        if !ok {
            label.push_str("（当前不在线）");
        }
        let support_get = target.as_deref().map(|m| {
            move |a: &Action| -> String {
                m.sched_action_supported(&a.kind)
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            }
        });
        let support: Option<&dyn Fn(&Action) -> String> = support_get
            .as_ref()
            .map(|f| f as &dyn Fn(&Action) -> String);
        v.tasks
            .push(sched::task_view_of(&t, now_local, label, ok, support));
    }
    // 开始时间升序，再按任务名；并列保持原顺序
    v.tasks.sort_by(|a, b| {
        a.task
            .start_time
            .cmp(&b.task.start_time)
            .then_with(|| a.task.name.cmp(&b.task.name))
    });
    v
}

/// 广播定时器状态（前端「定时器」页与概览提示用）。
pub fn push(state: &AppState) {
    let data = serde_json::to_value(view(state)).unwrap_or(serde_json::Value::Null);
    state.ws.broadcast("schedule", None, data);
}

/// 广播一份已组装好的视图（set/delete/toggle 与 push 共用一次组装结果）。
pub fn push_view(state: &AppState, v: &sched::SchedView) {
    let data = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
    state.ws.broadcast("schedule", None, data);
}

/// 把任务的目标解析成在线模组（`&mut`：命中迁移时顺手改任务的键）。
pub(crate) fn resolve(state: &AppState, t: &mut Task) -> Option<Arc<Modem>> {
    let all = state.mgr.all();
    for m in &all {
        if sched::quota_key(m) == t.modem_key {
            return Some(m.clone());
        }
    }
    let imei = t.imei.trim();
    if imei.is_empty() {
        return None;
    }
    let hits: Vec<&Arc<Modem>> = all
        .iter()
        .filter(|m| m.imei_cached().map(|s| s.trim() == imei).unwrap_or(false))
        .collect();
    if hits.len() != 1 {
        // 两台模组 IMEI 相同时不猜（宁可不执行，也不能锁错模组）
        return None;
    }
    let hit = hits[0].clone();
    let old = t.modem_key.clone();
    t.modem_key = sched::quota_key(&hit);
    log::info!("定时任务 {} 换端口迁移目标：{old} → {}", t.id, t.modem_key);
    state.sched.retarget(&t.id, &t.modem_key);
    Some(hit)
}

/// 后台循环跑一轮：把到点的槽位执行掉（每 `schedule_interval` 秒一次，启动先补一轮）。
pub fn tick(state: &AppState) {
    let now_local = local_unix_now();
    let mut changed = false;
    for mut t in state.sched.snapshot() {
        let due = sched::due_phases(&t, now_local);
        if due.is_empty() {
            continue;
        }
        let modem = resolve(state, &mut t);
        for phase in due {
            let Some(m) = &modem else {
                state
                    .sched
                    .record(&t.id, skip_run(&t, &phase, "目标模组当前不在线，已跳过"));
                changed = true;
                continue;
            };
            if !m.is_running() {
                state.sched.record(
                    &t.id,
                    skip_run(&t, &phase, "模组未运行（串口未打开），已跳过"),
                );
                changed = true;
                continue;
            }
            let run = run_phase(state, m, &t, &phase);
            state.sched.record(&t.id, run);
            changed = true;
        }
    }
    if changed {
        push(state);
        state.sched.flush();
    }
}

/// 目标不在线 / 串口未开时的失败记录（不改动作，只记账）。
fn skip_run(t: &Task, phase: &str, reason: &str) -> Run {
    Run {
        phase: phase.to_string(),
        slot: sched::slot(
            local_unix_now().div_euclid(86_400),
            sched::time_of_phase(t, phase),
        ),
        at: rfc3339(unix_now()),
        ok: false,
        msg: reason.to_string(),
    }
}

/// 执行一个槽位（按顺序执行该时间点的全部动作）并返回结果记录。
///
/// 多个动作是**依次下发**的：一个动作失败不影响后面的（例如固件不支持锁频段，
/// 但飞行模式那一步仍然要执行），最终结果里会写清成功几个、哪个失败。
fn run_phase(state: &AppState, modem: &Modem, t: &Task, phase: &str) -> Run {
    let actions = sched::actions_of(t, phase);
    let mut run = Run {
        phase: phase.to_string(),
        slot: sched::slot(
            local_unix_now().div_euclid(86_400),
            sched::time_of_phase(t, phase),
        ),
        at: rfc3339(unix_now()),
        ok: false,
        msg: String::new(),
    };
    if actions.is_empty() {
        run.msg = "该时间点没有动作（请重新编辑任务）".to_string();
        return run;
    }

    let mut fails: Vec<String> = Vec::new();
    let mut done = 0;
    for a in actions {
        let text = sched::action_text(a);

        // 先看固件支持不支持（避免把"不支持"当执行失败反复写日志）
        if let Err(e) = modem.sched_action_supported(&a.kind) {
            fails.push(format!("{text}：{e}"));
            if state.sched.log_allowed(&t.id) {
                log::warn!("定时任务 {} 的动作不被支持（模组 {}）：{e}", t.id, modem.id);
            }
            continue;
        }
        if let Err(e) = modem.apply_sched_action(a) {
            fails.push(format!("{text}：{e}"));
            if state.sched.log_allowed(&t.id) {
                log::warn!("定时任务 {}（{}）执行失败：{text}：{e}", t.id, t.name);
            }
            continue;
        }
        done += 1;
        log::info!("定时任务 {}（{}）{}：{text} 已下发", t.id, t.name, phase);
    }

    if fails.is_empty() {
        run.ok = true;
        run.msg = if actions.len() == 1 {
            "已执行".to_string()
        } else {
            format!("已执行 {done} 个动作：{}", sched::actions_label(actions))
        };
    } else if done > 0 {
        run.msg = format!(
            "已执行 {done}/{} 个动作；失败：{}",
            actions.len(),
            fails.join("；")
        );
    } else {
        run.msg = fails.join("；");
    }
    run
}

/// 手动执行一次（界面的「立即执行」按钮）：跑开始动作并把当前窗口内的
/// 开始槽位标记为已跑（避免这次手动执行后循环又自动补一遍）。
pub fn run_now(state: &AppState, id: &str) -> Result<Run> {
    let Some(mut task) = state.sched.snapshot().into_iter().find(|t| t.id == id) else {
        return Err(msg("任务不存在"));
    };
    let Some(m) = resolve(state, &mut task) else {
        return Err(msg("目标模组当前不在线"));
    };
    if !m.is_running() {
        return Err(msg("模组未运行（串口未打开）"));
    }
    let run = run_phase(state, &m, &task, sched::PHASE_START);
    state.sched.record(id, run.clone());
    push(state);
    state.sched.flush();
    Ok(run)
}

/// 保存前用目标模组补全 Port / IMEI（仅展示与换端口迁移用）。
///
/// 前端没给目标（modem_key 为空）时回退到请求里的 modem_id。
pub(crate) fn fill_target(state: &AppState, t: &mut Task, modem_id: u32) {
    let mut target = if t.modem_key.is_empty() {
        None
    } else {
        state
            .mgr
            .all()
            .into_iter()
            .find(|m| sched::quota_key(m) == t.modem_key)
    };
    if target.is_none() && t.modem_key.is_empty() {
        if let Some(m) = state.mgr.get(modem_id) {
            t.modem_key = sched::quota_key(&m);
            target = Some(m);
        }
    }
    if let Some(m) = target {
        t.port = m.port();
        t.imei = m.imei_cached().unwrap_or_default();
    }
}
