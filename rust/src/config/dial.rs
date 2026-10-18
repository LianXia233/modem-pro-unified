//! `[dial]` 段。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DialConfig {
    pub activate_ims: bool,
}
