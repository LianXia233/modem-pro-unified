//! `POST /api/imei/restore` —— 一键恢复备份的原始 IMEI。
//!
//! 请求体：`{modem_id}`。没有备份（这台模组从未被本程序改写过）时拒绝执行；
//! 备份的原始值为空（模组出厂即无 IMEI）时同样拒绝 —— 把 IMEI 写成空没有意义。
//! 广和通 MTK 平台恢复后同样需重启模组才生效，响应里会带 `warning`。

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

    let backup = state.imei.get(modem.id);
    super::imei_done(req, state, &modem, modem.restore_imei(backup))
}
