//! `GET /api/dial?modem_id=N[&force=1]` —— 拨号状态 + USB 类型识别 + 可选类型表。
//!
//! 页面进「拨号上网」先 GET 一次（状态 + 类型 + 表，一次拉全，不用发三次请求）。
//! `force=1` 表示重新识别 USB 类型（不走 30 秒缓存）—— 页面上「重新识别」按钮用它。
//!
//! 字段：
//!   `modem_id / state / usb / class_notes / types / auto_wan / wan_ipv6 /
//!    wan_ipv6_mode / wan_ipv6_modes / dial_enabled / wan_mode / wan_mode_label /
//!    wan_modes / wan_dns`。
//! `auto_wan`（自动组网最近一次结果）由后台 netprov 循环每 15 秒写一次；
//! 从未跑过就是 null，见 `netprov/provision.rs`。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let force = matches!(
        query.get("force").map(|s| s.as_str()),
        Some("1") | Some("true")
    );
    let usb = modem.usb_type(force);
    // 接口类码翻成人话给页面直接用（原生类码放在 usb.descriptor.classes 里备查）
    let class_notes = usb
        .descriptor
        .as_ref()
        .map(|d| crate::modem::model::dial::class_summary_list(&d.classes))
        .unwrap_or_default();
    let mode = state.wan_mode();
    // auto_wan：自动组网最近一次结果（由 netprov 循环每 15 秒写一次，见 netprov/）
    let auto_wan = state
        .netprov
        .get(modem.id)
        .map(|r| serde_json::to_value(&r).unwrap_or(serde_json::Value::Null))
        .unwrap_or(serde_json::Value::Null);
    let data = serde_json::json!({
        "modem_id": modem.id,
        "state": modem.dial_status(),
        "usb": usb,
        "class_notes": class_notes,
        "types": crate::modem::model::dial::dial_types(),
        "auto_wan": auto_wan,
        "wan_ipv6": state.wan_ipv6(),
        "wan_ipv6_mode": if state.wan_ipv6() {
            crate::netprov::v6::V6Mode::parse(&state.cfg.net.wan_ipv6_mode).as_str()
        } else {
            "off"
        },
        "wan_ipv6_modes": crate::netprov::v6::v6_modes_payload(),
        "dial_enabled": state.dial_enabled(),
        "auto_dial": state.auto_dial(),
        "wan_mode": mode.as_str(),
        "wan_mode_label": mode.label(),
        "wan_modes": crate::config::WanMode::list_payload(),
        // 组网实际会写进 network.modem<id>.dns 的那串（`[net] wan_dns` 经
        // IPv6 开关裁剪后的结果；空串 = 不强制下发，用运营商/DHCP 给的）。
        // 页面拿它显示「接口下发 DNS」，排障时不用去翻 config.toml。
        "wan_dns": crate::netprov::effective_wan_dns(
            &state.cfg.net.wan_dns,
            state.wan_ipv6(),
        ),
        // 主机侧拨号器当前后端（external = 厂商二进制，native-qmi = Rust 原生）
        "host_dialer": crate::runtime::dialer::supervisor().describe(modem.id),
    });
    response::ok(req, data)
}
