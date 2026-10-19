//! 路由分发。
//!
//! [`ROUTES`] 是接口全表（方法 + 路径），与后端注册表一一对应；
//! 每个登记项都必须有 handler（没有「待迁移」兜底了），
//! 未登记的 `/api/*` 返回 404，其余交静态页。

use std::sync::Arc;

use tiny_http::{Method, Request};

use crate::error::Result;

use super::{auth, handlers, request, response, static_ui, AppState};

/// 接口全表：`(METHOD, PATH)`。
///
/// 运行时不再用它做兜底（每个接口都有 handler，未登记就是 404）；保留它是为了
/// 和 `docs/PORTING.md` 对照，并让测试能校验「表里没有重复项」。
#[cfg_attr(not(test), allow(dead_code))]
pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/version"),
    ("GET", "/api/snapshot"),
    ("GET", "/api/tasks"),
    ("POST", "/api/tasks"),
    ("GET", "/api/tasks/get"),
    ("POST", "/api/tasks/cancel"),
    ("POST", "/api/login"),
    ("POST", "/api/logout"),
    ("GET", "/api/session"),
    ("GET", "/api/modems"),
    ("GET", "/api/modem/info"),
    ("GET", "/api/modem/status"),
    ("POST", "/api/modem/clock"),
    ("POST", "/api/modem/reset"),
    ("GET", "/api/sim"),
    ("GET", "/api/vendor/cmds"),
    ("GET", "/api/signal"),
    ("GET", "/api/signal/history"),
    ("GET", "/api/network"),
    ("GET", "/api/network/status"),
    ("GET", "/api/netinfo"),
    ("GET", "/api/airplane"),
    ("POST", "/api/airplane/set"),
    ("GET", "/api/roam"),
    ("POST", "/api/roam/set"),
    ("GET", "/api/cells"),
    ("GET", "/api/cells/all"),
    ("GET", "/api/netscan"),
    ("POST", "/api/netscan/stop"),
    ("GET", "/api/lock/status"),
    ("GET", "/api/lock/caps"),
    ("POST", "/api/lock/band"),
    ("POST", "/api/lock/cell"),
    ("POST", "/api/lock/mode"),
    ("POST", "/api/lock/unlock"),
    ("GET", "/api/mode"),
    ("POST", "/api/mode/set"),
    ("GET", "/api/dial"),
    ("POST", "/api/dial/enable"),
    ("POST", "/api/dial/mode"),
    ("POST", "/api/dial/ipv6"),
    ("POST", "/api/dial/connect"),
    ("POST", "/api/dial/disconnect"),
    ("GET", "/api/usbnet"),
    ("POST", "/api/usbnet/set"),
    ("GET", "/api/sms/read"),
    ("POST", "/api/sms/send"),
    ("POST", "/api/sms/delete"),
    ("GET", "/api/sms/storage"),
    ("POST", "/api/sms/storage"),
    ("GET", "/api/wechat"),
    ("POST", "/api/wechat/login"),
    ("POST", "/api/wechat/verify"),
    ("POST", "/api/wechat/set"),
    ("POST", "/api/wechat/test"),
    ("POST", "/api/wechat/logout"),
    ("GET", "/api/params"),
    ("POST", "/api/params/apn"),
    ("POST", "/api/params/apn/delete"),
    ("POST", "/api/params/pdp"),
    ("POST", "/api/params/netselect"),
    ("GET", "/api/quota"),
    ("POST", "/api/quota/set"),
    ("POST", "/api/quota/reset"),
    ("POST", "/api/quota/unblock"),
    ("POST", "/api/traffic/reset"),
    ("GET", "/api/ussd"),
    ("POST", "/api/ussd"),
    ("GET", "/api/imei"),
    ("POST", "/api/imei/set"),
    ("POST", "/api/imei/restore"),
    ("GET", "/api/schedule"),
    ("POST", "/api/schedule/set"),
    ("POST", "/api/schedule/run"),
    ("POST", "/api/schedule/toggle"),
    ("POST", "/api/schedule/delete"),
    ("GET", "/api/changes"),
    ("POST", "/api/changes/clear"),
    ("GET", "/api/logs"),
    ("GET", "/api/modules"),
    ("GET", "/api/pcie"),
    ("POST", "/api/pcie/reset"),
    ("POST", "/api/pcie/bind"),
    ("GET", "/api/usb"),
    ("POST", "/api/usb/clear"),
    ("POST", "/api/usb/bind"),
    ("GET", "/api/atport/bind"),
    ("POST", "/api/atport/bind"),
    ("GET", "/api/at"),
    ("POST", "/api/at"),
    ("POST", "/api/at/port"),
    ("GET", "/api/at/presets"),
    ("GET", "/api/host/nic"),
    ("POST", "/api/host/nic/apply"),
    ("POST", "/api/host/nic/save"),
    ("GET", "/api/ntp"),
    ("POST", "/api/ntp/set"),
    ("POST", "/api/ntp/sync"),
    ("GET", "/ws"),
    ("GET", "/display/ws"),
];

