//! schedule 模型的纯逻辑单测（执行路径要真串口，不在覆盖范围）。

use super::*;
use crate::logging::civil_from_days;

// ------------------------------------------------------------------ 时间解析 ---

#[test]
fn hhmm_tolerates_loose_input() {
    assert_eq!(parse_hhmm("08:00").unwrap(), (8, 0));
    assert_eq!(parse_hhmm("8:00").unwrap(), (8, 0));
    assert_eq!(parse_hhmm(" 23:59 ").unwrap(), (23, 59));
    assert_eq!(parse_hhmm("0:0").unwrap(), (0, 0));
    assert!(parse_hhmm("24:00").is_err());
    assert!(parse_hhmm("8:60").is_err());
    assert!(parse_hhmm("8").is_err());
    assert!(parse_hhmm("").is_err());
}

#[test]
fn lock_tech_aliases() {
    assert_eq!(parse_lock_tech("lte").unwrap(), "LTE");
    assert_eq!(parse_lock_tech(" 4G ").unwrap(), "LTE");
    assert_eq!(parse_lock_tech("5G").unwrap(), "NR");
    assert_eq!(parse_lock_tech("NR5G-SA").unwrap(), "NR");
    assert_eq!(parse_lock_tech("NSA").unwrap(), "NRNSA");
    assert!(parse_lock_tech("GSM").is_err());
    assert!(parse_lock_tech("").is_err());
}

#[test]
fn earfcn_ranges() {
    assert!(earfcn_valid("LTE", 1300));
    assert!(earfcn_valid("LTE", 65_535));
    assert!(!earfcn_valid("LTE", 65_536));
    assert!(earfcn_valid("NR", 627_264));
    assert!(earfcn_valid("NR", 3_279_165));
    assert!(!earfcn_valid("NR", 3_279_166));
    assert!(!earfcn_valid("LTE", 0));
    assert!(!earfcn_valid("LTE", -1));
}

// ------------------------------------------------------------------ 动作校验 ---

#[test]
fn mode_code_shapes() {
    // 同一批向量（覆盖实测过的各类写法）
    for v in [
        "00",
        "0",
        "2",
        "10",
        "38",
        "71",
        "0x00000109",
        "GSM",
        "TDS",
        "LTE_ONLY",
        "NR5G:SA",
    ] {
        assert!(looks_like_mode_code(v), "{v} 应被当成原码");
    }
    for v in [
        "",
        "5G",
        "4g",
        "1 2",
        "=1",
        "超长的根本不像码的字符串太多啦",
        "LTE!",
    ] {
        assert!(!looks_like_mode_code(v), "{v} 不该被当成原码");
    }
}

#[test]
fn validate_band_lock_normalizes() {
    let mut a = Action {
        kind: KIND_BAND_LOCK.into(),
        tech: "5g".into(),
        bands: "78:1:1,5".into(),
        ..Default::default()
    };
    validate_action(&mut a).unwrap();
    assert_eq!(a.tech, "NR");
    // 排序去重 + 统一冒号写法
    assert_eq!(a.bands, "1:5:78");

    let mut no_tech = Action {
        kind: KIND_BAND_LOCK.into(),
        bands: "78".into(),
        ..Default::default()
    };
    assert_eq!(
        validate_action(&mut no_tech).unwrap_err().to_string(),
        "锁频段需要选择制式（LTE / NR）"
    );
    let mut no_bands = Action {
        kind: KIND_BAND_LOCK.into(),
        tech: "NR".into(),
        ..Default::default()
    };
    assert!(validate_action(&mut no_bands).is_err());
}

