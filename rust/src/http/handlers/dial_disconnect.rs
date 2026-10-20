//! `POST /api/dial/disconnect` —— 断开数据连接（`{modem_id,cid}`）。
//!
//! cid 省略（或 0）时用当前拨号的 cid，再不行用数据默认 cid。
//! 失败时**仍然把状态带回去**（data = DialState）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};

use super::{dial_connect::push_dial, modem_by_query};

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));
    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(req, -1, "modem not found");
    };
    let cid = request::i64_param(&params, "cid", 0) as i32;
    // 用户点了「断开」：先把拨号器停掉，再让模组侧收尾。
    //
    // 顺序要紧 —— 反过来的话模组侧先 `AT+CGACT=0`，拨号器下一轮 QMI 轮询发现
    // 承载没了会自己重拨，我们又刚把它标成 manual_stop，两边会抢。
    // 而且 `hangup` 里已经置了 `manual_stop`，组网循环那一轮也不会再拉起来。
    crate::runtime::dialer::supervisor().stop_modem(modem.id);
    match modem.hangup(cid) {
        Ok(_) => {
            // 会话断了 ≠ 内核里的旧地址/旧默认路由没了。不清的话，流量会继续
            // 往一张没有承载的网卡上发（黑洞），直到 DHCP 租约过期。
            // 只清**这台模组自己的**数据网卡，绝不碰 LAN/WAN 的其它接口。
            let iface = modem.dial_status().host_iface;
            if !iface.trim().is_empty() {
                let done = crate::netprov::route::flush_stale(&iface);
                modem.note_step(
                    "清理主机网卡旧地址/路由",
                    true,
                    if done.is_empty() {
                        "无残留（或 ip 不可用）"
                    } else {
                        "已删除旧默认路由与地址"
                    },
                );
            }
            let st = modem.dial_status();
            push_dial(state, &st);
            response::ok_msg(
                req,
                serde_json::to_value(&st)?,
                &format!("ok, 已断开 cid={}", st.cid),
            )
        }
        Err(e) => {
            let st = modem.dial_status();
            push_dial(state, &st);
            response::fail_data(req, -1, serde_json::to_value(&st)?, &e.to_string())
        }
    }
}
