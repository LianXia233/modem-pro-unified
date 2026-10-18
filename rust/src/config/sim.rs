//! `[sim]` 段：SIM 热插拔检测。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SimConfig {
    pub hotswap_interval: String,
    pub hotswap_reset: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            hotswap_interval: "20s".to_string(),
            hotswap_reset: false,
        }
    }
}
