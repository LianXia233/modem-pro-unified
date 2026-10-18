//! `POST /api/ntp/set` —— 设置时区（`[ntp] timezone`），运行期立即生效。
//!
//! 接受 IANA 名（`Asia/Shanghai`）或 POSIX 写法（`CST-8`）；解析成功就
//! 全局生效（`logging::time::set_configured_tz`，所有本地时间立刻切换），
//! 再原地写回 config.toml（保留注释，见 config/patch.rs）。
//!
//! 边界：只动 `timezone` 一行；服务器列表与总开关仍走配置文件（改完重启生效），
//! 页面上不提供运行期改它们的入口 —— 半套配置两处真相最容易出事。

use std::collections::HashMap;

use tiny_http::Request;

use crate::config::patch;
use crate::error::Result;
use crate::http::{request, response, AppState};

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let tz = params.get("timezone").map(|s| s.trim()).unwrap_or("");
    if tz.is_empty() {
        return response::fail(req, -1, "缺少 timezone（如 Asia/Shanghai 或 CST-8）");
    }
    // 先改运行期（失败不动现有时区），再写配置（写失败只记日志，照 roam/set 先例）
    let off = match crate::runtime::ntp::set_timezone(tz) {
        Ok(off) => off,
        Err(e) => return response::fail(req, -1, &e),
    };
    if let Err(e) = patch::set_string(&state.config_path, "ntp", "timezone", tz) {
        log::warn!("写回 [ntp] timezone 失败: {e}");
    }

    let data = serde_json::json!({
        "timezone": tz,
        "offset_secs": off,
        "offset_label": format!("UTC{:+}", off / 3600),
    });
    response::ok_msg(
        req,
        data,
        &format!("时区已设为 {tz}（UTC{:+}，立即生效）", off / 3600),
    )
}