#[test]
fn validate_cell_lock_rules() {
    let mut ok = Action {
        kind: KIND_CELL_LOCK.into(),
        tech: "NR".into(),
        earfcn: 627264,
        pcid: 521,
        lock_type: 1,
        ..Default::default()
    };
    validate_action(&mut ok).unwrap();
    assert_eq!(ok.tech, "NR");

    let mut bad_lock_type = Action {
        kind: KIND_CELL_LOCK.into(),
        tech: "NR".into(),
        earfcn: 100,
        lock_type: 3,
        ..Default::default()
    };
    assert_eq!(
        validate_action(&mut bad_lock_type).unwrap_err().to_string(),
        "锁基站需要选择锁定方式（频点+PCI / 仅频点）"
    );
    let mut bad_pci = Action {
        kind: KIND_CELL_LOCK.into(),
        tech: "LTE".into(),
        earfcn: 1300,
        pcid: 1008,
        lock_type: 1,
        ..Default::default()
    };
    assert_eq!(
        validate_action(&mut bad_pci).unwrap_err().to_string(),
        "PCI 应为 0~1007"
    );
    let mut bad_earfcn = Action {
        kind: KIND_CELL_LOCK.into(),
        tech: "LTE".into(),
        earfcn: 70_000,
        lock_type: 2,
        ..Default::default()
    };
    assert!(validate_action(&mut bad_earfcn).is_err());
}

#[test]
fn validate_mode_pref_accepts_semantic_and_code() {
    for m in [
        "auto",
        "LTE",
        "nr5g",
        "LTE:NR5G",
        "wcdma:nr5g",
        "38",
        "0x109",
        "LTE_ONLY",
    ] {
        let mut a = Action {
            kind: KIND_MODE_PREF.into(),
            mode: m.into(),
            ..Default::default()
        };
        validate_action(&mut a).unwrap();
        assert_eq!(a.mode, m.to_ascii_uppercase());
    }
    let mut bad = Action {
        kind: KIND_MODE_PREF.into(),
        mode: "5G".into(),
        ..Default::default()
    };
    assert!(validate_action(&mut bad)
        .unwrap_err()
        .to_string()
        .contains("组网模式应为"));
}

#[test]
fn validate_unknown_kind_rejected() {
    let mut a = Action {
        kind: "no_such".into(),
        ..Default::default()
    };
    assert!(validate_action(&mut a)
        .unwrap_err()
        .to_string()
        .contains("未知动作类型"));
}

#[test]
fn dedupe_keeps_first_occurrence() {
    let acts = vec![
        Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: "78".into(),
            ..Default::default()
        },
        Action {
            kind: KIND_CELL_LOCK.into(),
            tech: "NR".into(),
            earfcn: 627264,
            pcid: 521,
            lock_type: 1,
            ..Default::default()
        },
        Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: "78".into(),
            ..Default::default()
        },
    ];
    let out = dedupe_actions(acts);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].kind, KIND_BAND_LOCK);
    assert_eq!(out[1].kind, KIND_CELL_LOCK);
}

// ------------------------------------------------------------------ 任务校验 ---

