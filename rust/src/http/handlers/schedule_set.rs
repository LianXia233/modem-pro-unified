//! `POST /api/schedule/set` —— 新增 / 更新一条任务（body 就是 Task；id 为空则新建）。
//!
//! body 另可带 `modem_id`，用于目标模组留空时的回退；兼容早期前端的
//! `start_action` / `end_action` 单动作写法（等同于只有一个元素的动作列表）。

use std::collections::HashMap;

use serde::Deserialize;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, schedule, AppState};
use crate::modem::model::{schedule as sched, Action, Task};

#[derive(Deserialize)]
struct SetReq {
    #[serde(flatten)]
    task: Task,
    #[serde(default)]
    modem_id: u32,
    // 兼容早期前端的单动作写法
    #[serde(default)]
    start_action: Option<Action>,
    #[serde(default)]
    end_action: Option<Action>,
}

pub fn handle(mut req: Request, state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    let body = request::read_body(&mut req);
    // 解析失败必须回信封：`?` 会把 Request 丢回网关层，连接上什么都不会写，
    // 页面只看到「modemd 无响应」，真正的 JSON 格式问题反而看不到。
    let mut parsed: SetReq = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(e) => return response::fail(req, -1, &format!("请求体应为 JSON 任务：{e}")),
    };
    let mut t = parsed.task;
    if t.start_actions.is_empty() {
        if let Some(a) = parsed.start_action.take() {
            t.start_actions = vec![a];
        }
    }
    if t.end_actions.is_empty() {
        if let Some(a) = parsed.end_action.take() {
            t.end_actions = vec![a];
        }
    }
    if t.id.trim().is_empty() {
        t.id = sched::new_id();
    }
    schedule::fill_target(state, &mut t, parsed.modem_id);
    if let Err(e) = state.sched.upsert(&mut t) {
        return response::fail(req, -1, &e.to_string());
    }
    log::info!(
        "定时任务已保存：{}「{}」目标 {} 启用={} {}({}) {}({})",
        t.id,
        t.name,
        t.modem_key,
        t.enabled,
        t.start_time,
        sched::actions_label(&t.start_actions),
        if t.end_time.is_empty() {
            "-"
        } else {
            &t.end_time
        },
        sched::actions_label(&t.end_actions),
    );

    let v = schedule::view(state);
    schedule::push_view(state, &v);
    response::json_body(req, v)
}
