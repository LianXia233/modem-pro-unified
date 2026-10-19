//! `GET /api/modules` —— 支持模块清单（91 个型号）与逐型号 AT 指令面。
//!
//! 三种用法（与前端「支持模块」页一致）：
//!
//! | 请求 | 返回 |
//! | --- | --- |
//! | `/api/modules` | `{count, usb_count, pcie_count, vendor_count, vendors, models}` |
//! | `/api/modules?model=RM500Q-GL` | `{plan}` —— 按型号离线推导 |
//! | `/api/modules?modem_id=0` | `{plan, live:true}` —— 按在网模组 |
//!
//! 清单数据来自 QModem 的 `support_list.md`（见 `modem/catalog.rs`），
//! 不在清单里的型号**不猜**：`in_catalog=false`，只回显识别到的型号/厂商。
//!
//! 注意：上游 Go 版的 `plan.commands` 是从「真实实现里读出来的」逐条能力表
//! （含 parser/formats）。Rust 重构版目前只把**清单里登记的 USB/PCIe 模式切换指令**
//! 作为可查询项，其余能力走 `/api/vendor/cmds` 与 `/api/at/presets`，避免伪造一份
//! 与实现脱节的愿望表。

use std::collections::HashMap;

use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::modem::catalog;

use super::modem_by_query;

pub fn handle(req: Request, state: &AppState, query: &HashMap<String, String>) -> Result<()> {
    // ① 在网模组：用实测型号查清单
    if query.contains_key("modem_id") {
        let Some(modem) = modem_by_query(state, query) else {
            return response::fail(req, -1, "modem not found");
        };
        let model = modem.model().unwrap_or_default();
        let plan = build_plan(&model, Some(modem.vendor().as_str()), true);
        return response::ok(req, serde_json::json!({ "plan": plan }));
    }

    // ② 按型号：离线推导
    if let Some(model) = query.get("model").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let plan = build_plan(model, None, false);
        return response::ok(req, serde_json::json!({ "plan": plan }));
    }

    // ③ 全量清单（不带 AT 明细：91 个型号的计划太大，点开哪台再单独拉 ②）
    let (count, usb_count, pcie_count, vendor_count) = catalog::stats();
    let vendors: Vec<serde_json::Value> = catalog::vendor_counts()
        .into_iter()
        .map(|(vendor, name, count)| {
            serde_json::json!({ "vendor": vendor, "name": name, "count": count })
        })
        .collect();
    let models: Vec<serde_json::Value> = catalog::entries()
        .iter()
        .map(|e| {
            serde_json::json!({
                "vendor": e.vendor,
                "vendor_name": e.vendor_label(),
                "model": e.model,
                "platform": e.platform,
                "usb_modes": e.usb,
                "pcie_modes": e.pcie,
                "usb_text": e.usb_text(),
                "pcie_text": e.pcie_text(),
            })
        })
        .collect();
    response::ok(
        req,
        serde_json::json!({
            "count": count,
            "usb_count": usb_count,
            "pcie_count": pcie_count,
            "vendor_count": vendor_count,
            "vendors": vendors,
            "models": models,
        }),
    )
}