/// 处理一个已抵达路由层的请求（鉴权在内部完成）。
pub fn handle(req: Request, state: Arc<AppState>) -> Result<()> {
    let url = req.url().to_string();
    let path = request::path_of(&url);
    let query = request::parse_query(&url);

    if !auth::is_authorized(&state, &req) {
        // `need_login` 让页面即使在非 401 平台代理下也能识别；`login_url` 是 LuCI 入口，
        // 页面「去 LuCI」按钮直接用（不写死路径）。
        return response::fail_status_data(
            req,
            401,
            -1,
            serde_json::json!({ "need_login": true, "login_url": "/cgi-bin/luci/" }),
            "请先登录路由器管理账号",
        );
    }

    let m = method(&req);
    match (m, path.as_str()) {
        ("GET", "/api/version") => handlers::version::handle(req, &state),
        // 异步化：快照（缓存优先，不发 AT）+ 任务（慢操作提交 → 轮询）
        ("GET", "/api/snapshot") => handlers::snapshot::handle(req, &state, &query),
        // 任务接口收 `Arc<AppState>`：任务闭包要 'static，得把状态搬进工作线程
        ("GET", "/api/tasks") | ("POST", "/api/tasks") => {
            handlers::tasks::handle(req, state.clone(), &query)
        }
        ("GET", "/api/tasks/get") => handlers::tasks::handle(req, state.clone(), &query),
        ("POST", "/api/tasks/cancel") => handlers::tasks::handle(req, state.clone(), &query),
        ("GET", "/api/modems") => handlers::modems::handle(req, &state, &query),
        ("GET", "/api/modem/info") => handlers::modem_info::handle(req, &state, &query),
        ("GET", "/api/modem/status") => handlers::modem_status::handle(req, &state, &query),
        ("POST", "/api/modem/clock") => handlers::modem_clock::handle(req, &state, &query),
        ("POST", "/api/modem/reset") => handlers::modem_reset::handle(req, &state, &query),
        ("GET", "/api/sim") => handlers::sim::handle(req, &state, &query),
        ("GET", "/api/vendor/cmds") => handlers::vendor_cmds::handle(req, &state, &query),
        ("GET", "/api/signal") => handlers::signal::handle(req, &state, &query),
        ("GET", "/api/signal/history") => handlers::signal_history::handle(req, &state, &query),
        ("GET", "/api/network") => handlers::network::handle(req, &state, &query),
        ("GET", "/api/network/status") => handlers::network_status::handle(req, &state, &query),
        ("GET", "/api/netinfo") => handlers::netinfo::handle(req, &state, &query),
        ("GET", "/api/airplane") => handlers::airplane::handle(req, &state, &query),
        ("GET", "/api/roam") => handlers::roam::handle(req, &state, &query),
        ("GET", "/api/mode") => handlers::mode::handle(req, &state, &query),
        ("GET", "/api/usbnet") => handlers::usbnet::handle(req, &state, &query),
        ("GET", "/api/imei") => handlers::imei::handle(req, &state, &query),
        ("POST", "/api/imei/set") => handlers::imei_set::handle(req, &state, &query),
        ("POST", "/api/imei/restore") => handlers::imei_restore::handle(req, &state, &query),
        ("GET", "/api/params") => handlers::params::handle(req, &state, &query),
        ("POST", "/api/params/apn") => handlers::params_apn::handle(req, &state, &query),
        ("POST", "/api/params/apn/delete") => {
            handlers::params_apn_delete::handle(req, &state, &query)
        }
        ("POST", "/api/params/pdp") => handlers::params_pdp::handle(req, &state, &query),
        ("POST", "/api/params/netselect") => {
            handlers::params_netselect::handle(req, &state, &query)
        }
        ("POST", "/api/mode/set") => handlers::mode_set::handle(req, &state, &query),
        ("POST", "/api/usbnet/set") => handlers::usbnet_set::handle(req, &state, &query),
        ("POST", "/api/airplane/set") => handlers::airplane_set::handle(req, &state, &query),
        ("POST", "/api/roam/set") => handlers::roam_set::handle(req, &state, &query),
        ("GET", "/api/dial") => handlers::dial::handle(req, &state, &query),
        ("POST", "/api/dial/enable") => handlers::dial_enable::handle(req, &state, &query),
        ("POST", "/api/dial/mode") => handlers::dial_mode::handle(req, &state, &query),
        ("POST", "/api/dial/ipv6") => handlers::dial_ipv6::handle(req, &state, &query),
        ("POST", "/api/dial/connect") => handlers::dial_connect::handle(req, &state, &query),
        ("POST", "/api/dial/disconnect") => handlers::dial_disconnect::handle(req, &state, &query),
        ("GET", "/api/quota") => handlers::quota::handle(req, &state, &query),
        ("POST", "/api/quota/set") => handlers::quota_set::handle(req, &state, &query),
        ("POST", "/api/quota/reset") => handlers::quota_reset::handle(req, &state, &query),
        ("POST", "/api/quota/unblock") => handlers::quota_unblock::handle(req, &state, &query),
        ("POST", "/api/traffic/reset") => handlers::traffic_reset::handle(req, &state, &query),
        ("GET", "/api/schedule") => handlers::schedule::handle(req, &state, &query),
        ("POST", "/api/schedule/set") => handlers::schedule_set::handle(req, &state, &query),
        ("POST", "/api/schedule/delete") => handlers::schedule_delete::handle(req, &state, &query),
        ("POST", "/api/schedule/toggle") => handlers::schedule_toggle::handle(req, &state, &query),
        ("POST", "/api/schedule/run") => handlers::schedule_run::handle(req, &state, &query),
        ("GET", "/api/host/nic") => handlers::host_nic::handle(req, &state, &query),
        ("POST", "/api/host/nic/apply") => handlers::host_nic_apply::handle(req, &state, &query),
        ("POST", "/api/host/nic/save") => handlers::host_nic_save::handle(req, &state, &query),
        ("GET", "/api/ntp") => handlers::ntp::handle(req, &state, &query),
        ("POST", "/api/ntp/set") => handlers::ntp_set::handle(req, &state, &query),
        ("POST", "/api/ntp/sync") => handlers::ntp_sync::handle(req, &state, &query),
        ("GET", "/api/ussd") => handlers::ussd::handle(req, &state, &query),
        ("POST", "/api/ussd") => handlers::ussd::handle(req, &state, &query),
        ("GET", "/api/cells") => handlers::cells::handle(req, &state, &query),
        ("GET", "/api/cells/all") => handlers::cells_all::handle(req, &state, &query),
        ("GET", "/api/netscan") => handlers::netscan::handle(req, &state, &query),
        ("POST", "/api/netscan/stop") => handlers::netscan_stop::handle(req, &state, &query),
        ("GET", "/api/lock/status") => handlers::lock_status::handle(req, &state, &query),
        ("GET", "/api/lock/caps") => handlers::lock_caps::handle(req, &state, &query),
        ("POST", "/api/lock/band") => handlers::lock_band::handle(req, &state, &query),
        ("POST", "/api/lock/cell") => handlers::lock_cell::handle(req, &state, &query),
        ("POST", "/api/lock/mode") => handlers::lock_mode::handle(req, &state, &query),
        ("POST", "/api/lock/unlock") => handlers::lock_unlock::handle(req, &state, &query),
        ("GET", "/api/modules") => handlers::modules::handle(req, &state, &query),
        ("GET", "/api/sms/read") => handlers::sms_read::handle(req, &state, &query),
        ("POST", "/api/sms/send") => handlers::sms_send::handle(req, &state, &query),
        ("POST", "/api/sms/delete") => handlers::sms_delete::handle(req, &state, &query),
        ("GET", "/api/sms/storage") => handlers::sms_storage::handle(req, &state, &query),
        ("POST", "/api/sms/storage") => handlers::sms_storage_set::handle(req, &state, &query),
        ("GET", "/api/at") | ("POST", "/api/at") => handlers::at::handle(req, &state, &query),
        ("POST", "/api/at/port") => handlers::at_port::handle(req, &state, &query),
        ("GET", "/api/at/presets") => handlers::at_presets::handle(req, &state, &query),
        ("GET", "/api/atport/bind") | ("POST", "/api/atport/bind") => {
            handlers::atport_bind::handle(req, &state, &query)
        }
        ("GET", "/api/usb") => handlers::usb::handle(req, &state, &query),
        ("POST", "/api/usb/clear") => handlers::usb_clear::handle(req, &state, &query),
        ("POST", "/api/usb/bind") => handlers::usb_bind::handle(req, &state, &query),
        ("GET", "/api/pcie") => handlers::pcie::handle(req, &state, &query),
        ("POST", "/api/pcie/reset") => handlers::pcie_reset::handle(req, &state, &query),
        ("POST", "/api/pcie/bind") => handlers::pcie_bind::handle(req, &state, &query),
        ("GET", "/api/changes") => handlers::changes::handle(req, &state, &query),
        ("POST", "/api/changes/clear") => handlers::changes_clear::handle(req, &state, &query),
        ("GET", "/api/logs") => handlers::logs::handle(req, &state, &query),
        ("GET", "/api/session") => handlers::session::session(req, &state),
        ("POST", "/api/login") => handlers::session::login(req, &state, &query),
        ("POST", "/api/logout") => handlers::session::logout(req),
        // 企业微信推送（短信密码已取消，只剩外层登录闸门）
        ("GET", "/api/wechat") => handlers::wechat::handle(req, &state),
        ("POST", "/api/wechat/login") => handlers::wechat::login(req, &state),
        ("POST", "/api/wechat/verify") => handlers::wechat::verify(req, &state),
        ("POST", "/api/wechat/set") => handlers::wechat::set(req, &state, &query),
        ("POST", "/api/wechat/test") => handlers::wechat::test(req, &state),
        ("POST", "/api/wechat/logout") => handlers::wechat::logout(req, &state),
        _ if path.starts_with("/api/") => response::fail_status(req, 404, -1, "接口不存在"),
        _ => static_ui::serve(req, &path),
    }
}

/// HTTP 方法名（仅区分 GET/POST，其余归入 OTHER）。
fn method(req: &Request) -> &'static str {
    match req.method() {
        Method::Get => "GET",
        Method::Post => "POST",
        _ => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use super::ROUTES;
    use std::collections::HashSet;

    #[test]
    fn route_table_has_no_duplicates() {
        let mut seen = HashSet::new();
        for (m, p) in ROUTES {
            assert!(
                seen.insert((*m, *p)),
                "路由表里有重复项: {m} {p}（同一个接口注册了两次）"
            );
        }
        assert!(!ROUTES.is_empty());
        // 全部是 /api/ 或两个 WebSocket 升级路径 —— 静态页不该混进来
        for (m, p) in ROUTES {
            assert!(
                p.starts_with("/api/") || *p == "/ws" || *p == "/display/ws",
                "路由表里混进了非接口路径: {m} {p}"
            );
        }
    }
}
