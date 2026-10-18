//! 支持模块清单（骨架版）。
//!
//! 完整清单应包含 USB / PCIe 型号以及每型号的 AT 指令与解析格式；
//! 骨架阶段先内置一个最小表，用于 `GET /api/modules` 返回结构演示与后续扩展
//! （后续可由 JSON 清单覆盖，见 docs/INTEGRATION.md）。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ModuleEntry {
    /// 型号关键字（匹配时忽略 空格/-/_/. ）
    pub model: String,
    pub vendor: &'static str,
    /// USB 连接方式：qmi / mbim / ecm / ncm / rndis
    pub usb_kind: &'static str,
    /// 是否走主机侧拨号器
    pub host_dial: bool,
}

/// 内置最小清单（后续由 JSON 覆盖）。
pub fn builtin() -> Vec<ModuleEntry> {
    vec![
        ModuleEntry {
            model: "fm160".into(),
            vendor: "fibocom",
            usb_kind: "qmi",
            host_dial: true,
        },
        ModuleEntry {
            model: "fm150".into(),
            vendor: "fibocom",
            usb_kind: "qmi",
            host_dial: true,
        },
        ModuleEntry {
            model: "rm500".into(),
            vendor: "quectel",
            usb_kind: "qmi",
            host_dial: true,
        },
        ModuleEntry {
            model: "ec20".into(),
            vendor: "quectel",
            usb_kind: "qmi",
            host_dial: true,
        },
        ModuleEntry {
            model: "srm815".into(),
            vendor: "meig",
            usb_kind: "qmi",
            host_dial: true,
        },
        ModuleEntry {
            model: "sim7600".into(),
            vendor: "simcom",
            usb_kind: "qmi",
            host_dial: true,
        },
        ModuleEntry {
            model: "sim8200".into(),
            vendor: "simcom",
            usb_kind: "qmi",
            host_dial: true,
        },
    ]
}
