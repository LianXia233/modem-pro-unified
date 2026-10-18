//! `POST /api/imei/set` —— 改写模组 IMEI（仅移远/美格等高通系、广和通 MTK 平台）。
//!
//! 请求体：`{modem_id, imei}`（15 位数字）。
//! 流程：校验格式 → 第一次改写前先用**同族只读命令**快照备份当前值到 `imei.json`
//! （广和通 MTK 走 `AT+EGMREXT=0,7`，其余走 `AT+CGSN`；明确应答空值算备份成功）
//! → 下发写入 → 回读确认。广和通 MTK 平台写入需重启模组后才生效，回读对不上时
//! 不判失败，而是在响应里带 `warning` 提示重启后复查。
//! 备份落盘失败不阻断改写（台账已尽力，界面会显示没备份）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let Some(imei) = params.get("imei").map(|s| s.trim().to_string()) else {
        return response::fail(req, -1, "缺少 imei 参数");
    };

    let backup = state.imei.get(modem.id);
    match modem.set_imei(&imei, backup) {
        Ok((_, warning, bak)) => {
            // 改写成功才落备份：失败时台账保持原样（可能还是空）
            state.imei.put(modem.id, bak.clone());
            let status = modem.imei_status(Some(&bak), warning.as_deref());
            response::ok(req, serde_json::to_value(status)?)
        }
        Err(e) => response::fail(req, -1, &e.to_string()),
    }
}
