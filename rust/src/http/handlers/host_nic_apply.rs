//! `POST /api/host/nic/apply` —— 生成一组主机网卡命令（可选直接执行）。
//!
//! 请求体 `{modem_id, iface, ip, mask, gateway, dns[], mode, execute}`；
//! `mode` 三选一：`renew`（先试 DHCP）/ `static`（配静态）/ `dhcp`（恢复自动获取）。
//! `execute` 默认 false：命令只生成出来复制，勾了才由本程序下发（要 root / 管理员）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{hostnic, request, response, AppState};
use crate::runtime::hostnic::{HostNicPlan, MODE_DHCP, MODE_RENEW, MODE_STATIC};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let mode = params.get("mode").cloned().unwrap_or_default();
    if !matches!(mode.as_str(), MODE_RENEW | MODE_STATIC | MODE_DHCP) {
        return response::fail(req, -1, "mode 只认 renew / static / dhcp");
    }
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
    let execute = request::bool_param(&params, "execute", false);
    let apply = hostnic::apply(state, &modem, &plan, &mode, execute);
    // 执行完地址可能变了：视图取的是执行后的状态
    let data = serde_json::json!({
        "view": hostnic::view(state, &modem),
        "apply": apply,
    });
    response::ok(req, data)
}
