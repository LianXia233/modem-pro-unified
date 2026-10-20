//! 各接口处理器：一个接口一个文件。

pub mod airplane;
pub mod airplane_set;
pub mod at;
pub mod at_port;
pub mod at_presets;
pub mod at_sched;
pub mod atport_bind;
pub mod cells;
pub mod cells_all;
pub mod changes;
pub mod changes_clear;
pub mod dial;
pub mod dial_connect;
pub mod dial_disconnect;
pub mod dial_auto;
pub mod dial_enable;
pub mod dial_ipv6;
pub mod dial_mode;
pub mod host_nic;
pub mod host_nic_apply;
pub mod host_nic_save;
pub mod imei;
pub mod imei_restore;
pub mod imei_set;
pub mod lock_band;
pub mod lock_caps;
pub mod lock_cell;
pub mod lock_mode;
pub mod lock_status;
pub mod lock_unlock;
pub mod logs;
pub mod mode;
pub mod mode_set;
pub mod modem_clock;
pub mod modem_info;
pub mod modem_reset;
pub mod modem_status;
pub mod modems;
pub mod modules;
pub mod netinfo;
pub mod netscan;
pub mod netscan_stop;
pub mod network;
pub mod network_status;
pub mod ntp;
pub mod ntp_set;
pub mod ntp_sync;
pub mod params;
pub mod params_apn;
pub mod params_apn_delete;
pub mod params_netselect;
pub mod params_pdp;
pub mod pcie;
pub mod pcie_bind;
pub mod pcie_reset;
pub mod quota;
pub mod quota_reset;
pub mod quota_set;
pub mod quota_unblock;
pub mod roam;
pub mod roam_set;
pub mod schedule;
pub mod schedule_delete;
pub mod schedule_run;
pub mod schedule_set;
pub mod schedule_toggle;
pub mod session;
pub mod signal;
pub mod signal_history;
pub mod sim;
pub mod sms_delete;
pub mod sms_read;
pub mod sms_send;
pub mod sms_storage;
pub mod sms_storage_set;
pub mod snapshot;
pub mod tasks;
pub mod timezone;
pub mod traffic;
pub mod traffic_reset;
pub mod usb;
pub mod usb_bind;
pub mod usb_clear;
pub mod usbnet;
pub mod usbnet_set;
pub mod ussd;
pub mod vendor_cmds;
pub mod version;
pub mod wechat;

use std::collections::HashMap;
use std::sync::Arc;

use crate::http::AppState;
use crate::modem::model::{ChangeEntry, Modem};

/// 从 query / 表单的 `modem_id`（不给就当 0）取模组。
///
/// ⚠️ **`modem_id` 给错了必须返回 `None`，不能回落成 0**：
/// 这个函数的调用方里有一大批是写操作（锁频段、改 IMEI、删短信、切飞行模式…），
/// 回落 0 等于把「锁 3 号模组的频段」悄悄执行成「锁 0 号模组的频段」，
/// 而调用方只会看到 `modem not found` 之外的正常返回。宁可让请求失败并被看见。
pub fn modem_by_query(state: &AppState, query: &HashMap<String, String>) -> Option<Arc<Modem>> {
    let raw = query
        .get("modem_id")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let id = match raw {
        // 没给 = 单模组场景的默认值。**回落到第一台实际存在的模组**，
        // 不再硬编码 0 —— USB 重新枚举 / 模组重启后管理器会重新编号
        // （实测 id 0 → 1），硬编码 0 会让所有缺省请求 404、前端整页崩。
        None => state
            .mgr
            .all()
            .first()
            .map(|m| m.id)
            .unwrap_or(0),
        // 给了但解析不了 = 调用方出错，让它看见「找不到」，不要猜
        Some(s) => s.parse::<u32>().ok()?,
    };
    state.mgr.get(id)
}

/// 把新增的波动记录推给所有页面（没有新增时什么都不做）。
///
/// 与查询共用同一份数据：查询顺手比对一次，不额外发 AT。
pub fn push_changes(state: &AppState, modem_id: u32, entries: &[ChangeEntry]) {
    if entries.is_empty() {
        return;
    }
    let data = serde_json::to_value(entries).unwrap_or(serde_json::Value::Null);
    state.ws.broadcast("changes", Some(modem_id), data);
}

/// 「本平台不支持按 VID:PID 绑定命令口」的说明文案（Windows 用）。
pub fn atport_bind_supported_msg() -> &'static str {
    "本平台不支持按 VID:PID 绑定命令口（Windows 请在「AT 指令」页的串口下拉里指定）"
}
