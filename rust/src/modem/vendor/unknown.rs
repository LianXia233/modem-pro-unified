//! 未识别厂商：只给 3GPP 通用能力。
//!
//! 宁可少给，也不要让用户对着别家的私有指令点来点去。

use super::{Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Unknown
    }

    fn label(&self) -> &'static str {
        "未识别"
    }

    fn detect(&self, _lines: &[String]) -> bool {
        false
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &["AT+CSQ", "AT+CESQ", "AT+COPS?"]
    }

    fn mode_read(&self) -> (&'static str, &'static str) {
        // 未识别厂商沿用「先试移远那套」的老行为（失败会自然降级为 unsupported）
        ("AT+QNWPREFCFG=\"mode_pref\"", "qcf")
    }
}
