//! `GET/POST /api/atport/bind` —— 按 USB 设备身份（`vid:pid`）绑定 AT 命令口。
//!
//!   · GET  列出全部绑定（含「这个口现在还在不在」）+ 本机串口
//!   · POST `{"vid":"0e8d","pid":"7126","port":"/dev/ttyUSB2"}`
//!     （port 传空串 = 解绑，恢复自动扫描）
//!
//! 绑上之后发现流程**直接用这个口当 AT 口**，不再扫描/探测哪个口是命令口：
//! 自动探测只认「能答 AT 的 OK」，挑错口时指令一条都跑不出去（IMEI/型号也读不出），
//! 按「模组身份」指定那套也救不了 —— 只能按 **设备身份** 钉死。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::atport_bind_supported_msg;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let ports = crate::modem::ports_list();
    let supported = cfg!(unix);

    let is_get = matches!(req.method(), tiny_http::Method::Get);
    if is_get {
        return response::ok(
            req,
            json!({
                "supported": supported,
                "path": state.atport_bind.path(),
                "ports": ports,
                "binds": bind_list(state, &ports),
            }),
        );
    }
    if !matches!(req.method(), tiny_http::Method::Post) {
        return response::fail(req, -1, "method not allowed, use GET/POST");
    }

    if !supported {
        return response::fail(req, -1, atport_bind_supported_msg());
    }

    let params = request::merge_form(query, &request::read_body(&mut req));
    let vid = params.get("vid").cloned().unwrap_or_default();
    let pid = params.get("pid").cloned().unwrap_or_default();
    let port = params.get("port").cloned().unwrap_or_default();
    let port = port.trim().to_string();

    if !port.is_empty() && !crate::modem::port_exists(&port) {
        return response::fail(req, -1, &format!("串口 {port} 不在本机串口列表里"));
    }

    let Some(key) = crate::runtime::store::at_port_bind_key(&vid, &pid) else {
        return response::fail(
            req,
            -1,
            &format!(
                "VID/PID 认不出来（要求 0e8d / 7126 这样的四位十六进制）: {}:{}",
                vid.trim(),
                pid.trim()
            ),
        );
    };

    let changed = state.atport_bind.set(&key, &port);
    if !changed {
        return response::ok_msg(req, json!({ "key": key, "port": port }), "没有变化");
    }

    // 立刻重新发现一轮：已发现的模组按新绑定切口，再扫一遍新口
    for modem in state.mgr.all() {
        state.mgr.apply_saved_at_port(&modem);
    }
    state.mgr.discover(true);

    if port.is_empty() {
        response::ok_msg(
            req,
            json!({ "key": key, "port": "" }),
            &format!("已解绑 {key}，恢复自动扫描"),
        )
    } else {
        response::ok_msg(
            req,
            json!({ "key": key, "port": port }),
            &format!("已绑定 {key} → {port}（这台设备的命令口直接用它，不再自动扫描）"),
        )
    }
}

/// 全部绑定 + 「这个口现在还在不在」（界面用来提示「绑的口现在不在」）。
fn bind_list(state: &AppState, ports: &[String]) -> Vec<serde_json::Value> {
    state
        .atport_bind
        .list()
        .into_iter()
        .map(|(key, port)| {
            let (vid, pid) = key.split_once(':').unwrap_or((key.as_str(), ""));
            json!({
                "key": key,
                "vid": vid,
                "pid": pid,
                "port": port,
                "present": ports.iter().any(|p| p == &port),
            })
        })
        .collect()
}
