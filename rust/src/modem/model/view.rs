//! 对外视图（`/api/modems` 序列化用）。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ModemView {
    pub id: u32,
    pub at_port: String,
    /// 同一设备的串口分组。简化模型（一个 AT 口 = 一个模组）下就是当前口自己，
    /// 界面拿它填「AT 命令口」下拉的候选。
    pub tty_list: Vec<String>,
    pub vendor: &'static str,
    /// 厂商可读名（界面「厂商」一栏用）。
    pub vendor_name: &'static str,
    /// 型号短名（界面显示）。
    pub model: Option<String>,
    /// 识别用的原始应答（ATI 原文；排查问题用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_raw: Option<String>,
    /// 这台模组的 AT 命令口**是用户手动指定的**（界面下拉里标「手动」）。
    pub at_port_manual: bool,
    pub running: bool,
    /// 最近一次基站扫描的时间（没扫过就没有这个字段；见 `GET /api/cells`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell_scan_at: Option<String>,
}
