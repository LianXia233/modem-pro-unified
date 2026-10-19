//! `POST /api/roam/set` —— 「数据漫游」开关（`{on}`）。
//!
//! 这是个**策略开关**（不是给模组下 AT）：写回 `config.toml` 的 `[net] allow_roaming`，
//! 并同步一份运行期值，因此不用重启就生效；界面据此判断国际漫游时要不要停数据。

use std::collections::HashMap;

use tiny_http::Request;

use crate::config::patch;
use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    // ⚠️ 这个开关**不能**用「缺省 true」：漏传 `on` 会被解释成「开启数据漫游」，
    // 在国际漫游下意味着悄悄放行流量。必须显式给值，给了但认不出也要报错。
    let on = match params
        .get("on")
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("1") | Some("true") | Some("on") | Some("yes") | Some("y") => true,
        Some("0") | Some("false") | Some("off") | Some("no") | Some("n") => false,
        Some(other) => {
            return response::fail(
                req,
                -1,
                &format!("on 参数无法识别：{other}（用 true / false）"),
            )
        }
        None => return response::fail(req, -1, "缺少 on 参数（true / false）"),
    };

    // 先写配置（失败也要把开关状态如实回给界面，不能假装成功）
    let written = patch::set_raw(
        &state.config_path,
        "net",
        "allow_roaming",
        if on { "true" } else { "false" },
    );
    if let Err(e) = &written {
        log::warn!("写回 [net] allow_roaming 失败: {e}");
    }
    *state.net_allow_roaming.lock().unwrap() = Some(on);

    response::ok(req, serde_json::to_value(modem.roam_view(on))?)
}
