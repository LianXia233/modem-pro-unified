//! `POST /api/dial/auto` —— 自动拨号开关（`{on}`）。
//!
//! 写回 `config.toml` 的 `[net] auto_dial`（fm350 同款语义、默认 true），并同步
//! 运行期值。开着时拨号协调器（`runtime/dialer` 的 `reconcile`，随组网循环
//! ≤15 秒一轮）统一巡检：
//! PDP 掉了 / 开机没拨就自动重拨；用户手动断开（manual_stop）后不再打扰。
//!
//! `on` 必须显式给值（同 `/api/dial/enable` 的口径，防误关）。

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

    let written = patch::set_raw(
        &state.config_path,
        "net",
        "auto_dial",
        if on { "true" } else { "false" },
    );
    if let Err(e) = &written {
        log::warn!("写回 [net] auto_dial 失败: {e}");
    }
    *state.net_auto_dial.lock().unwrap() = Some(on);
    // 刚打开自动拨号：清掉所有模组的退避窗口，别让人等完一个已经无意义的 5 分钟。
    // （调度由拨号协调器统一负责，见 runtime/dialer 的 `reconcile`。）
    if on {
        crate::runtime::dialer::supervisor().reset_backoff_all();
    }

    let msg = if on {
        "已开启自动拨号（[net] auto_dial 已写回配置；守护循环 30 秒内接管）"
    } else {
        "已关闭自动拨号（手动断开后不会再被自动重拨打扰）"
    };
    if written.is_err() {
        return response::fail(req, -1, &format!("{msg}（写回配置失败，重启后回落原值）"));
    }
    response::ok_msg(req, serde_json::json!({ "auto_dial": on }), msg)
}
