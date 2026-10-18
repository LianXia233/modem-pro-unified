//! `POST /api/host/nic/save` —— 保存一张网卡的手工配置（`hostnic.json`，重启不丢）。
//!
//! 请求体 `{modem_id, iface, ip, mask, gateway, dns[]}`；校验不通过直接报错不落盘。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{hostnic, request, response, AppState};
use crate::runtime::hostnic::HostNicPlan;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let plan = HostNicPlan {
        iface: params.get("iface").cloned().unwrap_or_default(),
        ip: params.get("ip").cloned().unwrap_or_default(),
        mask: params.get("mask").cloned().unwrap_or_default(),
        gateway: params.get("gateway").cloned().unwrap_or_default(),
        dns: params
            .get("dns")
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
    };
    let modem_id = params
        .get("modem_id")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let Some(modem) = state.mgr.get(modem_id) else {
        return response::fail(req, -1, "modem not found");
    };
    match state.hostnic.save(&plan) {
        Ok(saved) => response::ok_msg(
            req,
            serde_json::to_value(hostnic::view(state, &modem))?,
            &format!("ok, 已保存 {} 的配置", saved.iface),
        ),
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
