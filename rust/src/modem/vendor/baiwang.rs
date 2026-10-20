//! 百旺 Baiwang（大疆 QDC507）。
//!
//! ## 依据（参考实现重新编写，不复制 GPL 代码）
//!
//! 来源：`ainuoyan/QDC507-Modem-Support` @ `bb57bb7`（`luci-app-modem` 部分
//! 为 GPL-3.0；本模块只按其中**逐条出现过的指令事实**重新实现）：
//!
//! - `root/usr/share/modem/baiwang.sh` 第 2 行注释原文：
//!   *"Baiwang/DJI QDC507 uses the LTE Quectel AT dialect, not the RM5xx dialect"*
//!   —— 用的是 **LTE 代**移远方言，不是 RM5xx 那一套；
//! - `baiwang_dial()` / `baiwang_stop_dial()`：拨号只有 `AT+CGACT=1,1` /
//!   `AT+CGACT=0,1`（`define_connect` 默认 1），**没有** `AT+QNETDEVCTL`
//!   这类私有激活指令 —— 所以本模块不覆盖 `data_channel`，`datacall::plan`
//!   会把它统一拦到 3GPP 通用路径（与参考实现逐字一致）；
//! - `modem_support.json` 的 `qdc507` 条目：`manufacturer_id=2c7c`（与移远
//!   **同 VID**）、`platform=qualcomm`、`modes=["ecm"]`、`define_connect=1`；
//! - `baiwang.sh` 其余函数直接 `source quectel.sh`：温度 `AT+QTEMP`、电压
//!   `AT+CBC`、连接状态 `AT+CGACT?`、ICCID `AT+QCCID`、接入信息 `AT+QNWINFO`、
//!   小区 `AT+QENG`（LTE 固件，行尾到 CQI 为止，比 RM5xx 短）；
//! - `baiwang_control.lua`：组网模式 `AT+QCFG="nwscanmodeex"`，位标志
//!   2G=4 / 3G=8 / 4G=16；锁频段 `AT+QCFG="band"`（三段掩码，LTE 段 72 位）。
//!
//! ## 实机验证边界（README「已验证环境与边界」）
//!
//! 参考实现的实机验证来自 iStoreOS 25.12.5 x86_64 + QDC507GLEFM21 +
//! USB `2c7c:0125` + ECM；且明言 *"QDC507 适配仅支持 ECM，不宣称 NCM、
//! MBIM、QMI 或 5G 已验证"*。本模块的能力面照此收敛：只登记读得到、
//! 拨得出的部分。
//!
//! ## 识别为什么放在移远前面
//!
//! QDC507 的 USB 身份是 `2c7c:0125`（与移远 LTE 模组同 VID:PID），ATI 也可能
//! 只回 `Quectel`；而探测链 `AT+CGMM → AT+CGMI → ATI` 的第一条就是型号
//! （参考实现 `quectel_base_info` 也用 `AT+CGMM` 取型号）。因此
//! [`DETECT_ORDER`](super::DETECT_ORDER) 里本厂商排**第一**：只要应答里出现
//! `QDC507` / `BAIWANG`，就先判百旺，避免拿到移远的 `QNETDEVCTL` 方言
//! （LTE 固件不认，只会回 ERROR）。

use crate::modem::at::AtPreset;

