//! `GET /api/modules` —— 支持模块清单。

use tiny_http::Request;

use crate::error::Result;
use crate::http::response;
use crate::modem::catalog;

pub fn handle(req: Request) -> Result<()> {
    response::ok(req, serde_json::to_value(catalog::builtin())?)
}
