//! 移远 Quectel。
//!
//! 识别关键字 `QUECTEL`；数据面首选 QMI（与 quectel-CM 的主线一致）。
//! QENG / QNWCFG / QNWLOCK 这一族都是移远私有的（其它厂商不一定认）。

use crate::modem::at::AtPreset;

use super::{any_line, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Quectel
    }

    fn label(&self) -> &'static str {
        "移远 Quectel"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["QUECTEL"])
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("移远（本模块）", "ATI", "模组型号（移远支持 ATI）"),
            of(
                "移远（本模块）",
                "AT+QNWINFO",
                "网络接入信息（制式/频段/信道）",
            ),
            of(
                "移远（本模块）",
                "AT+QENG=\"servingcell\"",
                "服务小区（含 NR 信号）",
            ),
            of("移远（本模块）", "AT+QENG=\"neighbourcell\"", "邻区列表"),
            of("移远（本模块）", "AT+QCAINFO", "载波聚合分量载波"),
            of("移远（本模块）", "AT+QGDCNT?", "流量计数器（累计字节）"),
            of(
                "移远（本模块）",
                "AT+QNWCFG=\"nr5g_ambr\"",
                "签约速率 AMBR + CQI",
            ),
            of("移远（本模块）", "AT+QTEMP", "温度传感器"),
            of("移远（本模块）", "AT+QCFG=\"usbnet\"", "USB 网卡模式"),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"mode_pref\"",
                "组网模式偏好（当前值）",
            ),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"policy_band\"",
                "允许使用的频段（解锁时的还原依据）",
            ),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"lte_band\"",
                "LTE 锁频段（当前值）",
            ),
            of(
                "锁网（移远）",
                "AT+QNWPREFCFG=\"nr5g_band\"",
                "NR 锁频段（当前值）",
            ),
            of("锁网（移远）", "AT+QNWLOCK=\"common/4g\"", "LTE 锁小区状态"),
            of("锁网（移远）", "AT+QNWLOCK=\"common/5g\"", "NR 锁小区状态"),
            of(
                "锁网（移远）",
                "AT+QNWLOCK=\"common/4g\",0",
                "解锁 LTE 小区",
            ),
            of("锁网（移远）", "AT+QNWLOCK=\"common/5g\",0", "解锁 NR 小区"),
            of(
                "设置（谨慎）",
                "AT+QCFG=\"usbnet\",2",
                "切 MBIM 网卡模式（需重启模组）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+QNWINFO",
            "AT+QENG=\"servingcell\"",
            "AT+QENG=\"neighbourcell\"",
            "AT+QCAINFO",
            "AT+QCSQ",
            "AT+QTEMP",
            "AT+QGDCNT?",
        ]
    }

    fn traffic_candidates(&self) -> &'static [&'static str] {
        &["AT+QGDCNT?"]
    }

    fn temperature_cmds(&self, _mtk: bool) -> &'static [&'static str] {
        &["AT+QTEMP"]
    }
    fn mode_read(&self) -> (&'static str, &'static str) {
        ("AT+QNWPREFCFG=\"mode_pref\"", "qcf")
    }

    /// `AT+QNETDEVCTL` 家族：参数形状随平台变，写错会被回 `ERROR`。
    ///
    /// 依据 QModem `modem_dial.sh:1298-1319`（at_dial）与 `:1130-1133`（ecm_hang）：
    /// - 海思：`AT+QNETDEVCTL=1,1,1`（不支持指定 cid）；
    /// - 展锐：`AT+QNETDEVCTL=1,<cid>,1`；
    /// - 高通及其它：`AT+QNETDEVCTL=3,<cid>,1`（3 = NDIS/ECM 接入）。
    fn data_channel(
        &self,
        ctx: &super::datacall::DataCallCtx<'_>,
    ) -> Option<super::datacall::DataChannel> {
        use super::datacall::{AtStep, DataChannel};
        let cid = ctx.cid;
        let (cmd, why) = match ctx.platform {
            "hisilicon" => (
                "AT+QNETDEVCTL=1,1,1".to_string(),
                "海思平台：QNETDEVCTL 不支持指定 cid，固定承载 1",
            ),
            "unisoc" => (
                format!("AT+QNETDEVCTL=1,{cid},1"),
                "展锐平台：QNETDEVCTL=<cid>,<op>,<urc>",
            ),
            _ => (
                format!("AT+QNETDEVCTL=3,{cid},1"),
                "高通/其它平台：connect_type=3（NDIS/ECM 接入）",
            ),
        };
        Some(DataChannel {
            pdp_first: false,
            activate: vec![AtStep::slow(cmd, why)],
            deactivate: vec![AtStep::new(
                format!("AT+QNETDEVCTL={cid},2,1"),
                "QNETDEVCTL op=2 = 断开数据通道",
            )],
            cgact_fallback: true,
            autodial_probe: None,
            note: "移远 ECM/RNDIS/NCM 由 QNETDEVCTL 激活；只发 CGACT 时网卡常常没有 carrier",
        })
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
            "neighbour",
            "邻区",
            "neighbour",
            "model::cells::parse_neighbour",
            "QENG 邻区列表",
        );
        add_from_vendor(
            &mut v,
            ctx,
            "ca",
            "载波聚合 CA",
            "qcainfo",
            "model::cells::parse_ca",
            "分量载波列表",
        );
        replace(
            &mut v,
            AtCmdSpec::on(
                "iccid",
                "ICCID",
                "AT+QCCID",
                "model::siminfo::parse_iccid",
                "+QCCID: 裸数字",
                "厂商指令集",
            ),
        );
        replace(
            &mut v,
            AtCmdSpec::on(
                "ambr",
                "签约速率 AMBR",
                "AT+QNWCFG=\"nr5g_ambr\"",
                "model::network::parse_ambr",
                "AMBR + CQI",
                "厂商指令集",
            ),
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
            if ctx.legacy_lte {
                AtCmdSpec::on(
                    "lock_band",
                    "锁频段",
                    "AT+QCFG=\"band\",0,<hex 位图>,0",
                    "model::lock::parse_qcfg_band",
                    "lte 旧款位图",
                    "厂商指令集",
                )
            } else {
                let mut s = AtCmdSpec::on(
                    "lock_band",
                    "锁频段",
                    "AT+QNWPREFCFG=\"lte_band\"",
                    "model::lock::parse_qnwpref",
                    "LTE / NR5G 频段列表",
                    "厂商指令集",
                );
                s.candidates = vec![
                    "AT+QNWPREFCFG=\"lte_band\"".into(),
                    "AT+QNWPREFCFG=\"nr5g_band\"".into(),
                ];
                s
            },
        );

        v
    }
}

/// 预设表构造（同 `at/presets.rs` 的写法，保持表可读）。
fn of(group: &'static str, cmd: &'static str, desc: &'static str) -> AtPreset {
    AtPreset {
        group,
        cmd,
        desc,
        tpl: false,
    }
}
