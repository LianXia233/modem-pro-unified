//! `GET /api/host/nic?modem_id=N` —— 主机侧网卡配置视图（含建议值与三组命令）。
//!
//! 面板在前端已下线（350 系列之外的机型不需要），但接口按迁移清单保留：
//! 这是一份"配置 + 可复制命令"，命令默认**只生成不执行**（改网卡要 root/管理员）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{hostnic, response, AppState};

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    response::ok(req, serde_json::to_value(hostnic::view(state, &modem))?)
}
