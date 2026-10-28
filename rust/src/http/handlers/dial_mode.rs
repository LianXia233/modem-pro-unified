//! `POST /api/dial/mode` —— 「拨号上网」页面上的接口模式下拉（`{mode}`）。
//!
//! 三种模式（`[net] wan_mode`）：
//!   · `modem_pro` —— 只用一张接口（proto=modem_pro），IPv4+IPv6 都在上面。**默认**
//!   · `dhcp`      —— 传统两张：modem<id>(dhcp) + modem<id>_6(dhcpv6)
//!   · `both`      —— 两种都要：modem<id>(modem_pro) + modem<id>_6(dhcpv6)
//!
//! 写回配置后由组网循环（≤15 秒一轮）接手：会就地改主接口 proto、按需补/删
//! modem<id>_6，防火墙那段也跟着对账（见 netprov/）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::config::patch;
use crate::config::WanMode;
use crate::error::Result;
use crate::http::{request, response, AppState};

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let raw = params.get("mode").map(|s| s.trim()).unwrap_or("");
    if raw.is_empty() {
        return response::fail(req, -1, "缺少 mode（modem_pro / dhcp / both）");
    }
    let mode = WanMode::parse(raw);

    // 先写配置（失败也要把开关状态如实回给界面；照 roam/set 先例）。
    // ⚠️ 是字符串值，必须走 set_string：set_raw 原样写会把引号写丢，
    // 配置变成非法 TOML，重启时就再也起不来了（真机踩坑）。
    let written = patch::set_string(&state.config_path, "net", "wan_mode", mode.as_str());
    if let Err(e) = &written {
        log::warn!("写回 [net] wan_mode 失败: {e}");
    }
    *state.net_wan_mode.lock().unwrap() = Some(mode.as_str().to_string());

    let data = serde_json::json!({
        "wan_mode": mode.as_str(),
        "wan_mode_label": mode.label(),
        "wan_modes": WanMode::list_payload(),
    });
    response::ok_msg(
        req,
        data,
        &format!(
            "已切到「{}」（[net] wan_mode 已写回配置，组网循环 ≤15 秒生效）",
            mode.label()
        ),
    )
}
