//! `[ui]` 段：Web 界面与鉴权。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// 「跟随 LuCI 登录」闸门；不写也算开。
    pub require_luci_login: bool,
    /// ubus HTTP JSON-RPC 端点，留空用 "http://127.0.0.1/ubus"。
    pub ubus_http: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            require_luci_login: true,
            ubus_http: String::new(),
        }
    }
}

impl UiConfig {
    pub(super) fn normalize(&mut self) {
        if self.ubus_http.trim().is_empty() {
            self.ubus_http = "http://127.0.0.1/ubus".to_string();
        }
    }
}
