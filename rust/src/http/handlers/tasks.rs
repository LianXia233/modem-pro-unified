//! 异步任务接口：`/api/tasks`（提交 / 查询 / 取消）。
//!
//! ## 契约
//!
//! ```text
//! POST /api/tasks            {kind, modem_id?, ...参数}  → {task}
//! GET  /api/tasks?modem_id=N&active=1                    → {tasks:[task], count}
//! GET  /api/tasks/get?id=t3                              → {task}
//! POST /api/tasks/cancel     {id:"t3"}                    → {id, cancelled:true}
//! ```
//!
//! `task` = `{id, kind, modem_id, label, state, progress?, created_at, elapsed_ms, result?, error?}`，
//! `state ∈ queued|running|done|failed|cancelled`。**提交立即返回**，慢活在工作线程里。
//!
//! ## 为什么不是「每个接口加个 async=1 参数」
//!
//! 任务需要跨请求共享（去重、取消、列表、结果保留），而 handler 是每请求一个栈帧。
//! 集中到注册表后：同一 `kind+模组+参数` 只跑一个（扫网被点两次不会排两条），
//! 页面切走再回来还能按 id 拿到结果。

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;
use tiny_http::{Method, Request};

use crate::error::{msg, Result};
use crate::http::tasks::Spec;
use crate::http::{request, response, AppState};
use crate::modem::model::{split_at_lines, Modem};

use super::modem_by_query;

/// 单条 AT 的长度上限（与 `/api/at` 一致）。
const MAX_AT_LEN: usize = 128;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    match *req.method() {
        Method::Get => handle_get(req, state, query),
        Method::Post => handle_post(req, state, query),
        _ => response::fail(req, -1, "method not allowed, use GET or POST"),
    }
}

fn handle_get(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    // 单条：/api/tasks/get?id=t3
    if let Some(id) = query.get("id").filter(|s| !s.trim().is_empty()) {
        let Some(t) = state.tasks.get(id.trim()) else {
            return response::fail(req, -1, &format!("task not found: {id}"));
        };
        return response::ok(req, json!({ "task": t.view() }));
    }
    // 列表：/api/tasks?modem_id=0&active=1
    let modem_id = query
        .get("modem_id")
        .and_then(|s| s.trim().parse::<u32>().ok());
    let active = request::bool_param(query, "active", false);
    let list = state.tasks.list(modem_id, active);
    response::ok(
        req,
        json!({ "tasks": list, "count": list.len(), "active": active }),
    )
}

