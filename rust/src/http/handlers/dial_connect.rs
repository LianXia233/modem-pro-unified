//! `POST /api/dial/connect` —— 拨号（`{modem_id,cid,apn,pdp_type,reconnect,force}`）。
//!
//! apn 留空 = 沿用模组里已有的配置；reconnect = 已激活的先断开再连（让刚写入的 APN 生效）。
//! 失败时**仍然把状态带回去**（data = DialState），页面据此显示「卡在哪一步」，
//! 而不是只弹一句错误；错误信息走 msg。
//!
//! 契约见 docs/API.md。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::modem_by_query;
use crate::modem::model::{DialRequest, DialState};

/// 从请求参数构造 `DialRequest`（`cid/apn/pdp_type/reconnect/force`，缺省值同契约）。
///
/// 同步 `/api/dial/connect` 与异步任务 `dial.connect` 共用——两处口径必须一致，
/// 否则同一份表单走两条路径会得到不同行为（比如某处默认值改了另一处没改）。
pub(crate) fn dial_request_from_params(params: &HashMap<String, String>) -> DialRequest {
    DialRequest {
        cid: request::i64_param(params, "cid", 0) as i32,
        apn: params.get("apn").cloned().unwrap_or_default(),
        pdp_type: params.get("pdp_type").cloned().unwrap_or_default(),
        reconnect: request::bool_param(params, "reconnect", false),
        force: request::bool_param(params, "force", false),
    }
}

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let dial_req = dial_request_from_params(&params);
    match modem.dial(&dial_req, state.dial_enabled(), state.cfg.dial.activate_ims) {
        Ok(st) => {
            // QMI/MBIM 只是「交办」：会话由拨号协调器建。立刻踢一轮，
            // 别让用户（或任务线程之外的调用方）等下一次 15 秒节拍。
            // AT 路径此时已经连上，踢一轮是空操作（`starting` 保证不重复建会话）。
            crate::runtime::dialer::supervisor().kick();
            push_dial(state, &st);
            push_network(state, &modem);
            response::ok_msg(
                req,
                serde_json::to_value(&st)?,
                &format!("ok, 已拨号 cid={}", st.cid),
            )
        }
        Err(e) => {
            // 状态照旧带回去（data = st），页面显示「卡在哪一步」；错误信息走 msg
            let st: DialState = modem.dial_status();
            push_dial(state, &st);
            response::fail_data(req, -1, serde_json::to_value(&st)?, &e.to_string())
        }
    }
}

/// 拨号状态推给所有页面（「拨号上网」靠它实时刷新）。
pub(crate) fn push_dial(state: &AppState, st: &DialState) {
    let data = serde_json::to_value(st).unwrap_or(serde_json::Value::Null);
    state.ws.broadcast("dial", Some(st.modem_id), data);
}

/// 拨号会改链路 / 地址，顺手把「当前网络」卡片也刷一次。
///
/// 也供异步任务 `dial.connect` / `dial.disconnect` 复用（它们在任务线程里跑，
/// 拿到的仍是同一个 `AppState`，口径必须完全一致）。
pub(crate) fn push_network(state: &AppState, modem: &crate::modem::model::Modem) {
    let st = modem.network_status(state.cfg.net.allow_roaming);
    // 拨号卡片的「在用 APN / 本机地址 / 主机网卡」与「网络」卡片是同一批数据，顺手同步
    modem.sync_dial_state(&st);
    let data = serde_json::to_value(&st).unwrap_or(serde_json::Value::Null);
    state.ws.broadcast("network", Some(modem.id), data);
    super::push_changes(state, modem.id, &state.metrics.note_network(modem.id, &st));
}
