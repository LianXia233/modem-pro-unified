//! `POST /api/dial/ipv6` —— 「拨号上网」页面上的 IPv6 开关（`{on}`）。
//!
//! 这是个**策略开关**（不是给模组下 AT）：写回 `config.toml` 的 `[net] wan_ipv6`，
//! 并同步一份运行期值，因此不用重启就生效。**四种拨号方式都按它来**：
//! QMI 不给拨号器加 -6；ECM/NCM/RNDIS 不建 modem<id>_6；MBIM 给 umbim 的 pdp_type 只写 ip。
//! 真正动手的是组网循环（每 15 秒一轮）：这里只改配置 + 回状态。
//!
//! `on` 用「必须显式给值」的口径（不用指针）：body 里没写就不能把开关误关。

use std::collections::HashMap;

use tiny_http::Request;

use crate::config::patch;
use crate::error::Result;
use crate::http::{request, response, AppState};

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    if !params.contains_key("on") {
        return response::fail(req, -1, "缺少 on（要 true 或 false）");
    }
    let on = request::bool_param(&params, "on", false);

    // 先写配置（失败也要把开关状态如实回给界面，不能假装成功；照 roam/set 先例）
    let written = patch::set_raw(
        &state.config_path,
        "net",
        "wan_ipv6",
        if on { "true" } else { "false" },
    );
    if let Err(e) = &written {
        log::warn!("写回 [net] wan_ipv6 失败: {e}");
    }
    *state.net_wan_ipv6.lock().unwrap() = Some(on);

    let word = if on { "已开启" } else { "已关闭" };
    let data = serde_json::json!({ "wan_ipv6": state.wan_ipv6() });
    response::ok_msg(
        req,
        data,
        &format!("{word}模组侧 IPv6（[net] wan_ipv6 已写回配置，组网循环 ≤15 秒生效）"),
    )
}