fn handle_post(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));

    // 取消：/api/tasks/cancel {id}；也兼容「只有 id 没有 kind」的 POST（页面手滑也认）
    let wants_cancel = request::bool_param(&params, "cancel", false)
        || params.contains_key("cancel_id")
        || (params.get("id").map(|v| !v.trim().is_empty()).unwrap_or(false)
            && params.get("kind").map(|v| v.trim().is_empty()).unwrap_or(true));
    if wants_cancel {
        let id = params
            .get("id")
            .or_else(|| params.get("cancel_id"))
            .cloned()
            .unwrap_or_default();
        let ok = state.tasks.cancel(id.trim());
        return response::ok_msg(
            req,
            json!({ "id": id, "cancelled": ok }),
            if ok { "已请求取消" } else { "任务不存在或已结束" },
        );
    }

    let kind = params
        .get("kind")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if kind.is_empty() {
        return response::fail(req, -1, "kind required");
    }

    // 需要模组的种类：统一在这里解析一次，缺模组就当场拒（不要到工作线程才发现）
    let needs_modem = !matches!(kind.as_str(), "none");
    let modem = if needs_modem {
        match modem_by_query(state, &params) {
            Some(m) => Some(m),
            None => return response::fail(req, -1, "modem not found"),
        }
    } else {
        None
    };

    // 未知种类必须在这里用「正常响应」拒绝：`?` 会把错误抛给网关层，
    // 连接上什么都不会写（页面只见空回复，排查不到原因）。
    if let Err(e) = build_spec(&kind, modem.as_deref(), &params) {
        return response::fail(req, -1, &e.to_string());
    }
    let spec = build_spec(&kind, modem.as_deref(), &params)?;
    let tasks = &state.tasks;
    let cache = state.cache.clone();
    let cmd_timeout = state.cfg.serial.command_timeout();
    let allow_roaming = state.allow_roaming();

    let task = match kind.as_str() {
        // ---- 读取类（结果进缓存，供 /api/snapshot 立即读到）----
        "signal.refresh" => {
            let m = need(&modem)?;
            tasks.submit(spec, move |_t| {
                let v = m.signal()?;
                let j = serde_json::to_value(&v)?;
                cache.put("signal", m.id, j.clone());
                Ok(j)
            })
        }
        "network.refresh" => {
            let m = need(&modem)?;
            tasks.submit(spec, move |_t| {
                let v = m.network_status(allow_roaming);
                let j = serde_json::to_value(&v)?;
                cache.put("network", m.id, j.clone());
                Ok(j)
            })
        }
        "cells.scan" => {
            let m = need(&modem)?;
            tasks.submit(spec, move |t| {
                t.progress("正在扫描基站（约 5~20 秒）");
                let v = m.scan_cells();
                if t.cancelled() {
                    return Err(msg("已取消（扫描结果丢弃）"));
                }
                let j = serde_json::to_value(&v)?;
                cache.put("cells", m.id, j.clone());
                Ok(j)
            })
        }
        "lock.status" => {
            let m = need(&modem)?;
            let refresh = request::bool_param(&params, "refresh", true);
            tasks.submit(spec, move |_t| {
                let v = m.lock_status(refresh);
                let j = serde_json::to_value(&v)?;
                cache.put("lock", m.id, j.clone());
                Ok(j)
            })
        }
        // ---- 写操作 ----
        "at" => {
            let m = need(&modem)?;
            let cmd = at_command(&params)?;
            tasks.submit(spec, move |t| {
                t.progress(format!("等待模组应答：{cmd}"));
                let cancelled = t.clone();
                let out = m.with_at(|p| {
                    p.command_cancellable(&cmd, cmd_timeout, &|| cancelled.cancelled())
                })?;
                let (resp, stopped) = out;
                Ok(json!({
                    "cmd": cmd,
                    "resp": split_at_lines(&resp),
                    "cancelled": stopped,
                }))
            })
        }
        "ussd.send" => {
            let m = need(&modem)?;
            let code = params.get("code").cloned().unwrap_or_default();
            if code.trim().is_empty() {
                return response::fail(req, -1, "code required");
            }
            tasks.submit(spec, move |_t| {
                let r = m.send_ussd(&code)?;
                Ok(json!({
                    "code": code,
                    "result": r.map(serde_json::to_value).transpose()?,
                }))
            })
        }
        "sms.send" => {
            let m = need(&modem)?;
            let phone = params.get("phone").cloned().unwrap_or_default();
            let content = params.get("content").cloned().unwrap_or_default();
            if phone.trim().is_empty() || content.is_empty() {
                return response::fail(req, -1, "phone / content required");
            }
            tasks.submit(spec, move |t| {
                t.progress("正在发送短信（长短信会分多条）");
                let r = m.send_sms(&phone, &content)?;
                Ok(serde_json::to_value(r)?)
            })
        }
        "usbnet.set" => {
            let m = need(&modem)?;
            let value = request::i64_param(&params, "value", -1) as i32;
            tasks.submit(spec, move |_t| {
                let st = m.set_usbnet(value)?;
                Ok(serde_json::to_value(st)?)
            })
        }
        "lock.band" => {
            let m = need(&modem)?;
            let tech = params
                .get("tech")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let unlock = request::bool_param(&params, "unlock", false);
            let bands = parse_bands(params.get("bands").map(String::as_str).unwrap_or(""));
            if !unlock && bands.is_empty() {
                return response::fail(req, -1, "要么给几个频段（bands），要么显式 unlock=true");
            }
            tasks.submit(spec, move |t| {
                t.progress("正在写入频段锁定（会重启射频）");
                let v = m.set_band_lock(&tech, &bands, unlock)?;
                let j = serde_json::to_value(&v)?;
                cache.put("lock", m.id, j.clone());
                Ok(j)
            })
        }
        "netscan" => {
            let m = need(&modem)?;
            let tech = params
                .get("tech")
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "lte".to_string());
            let minutes = request::i64_param(&params, "minutes", 3);
            tasks.submit(spec, move |t| {
                t.progress(format!("正在扫网（最多 {minutes} 分钟）"));
                let v = m.scan_network(&tech, minutes);
                let j = serde_json::to_value(&v)?;
                cache.put("netscan", m.id, j.clone());
                Ok(j)
            })
        }
        "reboot" => {
            let m = need(&modem)?;
            tasks.submit(spec, move |t| {
                t.progress("已下发重启，模组会掉线约 20 秒");
                let r = m.reboot()?;
                Ok(serde_json::to_value(r)?)
            })
        }
        other => {
            return response::fail(req, -1, &format!("未知任务种类：{other}"));
        }
    };

    response::ok_msg(
        req,
        json!({ "task": task.view() }),
        if task.is_active() { "已提交" } else { "任务已结束" },
    )
}

/// 取模组（`needs_modem` 已保证存在，这里只是把 `Option` 拆开）。
fn need(modem: &Option<Arc<Modem>>) -> Result<Arc<Modem>> {
    modem
        .clone()
        .ok_or_else(|| msg("该任务需要 modem_id"))
}