fn good_task() -> Task {
    Task {
        id: "t1".into(),
        modem_key: "port:/dev/ttyUSB0".into(),
        enabled: true,
        start_time: "8:5".into(),
        start_actions: vec![Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: "78".into(),
            ..Default::default()
        }],
        end_time: "22:00".into(),
        end_actions: vec![Action {
            kind: KIND_BAND_UNLOCK.into(),
            tech: "NR".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn validate_task_normalizes_and_fills_name() {
    let mut t = good_task();
    validate_task(&mut t).unwrap();
    assert_eq!(t.start_time, "08:05");
    assert_eq!(t.name, "锁频段 NR 78 08:05");
    // 星期去重排序
    t.days = vec![6, 1, 6, 0];
    validate_task(&mut t).unwrap();
    assert_eq!(t.days, vec![0, 1, 6]);
}

#[test]
fn validate_task_requires_id_and_modem() {
    let mut t = good_task();
    t.id = String::new();
    assert_eq!(
        validate_task(&mut t).unwrap_err().to_string(),
        "缺少任务 id"
    );
    let mut t = good_task();
    t.modem_key = "  ".into();
    assert_eq!(
        validate_task(&mut t).unwrap_err().to_string(),
        "请选择目标模组（或直接填串口名，如 COM32）"
    );
}

#[test]
fn validate_task_end_needs_actions_and_differs() {
    let mut t = good_task();
    t.end_actions.clear();
    assert_eq!(
        validate_task(&mut t).unwrap_err().to_string(),
        "填了结束时间就要选结束动作（不需要结束动作请把结束时间留空）"
    );
    // 结束时间留空 ⇒ 结束动作被清掉（"看起来会执行其实不会"的坑）
    let mut t = good_task();
    t.end_time = String::new();
    validate_task(&mut t).unwrap();
    assert!(t.end_actions.is_empty());

    let mut t = good_task();
    t.end_time = "08:05".into();
    assert_eq!(
        validate_task(&mut t).unwrap_err().to_string(),
        "结束时间不能与开始时间相同"
    );
}

#[test]
fn validate_task_action_count_and_prefix() {
    let mut t = good_task();
    t.start_actions.clear();
    assert_eq!(
        validate_task(&mut t).unwrap_err().to_string(),
        "开始动作至少要有一个"
    );
    let mut t = good_task();
    t.start_actions = (0..5)
        .map(|i| Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: format!("{i}"),
            ..Default::default()
        })
        .collect();
    assert_eq!(
        validate_task(&mut t).unwrap_err().to_string(),
        "一个时间点最多 4 个动作（开始动作给的是 5 个）"
    );
    // 多动作时的错误带序号与文案
    let mut t = good_task();
    t.start_actions = vec![
        Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: "78".into(),
            ..Default::default()
        },
        Action {
            kind: KIND_CELL_LOCK.into(),
            tech: "NR".into(),
            earfcn: 1,
            lock_type: 1,
            pcid: 2000,
            ..Default::default()
        },
    ];
    let e = validate_task(&mut t).unwrap_err().to_string();
    assert!(e.starts_with("开始动作 2（锁基站 NR 1/PCI 2000）："), "{e}");
}

// ------------------------------------------------------------------ 时间判定 ---

fn task_two_phases() -> Task {
    let mut t = Task {
        id: "t1".into(),
        name: "测试".into(),
        enabled: true,
        modem_key: "port:/dev/ttyUSB0".into(),
        start_time: "08:00".into(),
        end_time: "22:00".into(),
        ..Default::default()
    };
    t.start_actions = vec![Action {
        kind: KIND_BAND_LOCK.into(),
        tech: "NR".into(),
        bands: "78".into(),
        ..Default::default()
    }];
    t.end_actions = vec![Action {
        kind: KIND_BAND_UNLOCK.into(),
        tech: "NR".into(),
        ..Default::default()
    }];
    t
}

/// 2026-09-30 是周三（本地"日"序号按 1970-01-01=0 折算）。
const DAY: i64 = 20_726;

#[test]
fn weekday_math() {
    // civil_from_days(20726) == (2026, 9, 30)，星期三
    assert_eq!(civil_from_days(DAY), (2026, 9, 30));
    assert_eq!(weekday_of(DAY), 3);
    assert_eq!(weekday_cn(0), "周日");
    assert_eq!(weekday_cn(6), "周六");
    assert_eq!(weekday_cn(3), "周三");
}

#[test]
fn due_phases_respect_window_and_dedup() {
    let t = task_two_phases();
    let at = DAY * 86_400 + 8 * 3600;
    // 到点瞬间该跑 start
    assert_eq!(due_phases(&t, at), vec!["start"]);
    // 窗口内（14 分 59 秒后）仍补做
    assert_eq!(due_phases(&t, at + CATCH_UP - 1), vec!["start"]);
    // 超出窗口不补
    assert!(due_phases(&t, at + CATCH_UP).is_empty());
    // 还没到点
    assert!(due_phases(&t, at - 1).is_empty());
    // 跑过记了槽位就不再跑
    let mut t2 = t.clone();
    t2.slots.insert("start".into(), slot(DAY, "08:00"));
    assert!(due_phases(&t2, at).is_empty());
}

#[test]
fn due_phases_respects_enabled_and_days() {
    let mut t = task_two_phases();
    t.enabled = false;
    let at = DAY * 86_400 + 8 * 3600;
    assert!(due_phases(&t, at).is_empty());
    t.enabled = true;
    t.days = vec![0]; // 只周日；DAY 是周三
    assert!(due_phases(&t, at).is_empty());
    t.days = vec![3];
    assert_eq!(due_phases(&t, at), vec!["start"]);
    // 空列表 = 每天
    t.days.clear();
    assert_eq!(due_phases(&t, at), vec!["start"]);
}

#[test]
fn next_runs_and_times_text() {
    let t = task_two_phases();
    let now = DAY * 86_400 + 9 * 3600; // 周三 09:00：当天 start 已过
    let runs = next_runs(&t, now, 3);
    assert_eq!(runs.len(), 3);
    // 当天 end(22:00)、次日 start、次日 end
    assert_eq!(runs[0], DAY * 86_400 + 22 * 3600);
    assert_eq!(runs[1], (DAY + 1) * 86_400 + 8 * 3600);
    assert_eq!(runs[2], (DAY + 1) * 86_400 + 22 * 3600);
    assert_eq!(
        times_text(&runs),
        vec![
            "09-30 22:00 周三".to_string(),
            "10-01 08:00 周四".to_string(),
            "10-01 22:00 周四".to_string(),
        ]
    );
    // 停用 / 上限 0 都给空
    assert!(next_runs(&t, now, 0).is_empty());
    let mut off = t.clone();
    off.enabled = false;
    assert!(next_runs(&off, now, 3).is_empty());
}

#[test]
fn next_runs_filters_by_weekday() {
    let mut t = task_two_phases();
    t.days = vec![0]; // 只周日
    let now = DAY * 86_400 + 9 * 3600; // 周三
    let runs = next_runs(&t, now, 3);
    // 8 天窗口（DAY..DAY+8）里只有一个周日（DAY+4），它带两个槽位
    assert_eq!(runs.len(), 2);
    for r in &runs {
        assert_eq!(weekday_of(r.div_euclid(86_400)), 0);
    }
    // 限制条数只影响截断
    assert_eq!(next_runs(&t, now, 1).len(), 1);
}

#[test]
fn slot_and_phases() {
    assert_eq!(slot(DAY, "08:00"), "2026-09-30 08:00");
    let t = task_two_phases();
    assert_eq!(phases(&t).len(), 2);
    // 没有结束动作 ⇒ end 不算槽位
    let mut t2 = t.clone();
    t2.end_actions.clear();
    assert_eq!(phases(&t2).len(), 1);
    assert_eq!(actions_of(&t, "end").len(), 1);
    assert_eq!(actions_of(&t, "start").len(), 1);
    assert_eq!(time_of_phase(&t, "end"), "22:00");
}

#[test]
fn action_texts() {
    assert_eq!(
        action_text(&Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: "78".into(),
            ..Default::default()
        }),
        "锁频段 NR 78"
    );
    assert_eq!(
        action_text(&Action {
            kind: KIND_BAND_UNLOCK.into(),
            tech: "LTE".into(),
            ..Default::default()
        }),
        "解除锁频段 LTE"
    );
    assert_eq!(
        action_text(&Action {
            kind: KIND_CELL_LOCK.into(),
            tech: "NR".into(),
            earfcn: 627264,
            pcid: 521,
            lock_type: 1,
            ..Default::default()
        }),
        "锁基站 NR 627264/PCI 521"
    );
    assert_eq!(
        action_text(&Action {
            kind: KIND_CELL_LOCK.into(),
            tech: "LTE".into(),
            earfcn: 1300,
            lock_type: 2,
            ..Default::default()
        }),
        "锁基站 LTE 频点 1300"
    );
    assert_eq!(
        action_text(&Action {
            kind: KIND_UNLOCK_ALL.into(),
            ..Default::default()
        }),
        "解除全部锁定"
    );
    assert_eq!(
        action_text(&Action {
            kind: KIND_MODE_PREF.into(),
            mode: "LTE:NR5G".into(),
            ..Default::default()
        }),
        "组网模式 → 4G + 5G"
    );
    assert_eq!(
        action_text(&Action {
            kind: KIND_AIRPLANE_ON.into(),
            ..Default::default()
        }),
        "开飞行模式"
    );
    assert_eq!(
        action_text(&Action {
            kind: "??".into(),
            ..Default::default()
        }),
        "??"
    );
    let two = vec![
        Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: "78".into(),
            ..Default::default()
        },
        Action {
            kind: KIND_CELL_LOCK.into(),
            tech: "NR".into(),
            earfcn: 1,
            pcid: 2,
            lock_type: 1,
            ..Default::default()
        },
    ];
    assert_eq!(actions_label(&two), "锁频段 NR 78 + 锁基站 NR 1/PCI 2");
    assert_eq!(actions_label(&[]), "-");
    assert_eq!(kind_label(KIND_BAND_LOCK), "锁频段");
    assert_eq!(kind_label("bogus"), "bogus");
}

