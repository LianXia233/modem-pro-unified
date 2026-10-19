//! `GET /api/traffic?modem_id=N` —— 链路实时速率、累计流量与最近 60 秒曲线。
//!
//! ## 数据来源
//!
//! `runtime/ifstat.rs`：后台线程**固定 1000ms** 从
//! `/sys/class/net/<iface>/statistics/{rx,tx}_bytes` 采样，与前端是否连接无关。
//! 接口名由 `netprov::pick_device` 从模组自己的 USB 设备推出来（不硬编码 eth2/wwan0）。
//!
//! ## 为什么不用 `modem/traffic.rs`（AT 计数器）
//!
//! FM350-GL 的 `AT+GTSTATIS` 实测回 `ERROR`，`AT+QGDCNT` 也不支持，于是那条路
//! `supported=false`、页面速率与累计全是 0。sysfs 的计数一直是准的 ——
//! 它才是「这条数据链路上到底过了多少字节」的权威来源。
//!
//! 两种口径都在响应里保留，但**命名不同**、不会混用：
//! - 本接口：`iface_sent` / `iface_recv`（接口计数器）、`session_sent` / `session_recv`
//!   （本进程启动后累计）；
//! - `modem/traffic.rs`：`GET /api/network/status` 里的 `traffic.sent/recv`（模组侧）。

use std::collections::HashMap;

use serde::Serialize;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::runtime::ifstat::IfStatView;

use super::modem_by_query;

/// 响应体：`IfStatView` 的字段平铺 + `modem_id`。
///
/// 平铺（而不是 `{modem_id, traffic: {...}}`）是为了让前端直接
/// `r.data.up_bps` 取值，少一层解构 —— 与其它接口的「平铺 data」风格一致。
#[derive(Serialize)]
struct TrafficResp {
    modem_id: u32,
    /// 模组对象是否已经登记（AT 口可用）。
    ///
    /// `false` 不代表「没有流量」：链路统计走 sysfs，与 AT 口无关。
    /// 页面可以据此把「模组 AT 未就绪」和「流量为 0」区分开。
    modem_found: bool,
    #[serde(flatten)]
    stat: IfStatView,
}

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    // 页面在轮询就说明「有人在看」——后台采样虽然不依赖它，但信号/网络循环要看
    state.note_http_seen();

    // ⚠️ 这里**不**因为 `modem not found` 就报错。
    //
    // 速率与累计来自 `/sys/class/net/<iface>/statistics`，是**链路级**事实，
    // 不依赖 AT 口能不能打开。服务刚起、模组正被别的进程探测、AT 口暂时打不开
    // ——这些情况下网卡上的字节数照样在涨，页面没理由显示「接口不存在」。
    // 原实现直接 `fail("modem not found")`，会让「有流量但读不到」和「真没流量」
    // 在页面上长得一模一样。
    let wanted = query
        .get("modem_id")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let (modem_id, modem_found) = match modem_by_query(state, query) {
        Some(m) => (m.id, true),
        None => (wanted, false),
    };
    let stat = state.ifstat.view();
    response::json_body(
        req,
        TrafficResp {
            modem_id,
            modem_found,
            stat,
        },
    )
}
