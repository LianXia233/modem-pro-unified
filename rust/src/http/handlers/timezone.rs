//! `/api/timezone` —— 系统时区（读 / 设）。
//!
//! ## 与 `/api/ntp/set` 的分工（两个不同的东西，别混）
//!
//! | 接口 | 管什么 | 落在哪 |
//! | --- | --- | --- |
//! | `GET/POST /api/timezone` | **系统时区**（路由器本身的时间） | `/etc/config/system` 的 `zonename`+`timezone`、`/etc/TZ` |
//! | `POST /api/ntp/set` | **modemd 自己的**时区（日志/定时任务用它判「几点」） | `/etc/modemd/config.toml` 的 `[ntp] timezone` |
//!
//! 界面上应该只给一个入口（本接口）：用户改的是「路由器时区」，
//! 改完顺便把 modemd 的配置也同步过去，否则会出现「系统 +9、日志还是 +8」。
//!
//! 具体写盘与回读验证见 `runtime/tz.rs`。

use std::collections::HashMap;

use tiny_http::Request;

use crate::config::patch;
use crate::error::Result;
use crate::http::{request, response, AppState};
use crate::runtime::tz;

/// `GET /api/timezone` —— 当前时区 + 预置列表。
pub fn get(req: Request, _state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    let cur = tz::current();
    response::json_body(
        req,
        serde_json::json!({
            "current": cur,
            "presets": tz::presets_payload(),
        }),
    )
}

/// `POST /api/timezone` —— `{zonename: "Asia/Tokyo"}`。
pub fn set(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let name = params.get("zonename").map(|s| s.trim()).unwrap_or("");
    if name.is_empty() {
        return response::fail(
            req,
            -1,
            "缺少 zonename（如 Asia/Shanghai；可选值见 GET /api/timezone 的 presets）",
        );
    }
    // ① 系统时区（uci + /etc/TZ），内部含回读验证
    let after = match tz::set(name) {
        Ok(v) => v,
        Err(e) => return response::fail(req, -1, &e.to_string()),
    };

    // ② 把 modemd 自己的时区也同步过去，避免「系统 +9、日志 +8」。
    //    写失败只记日志：系统时区已经生效，不该因为一次 TOML 写回失败就报整体失败。
    if let Err(e) = patch::set_string(&state.config_path, "ntp", "timezone", name) {
        log::warn!("同步写回 [ntp] timezone 失败: {e}");
    }
    if let Ok(off) = crate::runtime::ntp::set_timezone(name) {
        log::info!("时区已切换到 {name}（UTC{:+}）", off / 3600);
    }

    response::ok_msg(
        req,
        serde_json::json!({
            "current": after,
            "presets": tz::presets_payload(),
        }),
        &format!(
            "时区已设为 {}（{}，回读校验通过）",
            name, after.offset_label
        ),
    )
}