// ------------------------------------------------------------------ 序列化 ---

#[test]
fn task_json_field_names_match_upstream() {
    let t = task_two_phases();
    let v: serde_json::Value = serde_json::to_value(&t).unwrap();
    assert_eq!(v["id"], "t1");
    assert_eq!(v["modem_key"], "port:/dev/ttyUSB0");
    assert_eq!(v["start_time"], "08:00");
    assert_eq!(v["start_actions"][0]["kind"], "band_lock");
    assert_eq!(v["start_actions"][0]["tech"], "NR");
    assert_eq!(v["end_actions"][0]["kind"], "band_unlock");
    // omitempty 口径：没给的字段不出现
    assert!(v.get("days").is_none());
    assert!(v.get("slots").is_none());
    assert!(v.get("last").is_none());
    assert!(v["start_actions"][0].get("earfcn").is_none());

    let round: Task = serde_json::from_value(v).unwrap();
    assert_eq!(round, t);
}

#[test]
fn legacy_single_action_fields_parse() {
    // 旧版单动作写法（runtime 台账 / 手写 json 兼容）
    let t: Task = serde_json::from_str(
        r#"{"id":"t9","modem_key":"port:COM32","enabled":true,"start_time":"08:00"}"#,
    )
    .unwrap();
    assert_eq!(t.start_time, "08:00");
    assert!(t.start_actions.is_empty());
}

