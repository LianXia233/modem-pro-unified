//! 美格智能 MeiG。
//!
//! 识别关键字 `MEIG` / `SRM8*` / `SLM7*`。数据面首选 MBIM（与 `meig-cm` 同序）。

use crate::modem::at::AtPreset;

use super::{any_line, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Meig
    }

    fn label(&self) -> &'static str {
        "美格智能 MeiG"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["MEIG", "SRM8", "SLM7"])
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("美格（本模块）", "ATI", "模组型号（美格支持 ATI）"),
            of("美格（本模块）", "AT+QNWINFO", "网络接入信息"),
            of(
                "美格（本模块）",
                "AT+QENG=\"servingcell\"",
                "服务小区（含 NR 信号）",
            ),
            of("美格（本模块）", "AT+QENG=\"neighbourcell\"", "邻区列表"),
            of("美格（本模块）", "AT+QCAINFO", "载波聚合分量载波"),
            of(
                "美格（本模块）",
                "AT+QCSQ",
                "信号质量（部分固件用它替代 QENG）",
            ),
            of(
                "美格（本模块）",
                "AT+CPSI?",
                "服务小区（部分固件用它替代 QENG）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+QNWINFO",
            "AT+QENG=\"servingcell\"",
            "AT+QCSQ",
            "AT+CPSI?",
        ]
    }

    fn traffic_candidates(&self) -> &'static [&'static str] {
        &["AT+QGDCNT?"]
    }
    fn mode_read(&self) -> (&'static str, &'static str) {
        ("AT+QNWPREFCFG=\"mode_pref\"", "qcf")
    }

    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::{add_from_vendor, replace, AtCmdSpec};
        let mut v = crate::modem::atplan::base_specs(ctx);

        add_from_vendor(
            &mut v,
            ctx,
            "signal",
            "信号（CSQ/QCSQ）",
            "qcsq",
            "model::signal::parse_qcsq",
            "rsrp/rsrq/sinr",
        );
        replace(
            &mut v,
            AtCmdSpec::on(
                "imei_set",
                "IMEI 改写",
                "AT+EGMR=1,7,\"<15 位>\"",
                "model::imei::write",
                "高通通用，需重启",
                "厂商手册",
            ),
        );
        replace(
            &mut v,
            AtCmdSpec::on(
                "lock_band",
                "锁频段",
                "AT+QNWPREFCFG=\"lte_band\"",
                "model::lock::parse_qnwpref",
                "LTE / NR5G 频段列表",
                "厂商指令集",
            ),
        );

        v
    }
}

fn of(group: &'static str, cmd: &'static str, desc: &'static str) -> AtPreset {
    AtPreset {
        group,
        cmd,
        desc,
        tpl: false,
    }
}