/// 组一个 `plan`（纯函数，便于单测）。
fn build_plan(model: &str, vendor_hint: Option<&str>, live: bool) -> serde_json::Value {
    let hit = catalog::lookup(model);
    let vendor = hit
        .map(|e| e.vendor.clone())
        .or_else(|| vendor_hint.map(|s| s.to_string()))
        .unwrap_or_default();
    let platform = hit.map(|e| e.platform.clone()).unwrap_or_default();
    let usb: Vec<String> = hit.map(|e| e.usb.clone()).unwrap_or_default();
    let pcie: Vec<String> = hit.map(|e| e.pcie.clone()).unwrap_or_default();

    let mut commands: Vec<serde_json::Value> = Vec::new();
    let mut gaps: Vec<String> = Vec::new();
    let mut gap_notes: Vec<String> = Vec::new();

    if let Some(e) = hit {
        for side in ["usb", "pcie"] {
            let (modes, set) = if side == "usb" {
                (&e.usb, e.usb_at.as_ref())
            } else {
                (&e.pcie, e.pcie_at.as_ref())
            };
            if modes.is_empty() {
                continue;
            }
            if let Some(s) = set {
                if !s.query.is_empty() {
                    commands.push(serde_json::json!({
                        "key": format!("{side}_query"),
                        "label": format!("查询 {} 模式", side.to_uppercase()),
                        "cmd": s.query,
                        "parser": "catalog::ModeAtSet",
                        "format": "AT 查询（当前模式）",
                        "on": true,
                        "source": "QModem 指令脚本",
                    }));
                }
            }
            let mut missing = Vec::new();
            for kind in modes {
                let cmd = if side == "usb" {
                    e.usb_command_for(kind)
                } else {
                    e.pcie_command_for(kind)
                };
                match cmd {
                    Some(cmd) => commands.push(serde_json::json!({
                        "key": format!("{side}_{kind}"),
                        "label": format!("{} 切到 {}", side.to_uppercase(), catalog::kind_label(kind)),
                        "cmd": cmd,
                        "parser": "catalog::ModeAtSet",
                        "format": "完整 AT 指令（可直接下发）",
                        "on": true,
                        "source": "QModem 指令脚本",
                    })),
                    None => missing.push(kind.clone()),
                }
            }
            if !missing.is_empty() {
                gaps.push(format!(
                    "{} 模式切换：{}",
                    side.to_uppercase(),
                    missing.join(", ")
                ));
                gap_notes.push(
                    set.as_ref()
                        .map(|s| s.note.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| {
                            "QModem 未登记该形态的切换指令（只支持查询或由驱动决定）".into()
                        }),
                );
            }
        }
    } else {
        gaps.push("型号不在支持清单里：USB/PCIe 模式未知".into());
        gap_notes.push("请用 /api/vendor/cmds 查看该厂商的实际 AT 指令".into());
    }

    serde_json::json!({
        "model": hit.map(|e| e.model.clone()).unwrap_or_else(|| model.to_string()),
        "vendor": vendor,
        "vendor_name": catalog::vendor_label(&vendor),
        "platform": platform,
        "platform_name": catalog::platform_label(&platform),
        "in_catalog": hit.is_some(),
        "live": live,
        "usb_modes": usb,
        "pcie_modes": pcie,
        "usb_text": catalog::kinds_text(&usb),
        "pcie_text": catalog::kinds_text(&pcie),
        "cmd_impl": "rust::modem::model（内置）",
        "cmd_impl_note": "指令面来自厂商模块；本页仅列 QModem 清单登记的 USB/PCIe 模式切换指令",
        "commands": commands,
        "gaps": gaps,
        "gap_notes": gap_notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_for_known_model_has_modes_and_switch_commands() {
        let p = build_plan("RM500Q-GL", None, false);
        assert_eq!(p["in_catalog"], true);
        assert_eq!(p["vendor"], "quectel");
        assert_eq!(p["usb_modes"][0], "qmi");
        assert_eq!(p["pcie_modes"][0], "qmi");
        let cmds = p["commands"].as_array().expect("commands");
        assert!(cmds
            .iter()
            .any(|c| c["key"] == "usb_qmi" && c["cmd"] == "AT+QCFG=\"usbnet\",0"));
        // PCIe 只有查询，没有瞎编切换指令
        assert!(cmds
            .iter()
            .all(|c| !c["key"].as_str().unwrap_or("").starts_with("pcie_")
                || c["key"] == "pcie_query"));
    }

    #[test]
    fn plan_for_350_reports_rndis_and_mbim_pcie() {
        let p = build_plan("FM350-GL", None, true);
        assert_eq!(p["vendor"], "fibocom");
        assert_eq!(p["usb_text"], "RNDIS");
        assert_eq!(p["pcie_text"], "MBIM");
        assert_eq!(p["live"], true);
    }

    #[test]
    fn unknown_model_is_flagged_not_guessed() {
        let p = build_plan("XYZ-9999", Some("quectel"), false);
        assert_eq!(p["in_catalog"], false);
        assert_eq!(p["vendor"], "quectel", "厂商提示仍要保留");
        assert!(p["gaps"][0].as_str().unwrap().contains("不在支持清单"));
        assert!(p["commands"].as_array().unwrap().is_empty());
    }
}
