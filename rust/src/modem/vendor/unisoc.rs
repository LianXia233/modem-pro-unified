//! 展锐 Unisoc（含中兴 F50 一类 UMS9620 平台）。
//!
//! 识别靠 `AT+CGMR` 的平台标识（本模组不支持 `ATI`，发过去回 `+CME ERROR: 4`）。
//! 提供不了 `AT+QNWINFO` 这类网络接入信息，明确返回空串让调用方别发空指令。

use crate::modem::at::AtPreset;

use super::{any_line, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::Unisoc
    }

    fn label(&self) -> &'static str {
        "展锐 Unisoc"
    }

    fn detect(&self, lines: &[String]) -> bool {
        any_line(lines, &["UMS9620", "UNISOC", "MOCORTM", "QOGIRN", "SPRD"])
    }

    fn net_info_cmd(&self) -> &'static str {
        // 平台明确不提供这类信息：空串 ⇒ 调用方直接回空、不要往串口发空指令
        ""
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("展锐（本模块）", "AT+CGMR", "平台/硬件版本（本模组不支持 ATI，发 ATI 会回 +CME ERROR: 4）"),
            of("展锐（本模块）", "AT+CGMI", "模组厂商（本机回 Spreadtrum Communication CO.）"),
            of("展锐（本模块）", "AT+CGCONTRDP=1", "当前激活承载：APN + 本机 IP + 掩码 + DNS（必须带 cid）"),
            of("展锐（本模块）", "AT+CGCONTRDP=?", "本机支持的 cid 列表（实测 (1,11)：1 = 数据、11 = IMS）"),
            of("展锐（本模块）", "AT+SPIMEI?", "展锐原生 IMEI 回读（改 IMEI 走页面上的「IMEI」面板）"),
            of("展锐（本模块）", "AT+SPPHYCHINFO?", "频点 / 带宽 / PCI / 频段"),
            of("展锐（本模块）", "AT+SPMEID?", "MEID（实测 AA00000203EDE3）"),
            of("展锐（本模块）", "AT+CGDCONT?", "PDP 上下文（展锐的应答不带引号）"),
            of("展锐（本模块）", "AT+C5GQOSRDP", "签约速率 AMBR + QCI（3GPP 5GS QoS，每条承载一行）"),
            of("展锐（本模块）", "AT+CCLK?", "模组时钟（本机出厂未校时；页面「校时」按钮走 AT+CCLK= 写入）"),
            of("展锐（本模块）", "AT+COPS=?", "扫网：列出可见运营商（耗时较长，会短暂影响业务）"),
            of("短信（展锐·PDU）", "AT+CMGF=0", "切 PDU 模式（本机默认就是 0，必须用它）"),
            of("短信（展锐·PDU）", "AT+CMGL=4", "读全部短信（PDU 模式，4 = 全部）"),
            of("短信（展锐·PDU）", "AT+CPMS?", "短信存储用量（本机实测 SM 区）"),
            of("短信（展锐·PDU）", "AT+CSCA?", "短信中心号码（CSCS 为 HEX 时回的是 hex 串）"),
            of("短信（展锐·PDU）", "AT+CSCS=\"GSM\"", "切回 GSM 字符集（这样 CSCA/USSD 才是可读文本）"),
            of("组网模式（展锐）", "AT+SPRAT?", "读组网模式（本机唯一能回读的锁网项）"),
            of("组网模式（展锐）", "AT+SP5GRAN?", "读 5G 开关：1 = 5G+4G、0 = 仅 4G"),
            of("组网模式（展锐）", "AT+SP5GRAN=?", "5G 开关的参数形式（实测 (0-1)）"),
            of("组网模式（展锐）", "AT+SP5GRAN=0", "关闭 5G（仅 4G）。实测：NR 消失、落到 LTE"),
            of("组网模式（展锐）", "AT+SP5GRAN=1", "开启 5G（5G+4G，默认）。要恢复 5G 就发这一条"),
            of(
                "锁网（展锐 SP*·参数未取证）",
                "AT+SPCOMMLOCKBAND=?",
                "锁频段：参数形式（7 个）。⚠️ 带参数执行实测会把频段配置清空导致掉网，包内一律不自动发；掉网后用 AT+CFUN=0 再 AT+CFUN=1 可恢复",
            ),
            of(
                "锁网（展锐 SP*·参数未取证）",
                "AT+SPCOMMLOCKFRQ=?",
                "锁频点：参数形式（9 个）。实测 =2,12,0 被接受（回 OK）但无回显，含义无法确认",
            ),
            of(
                "锁网（展锐 SP*·参数未取证）",
                "AT+SPCELLLOCK=?",
                "锁基站：参数形式（4 个）。实测 =0,0,0 是清除，=1,0,3,140 只存下第 1 个参数",
            ),
            of(
                "锁网（展锐 SP*·已实测可用）",
                "AT+SPCELLLOCK=0,0,0",
                "解除小区锁定（实测可用，不影响驻网）。自己试锁网之后靠它把模组拉回来",
            ),
            of(
                "锁频段（展锐·已实测可用）",
                "AT^SYSCFGEX?",
                "读锁频段/组网配置：第 5 个字段是 LTE 频段位图，第 2 个是 2G/3G 频带码",
            ),
            of(
                "锁频段（展锐·已实测可用）",
                "AT^SYSCFGEX=?",
                "本机声明的 LTE 频段名单 —— 页面的「设备能力」列就是它",
            ),
            of(
                "锁频段（展锐·已实测可用）",
                "AT+SPBANDCTRL=0,0,0,0",
                "按 5 组回显频段配置（只读形态；实测 1=GSM、2=TD-SCDMA、3=WCDMA、4=LTE-TDD、5=LTE-FDD）",
            ),
            of("锁网（展锐 SP*·参数未取证）", "AT+SPLBAND=?", "锁频段（位图写法，6 个参数）"),
            of("锁网（展锐 SP*·参数未取证）", "AT+SPBANDCTRL=?", "频段控制（4 个参数）"),
            of(
                "锁网（展锐 SP*·参数未取证）",
                "AT+SPFREQSCAN=?",
                "频点扫描的参数形式（12 个参数）。只发这一条（=?）；带参数执行会堵死模组 AT 队列，要重启才恢复",
            ),
            of(
                "展锐·信道/邻区（未取证·只列 =?）",
                "AT+SPNRCHANNEL=?",
                "NR 信道上报开关（参数 0/1）。固件里另有 +SPNRCHANNELU 上报，带 5 个十六进制值",
            ),
            of(
                "展锐·信道/邻区（未取证·只列 =?）",
                "AT+SPNCELLSCAN=?",
                "邻区扫描（参数 0/1/2/16/19）。执行形式会触发扫描、可能短暂掉线，请在你方便时再试",
            ),
            of("展锐·信道/邻区（未取证·只列 =?）", "AT+SPQ4GNCELL=?", "4G 邻区（应答格式 +SPQ4GNCELL: %d）"),
            of("展锐·信道/邻区（未取证·只列 =?）", "AT+SPQ4GNCELLEX=?", "4G 邻区扩展（2 个自由参数）"),
            of("展锐·信道/邻区（未取证·只列 =?）", "AT+SPFDDBAND=?", "FDD 频段配置（3 个参数）"),
            of("展锐·信道/邻区（未取证·只列 =?）", "AT+SPCCHO=?", "小区选择相关（3 个参数）"),
            of("设置（谨慎·展锐）", "AT+SFUN=4", "模组重启（厂商工具用的就是这条，约 20 秒后回来）"),
            tpl(
                "设置（谨慎·展锐）",
                "AT+SPIMEI=0,\"<15位IMEI>\"",
                "写 IMEI（模板：把 <15位IMEI> 换成实际号码；写完必须 AT+CGSN 回读确认）",
            ),
        ]
    }

    fn suppressed_common(&self) -> &'static [&'static str] {
        // 实测：中兴 F50 / UMS9620 不认不带 cid 的 AT+CGCONTRDP
        &["AT+CGCONTRDP"]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT+CGMR",
            "AT+SPPHYCHINFO?",
            "AT+C5GQOSRDP",
            "AT+SPRAT?",
            "AT+SP5GRAN?",
        ]
    }

    fn mode_read(&self) -> (&'static str, &'static str) {
        // 唯一能回读的锁网项；写走 5G 开关 AT+SP5GRAN
        ("AT+SPRAT?", "sprat")
    }

    fn at_plan_commands(
        &self,
        ctx: &crate::modem::atplan::PlanCtx,
    ) -> Vec<crate::modem::atplan::AtCmdSpec> {
        use crate::modem::atplan::{add_from_vendor, replace, AtCmdSpec};
        let mut v = crate::modem::atplan::base_specs(ctx);

        add_from_vendor(&mut v, ctx, "serving", "服务小区（SPPHYCHINFO）", "spphychinfo", "model::cells::parse_serving", "展锐物理层信息");
        replace(&mut v, AtCmdSpec::off(
            "imei_set", "IMEI 改写", "本厂商未提供可验证的 IMEI 写指令（只读）"));
        replace(&mut v, AtCmdSpec::on(
            "lock_mode", "组网模式偏好（SPRAT）", "AT+SPRAT?", "model::mode::sprat", "展锐 RAT 枚举", "厂商指令集"));

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

fn tpl(group: &'static str, cmd: &'static str, desc: &'static str) -> AtPreset {
    AtPreset {
        group,
        cmd,
        desc,
        tpl: true,
    }
}