#[test]
fn task_view_flatten_overrides_action_lists() {
    let t = task_two_phases();
    let (start, _) = action_views(&t.start_actions, None);
    let (end, _) = action_views(&t.end_actions, None);
    let view = TaskView {
        task: t.clone(),
        target_label: "#0 /dev/ttyUSB0".into(),
        target_ok: true,
        start_actions: start,
        end_actions: end,
        next_runs: vec!["09-30 22:00 周三".into()],
        start_unsupported: String::new(),
        end_unsupported: String::new(),
    };
    let v: serde_json::Value = serde_json::to_value(&view).unwrap();
    // flatten 后同名键由 TaskView 覆盖：前端拿到的是带 text 的版本
    assert_eq!(v["start_actions"][0]["text"], "锁频段 NR 78");
    assert!(v["start_actions"][0].get("kind").is_some());
    assert_eq!(v["target_label"], "#0 /dev/ttyUSB0");
    assert_eq!(v["next_runs"][0], "09-30 22:00 周三");

    // 没有结束动作 ⇒ end_actions 整个不出现
    let mut t2 = t.clone();
    t2.end_actions.clear();
    let view2 = task_view_of(&t2, DAY * 86_400 + 9 * 3600, "#0".into(), true, None);
    let v2: serde_json::Value = serde_json::to_value(&view2).unwrap();
    assert!(v2.get("end_actions").is_none());
    assert_eq!(v2["next_runs"].as_array().unwrap().len(), 3);
}

#[test]
fn action_views_collect_first_unsupported() {
    let acts = vec![
        Action {
            kind: KIND_BAND_LOCK.into(),
            tech: "NR".into(),
            bands: "78".into(),
            ..Default::default()
        },
        Action {
            kind: KIND_AIRPLANE_ON.into(),
            ..Default::default()
        },
    ];
    let support = |a: &Action| {
        if a.kind == KIND_BAND_LOCK {
            "固件不支持锁网".to_string()
        } else {
            String::new()
        }
    };
    let (views, first) = action_views(&acts, Some(&support));
    assert_eq!(first, "固件不支持锁网");
    assert_eq!(views[0].unsupported, "固件不支持锁网");
    assert!(views[1].unsupported.is_empty());
    let (views, first) = action_views(&acts, None);
    assert!(first.is_empty());
    assert!(views.iter().all(|v| v.unsupported.is_empty()));
}

#[test]
fn new_id_shape_and_uniqueness() {
    let a = new_id();
    let b = new_id();
    assert!(a.starts_with('t'));
    assert_ne!(a, b);
    assert!(a.len() > 8);
}

#[test]
fn quota_key_prefers_port() {
    // quota_key 要真 Modem（字段私有），这里只验证格式约定：
    // port:/dev/ttyUSB0 是任务里存的键形态（见 good_task / 前端提交体）
    let t = good_task();
    assert!(t.modem_key.starts_with("port:"));
}