/// 组装任务描述：标签给人看，去重键把参数也算进去。
fn build_spec(kind: &str, modem: Option<&Modem>, params: &HashMap<String, String>) -> Result<Spec> {
    let id = modem.map(|m| m.id);
    let label = match kind {
        "signal.refresh" => "刷新信号".to_string(),
        "network.refresh" => "刷新网络信息".to_string(),
        "cells.scan" => "扫描基站".to_string(),
        "lock.status" => "读取锁网状态".to_string(),
        "at" => format!("AT：{}", params.get("cmd").cloned().unwrap_or_default()),
        "ussd.send" => format!("USSD：{}", params.get("code").cloned().unwrap_or_default()),
        "sms.send" => format!("发送短信给 {}", params.get("phone").cloned().unwrap_or_default()),
        "usbnet.set" => format!("切换 USB 网卡模式 → {}", params.get("value").cloned().unwrap_or_default()),
        "lock.band" => format!(
            "锁频段：{} {}",
            params.get("tech").cloned().unwrap_or_default(),
            params.get("bands").cloned().unwrap_or_default()
        ),
        "netscan" => "主动扫网".to_string(),
        "reboot" => "重启模组".to_string(),
        other => other.to_string(),
    };
    // 去重键：读类按种类即可；写类要把参数带上（AT 两条不同指令不能算同一个）
    let extra = match kind {
        "at" | "lock.band" | "ussd.send" | "sms.send" | "usbnet.set" | "netscan" => {
            let mut kv: Vec<String> = params
                .iter()
                .filter(|(k, _)| k.as_str() != "modem_id" && k.as_str() != "kind")
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            kv.sort();
            kv.join("&")
        }
        _ => String::new(),
    };
    Ok(Spec::new(leak_kind(kind)?, id, label).with_key(&extra))
}

/// `Spec.kind` 是 `&'static str`：把已知种类映射回静态串（**不接受任意输入**）。
fn leak_kind(kind: &str) -> Result<&'static str> {
    Ok(match kind {
        "signal.refresh" => "signal.refresh",
        "network.refresh" => "network.refresh",
        "cells.scan" => "cells.scan",
        "lock.status" => "lock.status",
        "at" => "at",
        "ussd.send" => "ussd.send",
        "sms.send" => "sms.send",
        "usbnet.set" => "usbnet.set",
        "lock.band" => "lock.band",
        "netscan" => "netscan",
        "reboot" => "reboot",
        other => return Err(msg(format!("未知任务种类：{other}"))),
    })
}

/// AT 参数校验（与 `/api/at` 同口径：单条、长度受限）。
fn at_command(params: &HashMap<String, String>) -> Result<String> {
    let cmd = params
        .get("cmd")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if cmd.is_empty() {
        return Err(msg("cmd required"));
    }
    if cmd.len() > MAX_AT_LEN || cmd.contains('\r') || cmd.contains('\n') {
        return Err(msg("cmd 必须是单行指令（≤128 字符）"));
    }
    Ok(cmd)
}

fn parse_bands(s: &str) -> Vec<i32> {
    s.trim()
        .trim_matches(|c| c == '[' || c == ']')
        .split([':', ',', ' '])
        .filter_map(|p| p.trim().parse::<i32>().ok())
        .filter(|b| *b > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_rejects_multiline_and_long() {
        let mut p = HashMap::new();
        p.insert("cmd".to_string(), "AT+CSQ\nAT+CFUN=0".to_string());
        assert!(at_command(&p).is_err());
        p.insert("cmd".to_string(), "A".repeat(MAX_AT_LEN + 1));
        assert!(at_command(&p).is_err());
        p.insert("cmd".to_string(), " AT+CSQ ".to_string());
        assert_eq!(at_command(&p).unwrap(), "AT+CSQ");
    }

    #[test]
    fn bands_accept_json_and_colon() {
        assert_eq!(parse_bands("[78,3]"), vec![78, 3]);
        assert_eq!(parse_bands("78:3"), vec![78, 3]);
        assert!(parse_bands("").is_empty());
    }

    #[test]
    fn unknown_kind_is_rejected_before_spawning() {
        assert!(leak_kind("rm -rf /").is_err());
        assert_eq!(leak_kind("cells.scan").unwrap(), "cells.scan");
    }

    #[test]
    fn spec_key_differs_by_at_command() {
        let mut a = HashMap::new();
        a.insert("modem_id".to_string(), "0".to_string());
        a.insert("cmd".to_string(), "AT+CSQ".to_string());
        let mut b = a.clone();
        b.insert("cmd".to_string(), "AT+CIMI".to_string());
        let ka = build_spec("at", None, &a).unwrap().key;
        let kb = build_spec("at", None, &b).unwrap().key;
        assert_ne!(ka, kb);
    }
}
