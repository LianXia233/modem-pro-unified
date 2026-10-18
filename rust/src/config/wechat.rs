//! `[wechat]` 段：企业微信推送。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct WeChatConfig {
    pub enabled: bool,
    pub base_url: String,
    pub bot_token: String,
    pub bot_id: String,
    /// 机器人自己的微信 id（扫码登录后由接口写回，登录应答里带）。
    ///
    /// ⚠️ 以前没有这个字段：配置注释里写着它，但结构体没收 ⇒ 扫码登录写盘后
    /// 下次启动又变回未登录。serde 默认**忽略未知键**，所以这种丢失是静默的。
    pub bot_user_id: String,
    pub to_user_id: String,
    pub template: String,
    pub listen: bool,
    /// 只接受这些发送者（逗号分隔子串，留空 = 不限制）。
    ///
    /// 同上：以前结构体里没有，`/api/wechat/set` 存了也读不回来。
    pub allowed_senders: String,
}

impl WeChatConfig {
    pub(super) fn normalize(&mut self) {
        // listen 不写默认开（仅整段缺失时兜底）。
        if !self.listen && self.base_url.is_empty() {
            self.listen = true;
        }
        if self.base_url.trim().is_empty() {
            self.base_url = "https://ilinkai.weixin.qq.com".to_string();
        }
    }
}
