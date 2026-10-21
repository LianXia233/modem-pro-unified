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

/// ⚠️ 与别的 handler 不同，这里收 `Arc<AppState>` 而不是 `&AppState`：
/// 提交出去的任务闭包要 `'static`（它在工作线程里跑，可能比这次请求活得久），
/// 所以必须能把整个 `AppState` 安全地搬进去。调用方传 `state.clone()`。
pub fn handle(req: Request, state: Arc<AppState>, query: &HashMap<String, String>) -> Result<()> {
    match *req.method() {
        Method::Get => handle_get(req, &state, query),
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

fn handle_post(
    mut req: Request,
    state: Arc<AppState>,
    query: &HashMap<String, String>,
) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));

    // 取消：/api/tasks/cancel {id}；也兼容「只有 id 没有 kind」的 POST（页面手滑也认）
    let wants_cancel = request::bool_param(&params, "cancel", false)
        || params.contains_key("cancel_id")
        || (params
            .get("id")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            && params
                .get("kind")
                .map(|v| v.trim().is_empty())
                .unwrap_or(true));
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
            if ok {
                "已请求取消"
            } else {
                "任务不存在或已结束"
            },
        );
    }

    let kind = params
        .get("kind")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if kind.is_empty() {
        return response::fail(req, -1, "kind required");
    }

    // 所有已知种类都需要模组：统一在这里解析一次，缺模组就当场拒
    // （不要到工作线程才发现，那时错误只能躺在任务结果里等人来翻）。
    //
    // ⚠️ 未知种类也必须在这里用「正常响应」拒绝：`?` 会把错误抛给网关层，
    // 连接上什么都不会写（页面只见空回复，排查不到原因）。所以下面每个可能
    // 失败的调用都用 `match ... Err(e) => return response::fail(...)`，不用 `?`。
    let modem = match modem_by_query(&state, &params) {
        Some(m) => m,
        None => return response::fail(req, -1, "modem not found"),
    };
    let spec = match build_spec(&kind, Some(&modem), &params) {
        Ok(s) => s,
        Err(e) => return response::fail(req, -1, &e.to_string()),
    };
    let tasks = &state.tasks;
    let cache = state.cache.clone();
    let cmd_timeout = state.cfg.serial.command_timeout();
    let allow_roaming = state.allow_roaming();

    let task = match kind.as_str() {
        // ---- 读取类（结果进缓存，供 /api/snapshot 立即读到）----
        "signal.refresh" => {
            let m = modem.clone();
            tasks.submit(spec, move |_t| {
                let v = m.signal()?;
                let j = serde_json::to_value(&v)?;
                cache.put("signal", m.id, j.clone());
                Ok(j)
            })
        }
        "modem.refresh" => {
            let m = modem.clone();
            tasks.submit(spec, move |_t| {
                let v = m.modem_status(allow_roaming)?;
                let j = serde_json::to_value(&v)?;
                cache.put("modem", m.id, j.clone());
                Ok(j)
            })
        }
        "network.refresh" => {
            let m = modem.clone();
            tasks.submit(spec, move |_t| {
                let v = m.network_status(allow_roaming);
                let j = serde_json::to_value(&v)?;
                cache.put("network", m.id, j.clone());
                Ok(j)
            })
        }
        "cells.scan" => {
            let m = modem.clone();
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
            let m = modem.clone();
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
            let m = modem.clone();
            let cmd = at_command(&params)?;
            tasks.submit(spec, move |t| {
                t.progress(format!("等待模组应答：{cmd}"));
                let cancelled = t.clone();
                // 走任务队列的手动 AT：同样是**用户操作**，别让后台采样把它压住
                let out = m.with_at_prio(crate::modem::at::Prio::User, |p| {
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
            let m = modem.clone();
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
            let m = modem.clone();
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
            let m = modem.clone();
            let value = request::i64_param(&params, "value", -1) as i32;
            tasks.submit(spec, move |_t| {
                let st = m.set_usbnet(value)?;
                Ok(serde_json::to_value(st)?)
            })
        }
        "lock.band" => {
            let m = modem.clone();
            let (tech, bands, unlock) = match super::band_lock_params(&params) {
                Ok(v) => v,
                Err(e) => return response::fail(req, -1, &e.to_string()),
            };
            tasks.submit(spec, move |t| {
                t.progress("正在写入频段锁定（会重启射频）");
                let v = m.set_band_lock(&tech, &bands, unlock)?;
                let j = serde_json::to_value(&v)?;
                cache.put("lock", m.id, j.clone());
                Ok(j)
            })
        }
        "netscan" => {
            let m = modem.clone();
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
            let m = modem.clone();
            tasks.submit(spec, move |t| {
                t.progress("已下发重启，模组会掉线约 20 秒");
                let r = m.reboot()?;
                // 与同步接口同口径：重启断链 → 状态拉回 dialing（不是用户断开），
                // 清退避让模组回来后立刻可重拨。
                m.note_external_break("模组重启中，连接将重建");
                crate::runtime::dialer::supervisor().reset_backoff(m.id);
                Ok(serde_json::to_value(r)?)
            })
        }
        // ---- 拨号域：一次拨号要连发好几条 AT、还可能起主机侧拨号器 ----
        // 同步 POST 会撞上 LuCI ucode 代理的 20 秒超时，所以页面走任务。
        "dial.connect" => {
            let m = modem.clone();
            let st = state.clone();
            let mut dial_req = super::dial_connect::dial_request_from_params(&params);
            // 表单三项全空时走运营商感知的自动 APN（与同步接口同口径）
            super::dial_connect::apply_auto_apn(&m, &mut dial_req);
            let enabled = state.dial_enabled();
            let ims = state.cfg.dial.activate_ims;
            tasks.submit(spec, move |t| {
                t.progress("正在拨号（QMI/MBIM 会起主机侧拨号器，约 10~30 秒）");
                let out = m.dial(&dial_req, enabled, ims);
                // QMI/MBIM 只是「交办」：会话由拨号协调器建。立刻踢一轮，
                // 别让用户等下一次 15 秒节拍（AT 路径此时已连上，踢一轮是空操作）。
                if out.is_ok() {
                    crate::runtime::dialer::supervisor().kick();
                }
                // 无论成败都把「卡在哪一步」的状态归档 + 广播，页面据此刷新
                let st_now = m.dial_status();
                let j = serde_json::to_value(&st_now)?;
                cache.put("dial", m.id, j);
                super::dial_connect::push_dial(&st, &st_now);
                super::dial_connect::push_network(&st, &m);
                out.and_then(|s| Ok(serde_json::to_value(s)?))
            })
        }
        "dial.disconnect" => {
            let m = modem.clone();
            let st = state.clone();
            let cid = request::i64_param(&params, "cid", 0) as i32;
            tasks.submit(spec, move |t| {
                t.progress("正在断开数据连接");
                // 顺序要紧（与同步接口同口径）：先停拨号器，再让模组侧收尾，
                // 否则模组侧先 `AT+CGACT=0`，拨号器下一轮发现自己重拨，两边会抢。
                crate::runtime::dialer::supervisor().stop_modem(m.id);
                let out = m.hangup(cid);
                let st_now = m.dial_status();
                let j = serde_json::to_value(&st_now)?;
                cache.put("dial", m.id, j);
                super::dial_connect::push_dial(&st, &st_now);
                out.and_then(|s| Ok(serde_json::to_value(s)?))
            })
        }
        // ---- 串口慢写：IMEI 改写（含备份快照 + 回读确认）与短信存储区切换 ----
        "imei.set" => {
            let m = modem.clone();
            let st = state.clone();
            let imei = params
                .get("imei")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if imei.is_empty() {
                return response::fail(req, -1, "imei required");
            }
            let backup = state.imei.get(modem.id);
            tasks.submit(spec, move |t| {
                t.progress("正在改写 IMEI（先备份原值，再写入并回读确认）");
                let (_, warning, bak) = m.set_imei(&imei, backup)?;
                // 改写成功才落备份：失败时台账保持原样（可能还是空）
                st.imei.put(m.id, bak.clone());
                let status = m.imei_status(Some(&bak), warning.as_deref());
                Ok(serde_json::to_value(status)?)
            })
        }
        "imei.restore" => {
            let m = modem.clone();
            let st = state.clone();
            let backup = state.imei.get(modem.id);
            tasks.submit(spec, move |t| {
                t.progress("正在恢复出厂原始 IMEI（同样需重启模组后复查）");
                let (_, warning, bak) = m.restore_imei(backup)?;
                st.imei.put(m.id, bak.clone());
                let status = m.imei_status(Some(&bak), warning.as_deref());
                Ok(serde_json::to_value(status)?)
            })
        }
        "sms.storage.set" => {
            let m = modem.clone();
            let st = state.clone();
            let mem = params.get("mem").cloned().unwrap_or_default();
            if mem.trim().is_empty() {
                return response::fail(req, -1, "mem required（SM / ME / MT）");
            }
            tasks.submit(spec, move |t| {
                t.progress(format!("正在切换到存储区 {mem}"));
                let out = m.set_sms_memory(&mem)?;
                // 写回配置：切完重启也能沿用（写失败只记日志，不影响本次结果 ——
                // 与同步接口 `sms_storage_set` 完全同口径）
                if let Err(e) = crate::config::patch::set_string(
                    &st.config_path,
                    "sms",
                    "sms_memory",
                    out.current.as_str(),
                ) {
                    log::warn!("写回 [sms] sms_memory 失败: {e}");
                }
                log::info!("模组 {} 短信存储区切到 {}", m.id, out.current);
                Ok(serde_json::to_value(out)?)
            })
        }
        other => {
            return response::fail(req, -1, &format!("未知任务种类：{other}"));
        }
    };

    response::ok_msg(
        req,
        json!({ "task": task.view() }),
        if task.is_active() {
            "已提交"
        } else {
            "任务已结束"
        },
    )
}

/// 组装任务描述：标签给人看，去重键把参数也算进去。
fn build_spec(kind: &str, modem: Option<&Modem>, params: &HashMap<String, String>) -> Result<Spec> {
    let id = modem.map(|m| m.id);
    let label = match kind {
        "signal.refresh" => "刷新信号".to_string(),
        "modem.refresh" => "刷新 SIM 与模组信息".to_string(),
        "network.refresh" => "刷新网络信息".to_string(),
        "cells.scan" => "扫描基站".to_string(),
        "lock.status" => "读取锁网状态".to_string(),
        "at" => format!("AT：{}", params.get("cmd").cloned().unwrap_or_default()),
        "ussd.send" => format!("USSD：{}", params.get("code").cloned().unwrap_or_default()),
        "sms.send" => format!(
            "发送短信给 {}",
            params.get("phone").cloned().unwrap_or_default()
        ),
        "usbnet.set" => format!(
            "切换 USB 网卡模式 → {}",
            params.get("value").cloned().unwrap_or_default()
        ),
        "lock.band" => format!(
            "锁频段：{} {}",
            params.get("tech").cloned().unwrap_or_default(),
            params.get("bands").cloned().unwrap_or_default()
        ),
        "netscan" => "主动扫网".to_string(),
        "reboot" => "重启模组".to_string(),
        "dial.connect" => "拨号".to_string(),
        "dial.disconnect" => "断开数据连接".to_string(),
        "imei.set" => format!(
            "改写 IMEI → {}",
            params.get("imei").cloned().unwrap_or_default()
        ),
        "imei.restore" => "恢复出厂原始 IMEI".to_string(),
        "sms.storage.set" => format!(
            "切换短信存储区 → {}",
            params.get("mem").cloned().unwrap_or_default()
        ),
        other => other.to_string(),
    };
    // 去重键：读类按种类即可；写类要把参数带上（AT 两条不同指令不能算同一个）
    let extra = match kind {
        "at" | "lock.band" | "ussd.send" | "sms.send" | "usbnet.set" | "netscan" | "imei.set"
        | "imei.restore" | "sms.storage.set" | "dial.connect" | "dial.disconnect" => {
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
        "modem.refresh" => "modem.refresh",
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
        "dial.connect" => "dial.connect",
        "dial.disconnect" => "dial.disconnect",
        "imei.set" => "imei.set",
        "imei.restore" => "imei.restore",
        "sms.storage.set" => "sms.storage.set",
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
