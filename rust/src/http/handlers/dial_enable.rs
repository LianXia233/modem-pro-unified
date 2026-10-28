//! `POST /api/dial/enable` —— 「拨号上网」最上面的拨号总开关（`{on}`）。
//!
//! 这是个**策略开关**（不是给模组下 AT）：写回 `config.toml` 的 `[net] dial_enabled`，
//! 并同步一份运行期值。关掉后**所有拨号方式都不拨号**：
//!   · 自动组网循环  整台模组跳过（不激活 PDP、不建 WAN 接口）；
//!   · 手动拨号      直接拒（见 model/dial.rs 的 `Modem::dial` 入口）。
//! 已经建过的 WAN 接口不动（只是拿不到地址），要拆就自己去网络设置里删。
//!
//! `on` 用「必须显式给值」的口径（不用指针）：body 里没写就不能把拨号误关。

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
        "dial_enabled",
        if on { "true" } else { "false" },
    );
    if let Err(e) = &written {
        log::warn!("写回 [net] dial_enabled 失败: {e}");
    }
    *state.net_dial_enabled.lock().unwrap() = Some(on);
    // 重新打开：清退避，让统一调度下一轮（≤15 秒）立刻尝试，而不是等旧窗口走完。
    if on {
        crate::runtime::dialer::supervisor().reset_backoff_all();
    }

    let msg = if on {
        "已开启拨号（[net] dial_enabled 已写回配置；自动组网 ≤15 秒按新值走）"
    } else {
        // 关掉时组网循环的拨号器托管会立刻把 QMI/MBIM 的会话收起
        // （见 runtime/dialer 的 `stop_reason`「拨号总开关关闭」），不用在这里亲自动手；
        // AT 路径也不再自动重拨（统一判定在 `auto_connect_allowed`）。
        "已关闭拨号（[net] dial_enabled 已写回配置）：所有拨号方式都不再拨号，主机侧拨号器也会被收起；要恢复打开这个开关即可"
    };
    let data = serde_json::json!({ "dial_enabled": state.dial_enabled() });
    response::ok_msg(req, data, msg)
}
