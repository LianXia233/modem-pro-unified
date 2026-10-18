//! `GET /api/version` —— 版本 + 运行时信息（免登录）。

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::version;

pub fn handle(req: Request, state: &AppState) -> Result<()> {
    let info = version::current(state.cfg.require_luci_login());
    response::ok(req, serde_json::to_value(info)?)
}