use super::{any_line, UsbnetWrite, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Baiwang
    }

    fn label(&self) -> &'static str {
        "Baiwang（QDC507）"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["QDC507", "BAIWANG"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        // LTE 固件的 QENG 行尾到 CQI 为止（比 RM5xx 短），解析端按缺字段容错
        "AT+QENG=\"servingcell\""
    }

    fn net_info_cmd(&self) -> &'static str {
        "AT+QNWINFO"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("百旺（本模块）", "ATI", "模组型号 / 固件版本"),
            of(
                "百旺（本模块）",
                "AT+QNWINFO",
                "网络接入信息（制式/频段/信道）",
            ),
            of(
                "百旺（本模块）",
                "AT+QENG=\"servingcell\"",
                "服务小区（LTE 行形状，到 CQI 为止）",
            ),
            of("百旺（本模块）", "AT+QCCID", "ICCID"),
            of("百旺（本模块）", "AT+CSQ", "信号强度"),
            of("百旺（本模块）", "AT+CESQ", "扩展信号"),
            of("百旺（本模块）", "AT+QTEMP", "温度传感器"),
            of("百旺（本模块）", "AT+CBC", "供电电压"),
            of(
                "拨号（百旺）",
                "AT+CGACT?",
                "PDP 激活状态（拨号即 CGACT=1,1）",
            ),
            of(
                "USB（百旺）",
                "AT+QCFG=\"usbnet\"",
                "USB 网卡模式（只读；仅 ECM）",
            ),
            of(
                "锁网（百旺）",
                "AT+QCFG=\"band\"",
                "锁频段掩码（三段：GSM/UMTS/LTE）",
            ),
            of(
                "组网模式（百旺）",
                "AT+QCFG=\"nwscanmodeex\"",
                "组网模式偏好（位标志：2G=4 / 3G=8 / 4G=16）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "ATI",
            "AT+QNWINFO",
            "AT+QENG=\"servingcell\"",
            "AT+QCCID",
            "AT+CSQ",
            "AT+QTEMP",
            "AT+CBC",
            "AT+CGACT?",
            "AT+QCFG=\"usbnet\"",
            "AT+QCFG=\"band\"",
            "AT+QCFG=\"nwscanmodeex\"",
        ]
    }

    fn temperature_cmds(&self, _mtk_platform: bool) -> &'static [&'static str] {
        // baiwang.sh 直接复用 quectel_get_temperature
        &["AT+QTEMP"]
    }

    /// 参考实现不查流量统计 —— 未取证就不发，候选表留空。
    fn traffic_candidates(&self) -> &'static [&'static str] {
        &[]
    }

    /// USB 网卡模式：读走移远方言（`AT+QCFG="usbnet"`），写只认 ECM 一档。
    ///
    /// 依据 `modem_support.json` 的 `modes:["ecm"]` 与 `baiwang_set_mode()`
    /// 的 `[ "$2" = "ecm" ] || return 1` —— 其余档位（RMNET/MBIM/RNDIS/NCM）
    /// 参考实现没有实现，切过去没有依据。
    fn usbnet_write(&self, value: u8) -> UsbnetWrite {
        match value {
            1 => UsbnetWrite::Allow,
            _ => UsbnetWrite::Deny(
                "QDC507 只有 ECM 一种数据形态（参考实现 modes=[\"ecm\"]），该档位未取证",
            ),
        }
    }

    fn mode_read(&self) -> (&'static str, &'static str) {
        // 解析分支在 model::mode::baiwang_mode_from_nwscanmodeex
        ("AT+QCFG=\"nwscanmodeex\"", "baiwang_nwscan")
    }

    // 拨号就是 3GPP 通用 `AT+CGACT`（参考实现 `baiwang_dial` 逐字如此），
    // 因此**不覆盖** `data_channel`：`datacall::plan` 对未登记厂商统一走
    // 通用路径。这是与移远最大的差别 —— 移远同平台要发 `QNETDEVCTL`。
    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::{add_from_vendor, replace, AtCmdSpec};
        let mut v = crate::modem::atplan::base_specs(ctx);

        // ICCID 走移远方言（+QCCID 裸数字），解析器两者都吃
        add_from_vendor(
            &mut v,
            ctx,
            "iccid",
            "ICCID",
            "qccid",
            "model::siminfo::parse_iccid",
            "+QCCID: 裸数字",
        );
        // 电压走 AT+CBC（参考实现 quectel_get_voltage），不是通用 CADC
        add_from_vendor(
            &mut v,
            ctx,
            "voltage",
            "供电电压",
            "cbc",
            "model::siminfo::parse_voltage",
            "+CBC: bcs,bcl,mV（两种行形状）",
        );
        // 锁频段只有读（三段 QCFG 掩码）；写路径未集成，见 UPSTREAM_AUDIT
        add_from_vendor(
            &mut v,
            ctx,
            "lock_band",
            "锁频段（只读）",
            "\"band\"",
            "model::lock::parse_qcfg_band",
            "QCFG 三段频段掩码",
        );
        // IMEI 改写未取证：只读
        replace(
            &mut v,
            AtCmdSpec::off(
                "imei_set",
                "IMEI 改写",
                "QDC507 未提供可验证的 IMEI 写指令（只读）",
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
