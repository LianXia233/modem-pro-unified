//! 鼎桥 TD Tech（MT5700 系列）。
//!
//! 识别关键字 `MT5700`。网络/信号/温度/流量全在 `^` 私有命令里
//! （移远那套全回 ERROR）；数据面虽是 ECM/NCM，实机用的却是**展锐那套
//! 模组自拨**（`AT^SETAUTODIAL`，见 `data_channel`），不是海思的 `AT^NDISDUP`。

use crate::modem::at::AtPreset;

use super::{any_line, Vendor, VendorSpec};

pub struct Spec;

impl VendorSpec for Spec {
    fn id(&self) -> Vendor {
        Vendor::TDTech
    }

    fn label(&self) -> &'static str {
        "鼎桥 TD Tech"
    }

    fn detect(&self, lines: &[String]) -> bool {
        // 手册是《MT5700M-CN 5G 系列》（V200R001C20），清单里同族的 mt5710_cn
        // 同属巴龙平台、指令面一致 —— 关键字一并认。固件 ATI 也可能直接报
        // 厂商名（TDTECH / TD TECH），一并接受，别只认型号（A-P2-5）。
        any_line(lines, &["MT5700", "MT5710", "TDTECH", "TD TECH"])
    }

    fn serving_cell_cmd(&self) -> &'static str {
        "AT^MONSC"
    }

    fn net_info_cmd(&self) -> &'static str {
        // 没有 AT+QNWINFO，用它私有的 AT^SYSINFOEX
        "AT^SYSINFOEX"
    }

    fn at_presets(&self) -> Vec<AtPreset> {
        vec![
            of("鼎桥（本模块）", "ATI", "模组型号（鼎桥支持 ATI）"),
            of(
                "鼎桥（本模块）",
                "AT^MONSC",
                "服务小区（NR/LTE，直接给 dB 值）",
            ),
            of("鼎桥（本模块）", "AT^MONNC", "邻区列表"),
            of("鼎桥（本模块）", "AT^HCSQ?", "信号强度（索引形式）"),
            of("鼎桥（本模块）", "AT^SYSINFOEX", "链路制式/系统信息"),
            of(
                "鼎桥（本模块）",
                "AT^HFREQINFO?",
                "工作频率（频段/ARFCN/带宽）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^ICCID?",
                "ICCID（AT+CCID 在本固件回 ERROR）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^CHIPTEMP?",
                "温度（12 路传感器，0.1 ℃）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^DSFLOWQRY",
                "流量统计（累计收发字节，十六进制）",
            ),
            of(
                "鼎桥（本模块）",
                "AT^DSAMBR=1",
                "签约速率 AMBR（kbps，只支持 cid 1）",
            ),
            // 以下出自官方手册《MT5700M-CN 5G 系列模组 AT 命令手册》V200R001C20
            of("鼎桥 手册 §3.17", "AT^VERSION", "软件版本信息"),
            of(
                "鼎桥 手册 §5.35",
                "AT^CELLSCAN",
                "小区扫频（支持无卡扫描，多行结果）",
            ),
            of("鼎桥 手册 §13.22", "AT^MCS?", "上下行 MCS 查询"),
            of(
                "鼎桥 手册 §13.23",
                "AT^TXPOWER?",
                "GUL（2/3/4G）发射功率（NR 下回 ERROR）",
            ),
            of(
                "鼎桥 手册 §13.24",
                "AT^NTXPOWER?",
                "NR 发射功率（每 5 字段一组，最多 4 载波）",
            ),
            of(
                "鼎桥 手册 §13.27",
                "AT^MONSSC",
                "NSA 辅连接服务小区（单载波回 NONE）",
            ),
            of("鼎桥 手册 §13.17", "AT^C5GOPTION?", "5G 接入模式选项"),
            of("鼎桥 手册 §11.7", "AT^LENDC", "NSA 下 LTE-NR DC 双连接状态"),
            of(
                "鼎桥 手册 §13.18",
                "AT^CASCELLINFO?",
                "LTE CA 辅小区信息（单载波回 ERROR）",
            ),
            of("鼎桥 手册 §13.21", "AT^RRCSTAT", "RRC 状态查询"),
            of(
                "鼎桥 手册 §16.6",
                "AT^DHCP",
                "查询模组侧 DHCP/IP（拨号后取地址）",
            ),
            of(
                "鼎桥 手册 §16.8",
                "AT^DCONNSTAT?",
                "拨号连接状态（cid/APN/v4/v6/类型）",
            ),
        ]
    }

    fn vendor_cmds(&self) -> &'static [&'static str] {
        &[
            "AT^MONSC",
            "AT^MONNC",
            "AT^HCSQ?",
            "AT^SYSINFOEX",
            "AT^HFREQINFO?",
            "AT^ICCID?",
            "AT^CHIPTEMP?",
            "AT^DSFLOWQRY",
            "AT^DSAMBR=1",
            "AT^VERSION",
            "AT^CELLSCAN",
            "AT^MONSSC",
            "AT^MCS?",
            "AT^TXPOWER?",
            "AT^NTXPOWER?",
            "AT^C5GOPTION?",
            "AT^LENDC",
            "AT^CASCELLINFO?",
            "AT^RRCSTAT",
            "AT^DHCP",
            "AT^DCONNSTAT?",
        ]
    }

    /// 主动扫频：手册 §5.35，**支持无卡扫描**（多行 `^CELLSCAN:` 结果）。
    fn cell_scan_cmd(&self) -> Option<&'static str> {
        Some("AT^CELLSCAN")
    }

    /// 温度：手册 §17.1，12 路传感器一条应答。
    fn temperature_cmds(&self, _mtk_platform: bool) -> &'static [&'static str] {
        &["AT^CHIPTEMP?"]
    }

    /// 流量：默认候选（`AT+QGDCNT?` / `AT+GTSTATIS?`）在鼎桥上全回 ERROR
    /// （quirks 记录），只有私有的 `AT^DSFLOWQRY`（手册 §16.3，十六进制
    /// last/total 两组）拿得到模组侧累计 —— 见 `model::traffic::parse_dsflowqry`。
    fn traffic_candidates(&self) -> &'static [&'static str] {
        &["AT^DSFLOWQRY"]
    }

    fn mode_read(&self) -> (&'static str, &'static str) {
        // acqorder 双向映射（见 tdtech_acq_order / parse_syscfgex）
        ("AT^SYSCFGEX?", "syscfgex")
    }

    /// 鼎桥 MT5700/MT5710 虽然常被归到「巴龙平台」，实机方言却是**展锐那套
    /// 模组自拨**：`AT^SETAUTODIAL=1,<mode>` / `=0` + `AT^CFUN` 循环，
    /// 数据面用 `AT^NDISSTATQRY?`（手册 §16.4/§16.18）。
    ///
    /// 依据：上游 `luci-app-mt5700`（同型号实机抓样）与 QModem
    /// `huawei_auto_dial_unisoc()` / `auto_dial_hang_huawei_unisoc()`
    /// （`modem_dial.sh:1524-1568`）。此前照搬海思 `AT^NDISDUP` 是错的 ——
    /// MT5700 上这条会回 ERROR，拨号永远停在「已激活、网卡无地址」。
    ///
    /// 型号未落到 MT5700/MT5710 的罕见情况（只报到厂商名）仍保守走巴龙
    /// `AT^NDISDUP`，不凭猜测发私有指令。
    fn data_channel(
        &self,
        ctx: &super::datacall::DataCallCtx<'_>,
    ) -> Option<super::datacall::DataChannel> {
        use super::datacall::{AtStep, DataChannel};
        let cid = ctx.cid;
        if ctx.platform == "unisoc" || crate::modem::model::is_tdtech_mt5700(ctx.model) {
            return Some(DataChannel {
                pdp_first: false,
                activate: vec![AtStep::new(
                    "AT^SETAUTODIAL=1,4",
                    "鼎桥（展锐系）：打开模组自拨（mode=4，与 QModem 默认一致）",
                )],
                deactivate: vec![
                    AtStep::new("AT^SETAUTODIAL=0", "关闭模组自拨").optional(),
                    AtStep::new("AT+CFUN=0", "射频下行（自拨会话需要 CFUN 循环才彻底断开）"),
                    AtStep::new("AT+CFUN=1", "射频恢复上行"),
                ],
                cgact_fallback: false,
                // 先问「自拨开着吗」再决定要不要 CFUN 循环 —— 依据 QModem
                // auto_dial_hang_huawei_unisoc（modem_dial.sh:1546-1568）。
                autodial_probe: Some(super::datacall::AutodialProbe {
                    query: "AT^SETAUTODIAL?",
                    fallback: "AT^NDISDUP=0,0",
                    status: Some("AT^NDISSTATQRY?"),
                }),
                note: "鼎桥 MT5700/MT5710 由模组自拨，主机不持有 PDP；断开需 CFUN 循环（先查自拨状态）",
            });
        }
        Some(DataChannel {
            pdp_first: false,
            activate: vec![AtStep::slow(
                format!("AT^NDISDUP=1,{cid}"),
                "巴龙同源：NDISDUP=1 让该 cid 的数据进入 USB 网卡",
            )],
            deactivate: vec![AtStep::new(
                "AT^NDISDUP=0,0",
                "NDISDUP=0 关闭 NDIS 数据通道",
            )],
            cgact_fallback: true,
            autodial_probe: None,
            note: "鼎桥（未确认型号）暂按巴龙 AT^NDISDUP 处理",
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
            "monnc",
            "model::cells::parse_neighbour",
            "^MONNC 邻区表",
        );
        add_from_vendor(
            &mut v,
            ctx,
            "iccid",
            "ICCID",
            "iccid",
            "model::siminfo::parse_iccid",
            "^ICCID: 数字",
        );
        add_from_vendor(
            &mut v,
            ctx,
            "temp",
            "模组温度",
            "chiptemp",
            "model::siminfo::parse_temp",
            "^CHIPTEMP 多传感器",
        );
        add_from_vendor(
            &mut v,
            ctx,
            "ambr",
            "签约速率 AMBR",
            "dsambr",
            "model::network::parse_ambr",
            "AMBR",
        );
        replace(
            &mut v,
            AtCmdSpec::on(
                "imei_set",
                "IMEI 改写",
                "AT^PHYNUM=IMEI,\"<15 位>\"",
                "model::imei::write",
                "巴龙系",
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
