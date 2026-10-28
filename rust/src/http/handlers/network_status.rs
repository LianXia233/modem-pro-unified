//! `GET /api/network/status?modem_id=N` —— 网络状态总览。
//!
//! 查询结果顺手喂给波动记录（见 `modem/metrics.rs`），不额外发 AT。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};

use super::{modem_by_query, push_changes};

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let Some(modem) = modem_by_query(state, query) else {
        return response::fail(req, -1, "modem not found");
    };
    let status = modem.network_status(state.cfg.net.allow_roaming);
    push_changes(
        state,
        modem.id,
        &state.metrics.note_network(modem.id, &status),
    );
    let mut value = match response::to_value(&status) {
        Ok(j) => j,
        Err(e) => return response::fail(req, -1, &e),
    };
    // `traffic` 统一换成 **sysfs 口径**（`runtime/ifstat.rs`）。
    //
    // 原来的 `traffic` 来自模组 AT 计数器：FM350-GL 的 `AT+GTSTATIS` 回 ERROR，
    // 于是 `supported=false`、速率与累计恒为 0，页面四个指标全空。
    // 现在改成「网卡实际字节数」，与 `/api/traffic` 完全同源 ——
    // 两个接口看到的是同一份数字。
    if let Ok(t) = serde_json::to_value(state.ifstat.view()) {
        value["traffic"] = t;
    }
    // 进缓存：快照页免发 AT（network_status 是一次串口会话，代价不小）
    state.cache.put("network", modem.id, value.clone());
    response::ok(req, value)
}
