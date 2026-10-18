//! `[sms]` 段：短信存储区偏好。
//!
//! 只有一项：`sms_memory`（`SM` / `ME` / `MT`）。它由 `POST /api/sms/storage`
//! 在切换成功后写回，进程启动后**第一次读短信时**应用到模组（见
//! `Modem::apply_sms_memory_pref`）—— 不放在发现阶段，避免对每台模组都多发
//! 一条 `AT+CPMS`（有的固件不支持 `ME`/`MT`，发现阶段失败会刷一屏 warn）。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SmsConfig {
    /// 期望的存储区（空 = 不干预，沿用模组当前值）。
    pub sms_memory: String,
}

impl Default for SmsConfig {
    fn default() -> Self {
        Self {
            sms_memory: "SM".to_string(),
        }
    }
}
