//! `[quota]` 段：流量配额的默认配置。
//!
//! 这里是**默认值**（config.toml 是 conffile，升级包不会覆盖它）；
//! 用户在页面上按模组保存的配额与累计用量另存 `quota.json`（见 runtime/store.rs），
//! 两份合并规则：json 里有就按 json，没有就取这里的默认值。
//!
//! ⚠️ 计量依赖厂商私有指令（见 modem/model/traffic.rs）：模组不提供计数器时
//! 配额无法启用，接口会把 `supported=false` 如实报给界面。

use serde::Deserialize;

/// 超限动作。
pub const ACTION_BLOCK: &str = "block";
pub const ACTION_BLOCK_RF: &str = "block_rf";
pub const ACTION_WARN: &str = "warn";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QuotaConfig {
    /// 是否启用配额（默认关：启用就会在超限时动数据连接，不能默认替用户做这个决定）
    pub enabled: bool,
    /// 周期流量上限（字节）。0 = 不限量（只统计不断网）
    pub limit_bytes: i64,
    /// 上行是否计入
    pub count_up: bool,
    /// 下行是否计入
    pub count_down: bool,
    /// 超限动作：`block`（停用数据连接）/ `block_rf`（飞行模式）/ `warn`（只提醒）
    pub action: String,
    /// 每月几日重置（1~28）；0 = 不自动重置，只能手动清零
    pub reset_day: i64,
    /// 超限后重复执行断网动作的间隔（秒）。后台采样循环起来后按它反复压制
    /// 「模组自己又拨上了」的情况；当前由查询接口顺手评估。
    pub block_interval: i64,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            limit_bytes: 0,
            count_up: true,
            count_down: true,
            action: ACTION_BLOCK.to_string(),
            reset_day: 0,
            block_interval: 30,
        }
    }
}

impl QuotaConfig {
    pub(super) fn normalize(&mut self) {
        self.action = normalize_action(&self.action);
        if self.limit_bytes < 0 {
            self.limit_bytes = 0;
        }
        // 29~31 有的月份没有，一律夹到 28（界面下拉也只给到 28）
        if self.reset_day < 0 || self.reset_day > 28 {
            self.reset_day = 0;
        }
        if self.block_interval <= 0 {
            self.block_interval = 30;
        }
    }

    /// 超限动作（认不出的写法一律按「停用数据连接」，不让配置写错变成「只提醒」而悄悄超量）。
    pub fn action(&self) -> &'static str {
        match self.action.trim().to_ascii_lowercase().as_str() {
            ACTION_BLOCK_RF => ACTION_BLOCK_RF,
            ACTION_WARN => ACTION_WARN,
            _ => ACTION_BLOCK,
        }
    }

    /// 归一化后返回（供按模组保存设置时使用，保证写进 `quota.json` 的也是合法值）。
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }
}

/// 归一化动作名：空串 / 未知 → `block`。
fn normalize_action(action: &str) -> String {
    match action.trim().to_ascii_lowercase().as_str() {
        ACTION_BLOCK_RF => ACTION_BLOCK_RF.to_string(),
        ACTION_WARN => ACTION_WARN.to_string(),
        _ => ACTION_BLOCK.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_action, QuotaConfig};

    #[test]
    fn defaults_are_safe() {
        let c = QuotaConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.limit_bytes, 0);
        assert!(c.count_up && c.count_down);
        assert_eq!(c.action(), "block");
        assert_eq!(c.reset_day, 0);
        assert_eq!(c.block_interval, 30);
    }

    #[test]
    fn unknown_action_falls_back_to_block() {
        assert_eq!(normalize_action("block_rf"), "block_rf");
        assert_eq!(normalize_action("WARN"), "warn");
        assert_eq!(normalize_action(""), "block");
        assert_eq!(normalize_action("关射频"), "block");
    }

    #[test]
    fn normalize_clamps_bad_values() {
        let mut c = QuotaConfig {
            limit_bytes: -5,
            reset_day: 31,
            block_interval: 0,
            ..Default::default()
        };
        c.normalize();
        assert_eq!(c.limit_bytes, 0);
        assert_eq!(c.reset_day, 0);
        assert_eq!(c.block_interval, 30);
    }
}
