//! 监听入口：转交同端口网关（HTTP 透传 + WebSocket）。

use std::sync::Arc;

use crate::error::Result;

use super::gateway;
use super::AppState;

/// 绑定监听地址并进入请求循环（阻塞）。
pub fn run(state: Arc<AppState>) -> Result<()> {
    gateway::run(state.cfg.server.listen.clone(), state)
}
