//! `POST /api/at/port` —— 手动指定这台模组的 AT 命令口（跨重启记住）。
//!
//! 契约（与 docs/API.md 一致）：
//!   · 键用「模组身份」（IMEI 优先，其次型号；未识别时用当前口自己当兜底键），
//!     **不是端口号** —— 端口号正是要换的那个东西；
//!   · 允许指定候选之外的口，但必须是系统里真实存在的串口；
//!   · 保存前先验一下它真能答 AT（不是当前正在用的口才验）；已识别模组验不过即拒，
//!     未识别模组放行（那时的现状本来就是坏的，硬拒 = 用户永远修不回来）；
//!   · 空串 = 恢复自动探测。
//!
//! 简化模型说明（Rust 侧一个 AT 口 = 一个模组，暂无 USB 设备分组）：
//!   · `tty_list` 这里就是当前口自己；
//!   · 「立刻切」是整口换掉并重新识别；
//!   · 换口成功后回收被换口上的「幻影模组」（同一物理设备的另一个 tty 在简化模型下
//!     会各自成一个模组）。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{request, response, AppState};
use crate::modem::at::AtPort;
use crate::modem::port_exists;

use super::modem_by_query;

pub fn handle(mut req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    let params = request::merge_form(query, &request::read_body(&mut req));

    let Some(modem) = modem_by_query(state, &params) else {
        return response::fail(
            req,
            -1,
            &format!("modem not found: {}", modem_id_of(&params)),
        );
    };
    let port = params.get("port").cloned().unwrap_or_default();
    let port = port.trim().to_string();
    let baud = state.cfg.serial.baud;
    let probe_timeout = state.cfg.serial.probe_timeout();

    // 键：身份键优先；身份读不出来（未识别模组）用「当前口自己」当兜底键
    let identity_key = modem.at_port_key();
    let fallback_key = format!("ports:{}", modem.port());
    let key = identity_key.clone().unwrap_or_else(|| fallback_key.clone());
    if key.is_empty() {
        // 连串口信息都没有（理论上不该发生）⇒ 只能靠端口名反查着清记录，保证能回到自动探测
        if port.is_empty() && state.atport.clear_by_port(&modem.port()) > 0 {
            return response::ok_msg(
                req,
                serde_json::json!({ "modem_id": modem.id, "port": "" }),
                "已清掉这台模组的手动指定，恢复自动探测",
            );
        }
        return response::fail(req, -1, "这台模组还没有串口信息，等探测完再选");
    }

    let mut verified = true;
    if !port.is_empty() {
        // 允许指定候选之外的口（简化模型下候选就是当前口自己），但必须是系统里真实存在的串口
        if port != modem.port() && !port_exists(&port) {
            return response::fail(req, -1, &format!("串口 {port} 不在本机串口列表里"));
        }
        // ⚠️ 保存前先验一下它真能答 AT（不是当前正在用的口才验，免得把自己占用中的口判死）：
        // 存一个不应答的口 = 这台模组下次重启后就认不出来了。
        // 例外：未识别的模组（IMEI/型号都读不出来 —— 通常正是"AT 口挑错了"造成的）一律放行。
        if port != modem.port() && !AtPort::probe(&port, baud, probe_timeout) {
            if identity_key.is_some() {
                return response::fail(
                    req,
                    -1,
                    &format!(
                        "串口 {port} 不应答 AT，不能当命令口用（换成能答的那个，或选自动探测）"
                    ),
                );
            }
            verified = false;
        }
    }

    let changed = state.atport.set(&key, &port);
    // 身份读出来了就以身份键为准：顺手清掉「未识别时期」用兜底键存的记录（免得两份记录打架）
    if let Some(ik) = &identity_key {
        if *ik != key {
            state.atport.set(&fallback_key, "");
        }
    }

    // 立刻切（不等下一次巡检）：简化模型里就是整口换掉并重新识别
    let mut note = String::new();
    if port.is_empty() {
        // 恢复自动探测：在候选里挑一个能答 AT 的口（排除当前口）
        let pick = crate::modem::ports_list()
            .into_iter()
            .find(|p| *p != modem.port() && AtPort::probe(p, baud, probe_timeout));
        if let Some(p) = pick {
            if let Err(e) = modem.repoint(&p) {
                note = format!("（切回 {p} 失败: {e}，保持原口）");
            }
        }
        modem.set_at_port_manual(false);
    } else if port != modem.port() {
        if let Err(e) = modem.repoint(&port) {
            note = format!("（切到 {port} 失败: {e}，台账已记住，重启后按它试）");
        } else {
            modem.set_at_port_manual(true);
            reclaim_ghosts(state, &modem);
            // 切口后身份可能读出来了：把记录从兜底键迁到身份键
            if let Some(ik) = modem.at_port_key() {
                if ik != key {
                    state.atport.set(&ik, &port);
                    state.atport.set(&key, "");
                }
            }
        }
    } else {
        modem.set_at_port_manual(true);
    }

    let msg = if port.is_empty() {
        format!("已恢复自动探测（发现流程会重新挑一个命令口）{note}")
    } else if !verified {
        format!("已记住 {port}（这个口现在没答上 AT —— 发现流程会按它试；不行再换一个口）{note}")
    } else {
        format!("已切到 {port}（已记住，重启后仍用它）{note}")
    };
    response::ok_msg(
        req,
        serde_json::json!({ "modem_id": modem.id, "port": port, "changed": changed, "key": key }),
        &msg,
    )
}

/// 回收被换口上的「幻影模组」（同一物理设备的另一个 tty 在简化模型下各自成一个模组）。
fn reclaim_ghosts(state: &AppState, modem: &crate::modem::model::Modem) {
    let target = modem.port();
    for other in state.mgr.all() {
        if other.id != modem.id && other.port() == target {
            log::info!(
                "回收幻影模组 id={} at={target}（id={} 的手动命令口）",
                other.id,
                modem.id
            );
            state.mgr.remove(other.id);
        }
    }
}

fn modem_id_of(params: &HashMap<String, String>) -> String {
    params
        .get("modem_id")
        .cloned()
        .unwrap_or_else(|| "0".to_string())
}
