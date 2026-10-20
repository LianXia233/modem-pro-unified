//! 模组侧数据通道（ECM / RNDIS / NCM）的拨号与挂断计划。
//!
//! ## 为什么需要这一层
//!
//! QMI / MBIM 的数据会话由主机侧拨号器建立（见 `runtime/dialer`），模组侧只需
//! 让出承载；而 **ECM / RNDIS / NCM 是模组自己持有 PDP**：主机只看到一个网卡，
//! 真正的「拨号」是模组固件里的一条私有 AT 指令。
//!
//! 早先的实现对所有非 QMI/MBIM 机型一律发 `AT+CGACT=1,<cid>`：
//!
//! - 移远 RNDIS/ECM（`AT+QNETDEVCTL`）在多数固件上**不接受**只发 CGACT ——
//!   PDP 显示 active，网卡却一直没 carrier；
//! - 华为 `AT^NDISDUP`、广和通 `AT+GTRNDIS`、美格 `AT$QCRMCALL` 同理；
//! - 反过来，广和通 MTK 平台（FM350）与芯讯通 ASR/LTE 平台**只认** `AT+CGACT`，
//!   给它们发 `AT+GTRNDIS` 会直接 ERROR。
//!
//! 所以这张表按「厂商 × 平台」给出**该发哪一条**，并把挂断做成与激活严格对称的
//! 另一组指令 —— QModem 的 `at_dial()` / `ecm_hang()` 就是这么成对写的
//! （`application/qmodem/files/usr/share/qmodem/modem_dial.sh:1286-1460`、`:1122-1165`）。
//!
//! ## 与上游 A 的差异
//!
//! `ouyangzq/modem-pro` 的非 QMI 路径只做 `AT+CGACT` + 读 `AT+CGPADDR`
//! （`module/kernel/dial_engine.go:330-414`），没有这张厂商表 —— 它的 ECM/RNDIS
//! 机型依赖固件「插上就自拨」。QModem 覆盖了 14 家厂商的私有激活指令。
//! 本项目选择**以 QModem 的表为准**，理由是有具体厂商指令可查、可测；
//! 同时保留一条 `AT+CGACT` 回退，避免老固件因私有指令被拒而完全拨不出去。
//!
//! ## 红线
//!
//! - 只发**有上游依据**的指令（每条都标了来源与适用平台）；
//! - 平台不明时退回 3GPP 通用路径（`AT+CGACT`），不猜私有指令；
//! - 挂断指令必须与激活指令成对；没有依据时用 `AT+CGACT=0` 收尾并注明。

use serde::Serialize;

use super::Vendor;

/// 计划里的一条 AT 指令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AtStep {
    /// 完整指令（`<cid>` 已在构造时替换）。
    pub cmd: String,
    /// 这条指令期望多久内回 `OK`（毫秒）。
    pub timeout_ms: u64,
    /// 失败是否不致命（继续下一步）。
    pub optional: bool,
    /// 为什么发这条（页面「拨号过程」里直接显示）。
    pub why: &'static str,
}

impl AtStep {
    pub(crate) fn new(cmd: impl Into<String>, why: &'static str) -> Self {
        Self {
            cmd: cmd.into(),
            timeout_ms: 8_000,
            optional: false,
            why,
        }
    }

    /// 慢指令（PDP 激活、`AT+GTRNDIS` 等固件内部要等网络侧）。
    pub(crate) fn slow(cmd: impl Into<String>, why: &'static str) -> Self {
        Self {
            cmd: cmd.into(),
            timeout_ms: 20_000,
            optional: false,
            why,
        }
    }

    pub(crate) fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

/// 挂断前的「模组自拨」状态查询（华为展锐专用；其余厂商 `None`）。
///
/// 依据 QModem `auto_dial_hang_huawei_unisoc()`
/// （`application/qmodem/files/usr/share/qmodem/modem_dial.sh:1546-1568`）：
/// 先 `AT^SETAUTODIAL?` 看状态，**只有自拨开着**才 `=0` + CFUN 循环；
/// 本来就是关着时改走通用的 `AT^NDISDUP=0,0`，不碰射频。
/// 不看状态就 CFUN 循环，会把一次「用户点断开」变成整模组重新注册。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutodialProbe {
    /// 查询指令（`AT^SETAUTODIAL?`）。
    pub query: &'static str,
    /// 回读未显示「开着」时改发的断开指令（`AT^NDISDUP=0,0`）。
    pub fallback: &'static str,
}

/// 一家厂商在某个平台上的数据通道配方。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataChannel {
    /// 激活前是否要先发 `AT+CGACT=1,<cid>`。
    ///
    /// `true`：3GPP 激活即可（FM350 MTK、芯讯通 ASR/LTE）；
    /// `false`：私有指令自己会激活 PDP，多发 CGACT 反而可能被拒。
    pub pdp_first: bool,
    /// 数据通道激活指令（按顺序发）。
    pub activate: Vec<AtStep>,
    /// 数据通道断开指令（与 `activate` 对称）。
    pub deactivate: Vec<AtStep>,
    /// 私有指令全部被拒时，是否允许退回 `AT+CGACT=1,<cid>`。
    pub cgact_fallback: bool,
    /// 挂断前先查自拨状态（见 [`AutodialProbe`]）；`None` = 直接走 `deactivate`。
    pub autodial_probe: Option<AutodialProbe>,
    /// 给界面看的一句话（为什么这么拨 / 有哪些限制）。
    pub note: &'static str,
}

impl DataChannel {
    /// 只走 3GPP 通用路径（激活靠 `AT+CGACT`，不额外发私有指令）。
    fn generic(note: &'static str) -> Self {
        Self {
            pdp_first: true,
            activate: Vec::new(),
            deactivate: Vec::new(),
            cgact_fallback: true,
            autodial_probe: None,
            note,
        }
    }
}

/// 构造计划所需的上下文（全部来自**已识别**的模组，不做猜测）。
#[derive(Debug, Clone, Copy)]
pub struct DataCallCtx<'a> {
    /// 数据形态：`ecm` / `rndis` / `ncm`（其余形态不走本模块）。
    pub kind: &'a str,
    /// 清单里的平台列（`qualcomm` / `unisoc` / `mediatek` / `hisilicon` / `lte`…）。
    pub platform: &'a str,
    /// 清单里的型号（小写原名；未识别时为空）。
    pub model: &'a str,
    /// 要操作的 PDP 上下文号。
    pub cid: i32,
}

impl DataCallCtx<'_> {
    /// 这个形态是否需要模组侧拨号（即不是 QMI/MBIM）。
    pub fn is_at_dial_kind(&self) -> bool {
        matches!(self.kind, "ecm" | "rndis" | "ncm" | "gobinet" | "rmnet")
    }
}

/// 查表：厂商 × 平台 → 数据通道配方。
///
/// 平台不明（空串）时一律走通用路径 —— 私有指令的适用条件没确认，不能发。
pub fn plan(vendor: Vendor, ctx: &DataCallCtx<'_>) -> DataChannel {
    if !ctx.is_at_dial_kind() {
        return DataChannel::generic("该形态由主机侧拨号器建立会话，模组侧不激活 PDP");
    }
    if ctx.platform.trim().is_empty() {
        return DataChannel::generic("平台未识别：只走 3GPP 通用 AT+CGACT，不发厂商私有指令");
    }
    vendor.spec().data_channel(ctx).unwrap_or_else(|| {
        DataChannel::generic("该厂商未登记私有数据通道指令：按 3GPP 通用路径拨号")
    })
}

/// 国产/常见厂商的 `AT+CGACT` 收尾（挂断时没有更具体的指令可用）。
pub(crate) fn cgact_deactivate(cid: i32) -> AtStep {
    AtStep::new(
        format!("AT+CGACT=0,{cid}"),
        "3GPP 去激活（无厂商私有挂断指令时的收尾）",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 激活指令里是否已经有「设置 APN 之外」的私有动作。
    fn has_vendor_step(plan: &DataChannel) -> bool {
        plan.activate.iter().any(|s| !s.cmd.trim().is_empty())
    }

    fn ctx<'a>(kind: &'a str, platform: &'a str, model: &'a str, cid: i32) -> DataCallCtx<'a> {
        DataCallCtx {
            kind,
            platform,
            model,
            cid,
        }
    }

    /// 每家的私有指令都必须与 QModem `at_dial()` / `ecm_hang()` 的字面量一致。
    #[test]
    fn vendor_table_matches_qmodem_dial_script() {
        let cases: &[(Vendor, &str, &str, &str, &str)] = &[
            // 厂商, 平台, 激活, 挂断, 型号
            (
                Vendor::Quectel,
                "qualcomm",
                "AT+QNETDEVCTL=3,1,1",
                "AT+QNETDEVCTL=1,2,1",
                "rm520n-gl",
            ),
            (
                Vendor::Quectel,
                "unisoc",
                "AT+QNETDEVCTL=1,1,1",
                "AT+QNETDEVCTL=1,2,1",
                "rm500u-cn",
            ),
            (
                Vendor::Quectel,
                "hisilicon",
                "AT+QNETDEVCTL=1,1,1",
                "AT+QNETDEVCTL=1,2,1",
                "ec20",
            ),
            (
                Vendor::Fibocom,
                "qualcomm",
                "AT+GTRNDIS=1,1",
                "AT+GTRNDIS=0,1",
                "fm150",
            ),
            (
                Vendor::Fibocom,
                "unisoc",
                "AT+GTRNDIS=1,1",
                "AT+GTRNDIS=0,1",
                "fm650-cn",
            ),
            (
                Vendor::Huawei,
                "hisilicon",
                "AT^NDISDUP=1,1",
                "AT^NDISDUP=0,0",
                "e8372",
            ),
            (
                Vendor::Meig,
                "qualcomm",
                "AT$QCRMCALL=1,0,3,2,1",
                "AT$QCRMCALL=0,0,3,2,1",
                "srm815",
            ),
            (
                Vendor::Neoway,
                "unisoc",
                "AT$MYUSBNETACT=0,1",
                "AT$MYUSBNETACT=0,0",
                "n510m",
            ),
            (
                Vendor::Gosuncn,
                "lte",
                "AT+ZECMCALL=1",
                "AT+ZECMCALL=0",
                "gm800",
            ),
            (
                Vendor::Telit,
                "qualcomm",
                "AT#ICMAUTOCONN=1,1",
                "AT+CGACT=0,1",
                "ln920",
            ),
        ];
        for (vendor, platform, activate, deactivate, model) in cases {
            let p = plan(*vendor, &ctx("rndis", platform, model, 1));
            assert_eq!(
                p.activate.first().map(|s| s.cmd.as_str()),
                Some(*activate),
                "{vendor:?}/{platform} 激活指令"
            );
            assert_eq!(
                p.deactivate.first().map(|s| s.cmd.as_str()),
                Some(*deactivate),
                "{vendor:?}/{platform} 挂断指令"
            );
        }
    }

    /// FM350（广和通 MTK）只认 CGACT：不能给它发 GTRNDIS。
    #[test]
    fn fibocom_mediatek_keeps_cgact_only() {
        let p = plan(Vendor::Fibocom, &ctx("rndis", "mediatek", "fm350-gl", 3));
        assert!(p.pdp_first, "MTK 平台先发 AT+CGACT=1");
        assert!(p.activate.is_empty(), "MTK 平台没有 GTRNDIS");
        assert_eq!(p.deactivate, vec![cgact_deactivate(3)]);
    }

    /// 芯讯通两大平台也是 CGACT；高通平台的 CNMP 保活不写死。
    #[test]
    fn simcom_stays_on_3gpp() {
        for platform in ["asrmicro", "lte", "qualcomm"] {
            let p = plan(Vendor::Simcom, &ctx("rndis", platform, "sim8200", 1));
            assert!(p.pdp_first, "{platform} 先 CGACT");
            assert!(p.activate.is_empty());
            assert!(p.note.contains("CNMP") || platform != "qualcomm");
        }
    }

    /// 平台不明 → 通用路径，绝不猜私有指令。
    #[test]
    fn unknown_platform_never_guesses() {
        let p = plan(Vendor::Quectel, &ctx("rndis", "", "rm520n-gl", 1));
        assert!(p.pdp_first);
        assert!(p.activate.is_empty());
        assert!(p.cgact_fallback);
        assert!(p.note.contains("平台未识别"));
    }

    #[test]
    fn qmi_and_mbim_have_no_at_plan() {
        for kind in ["qmi", "mbim"] {
            let p = plan(Vendor::Quectel, &ctx(kind, "qualcomm", "rm520n-gl", 1));
            assert!(p.activate.is_empty() && p.deactivate.is_empty());
            assert!(
                p.note.contains("主机侧拨号器"),
                "QMI/MBIM 必须明确说明会话不归模组侧"
            );
        }
    }

    /// 挂断必须与激活成对：有激活步骤的厂商，挂断步骤不能为空。
    #[test]
    fn activate_and_deactivate_are_symmetric() {
        let vendors = [
            Vendor::Quectel,
            Vendor::Fibocom,
            Vendor::Huawei,
            Vendor::Meig,
            Vendor::Neoway,
            Vendor::Gosuncn,
            Vendor::Telit,
            Vendor::Simcom,
            Vendor::Openluat,
            Vendor::TDTech,
            Vendor::Unisoc,
            Vendor::Unknown,
        ];
        for v in vendors {
            for platform in ["qualcomm", "unisoc", "mediatek", "hisilicon", "lte", ""] {
                let p = plan(v, &ctx("ecm", platform, "x", 1));
                if has_vendor_step(&p) {
                    // 合宙是唯一的例外：模组自持 PDP，上游明确「断开不动模组」。
                    // 例外必须写在 note 里，不能默默留空。
                    assert!(
                        !p.deactivate.is_empty() || p.note.contains("不向模组发断开指令"),
                        "{v:?}/{platform} 有激活指令却没有挂断指令"
                    );
                }
            }
        }
    }

    /// 私有的「自拨」厂商（合宙 / 华为展锐）不应要求先 CGACT。
    #[test]
    fn self_dialing_vendors_do_not_pre_activate() {
        let p = plan(Vendor::Openluat, &ctx("rndis", "unisoc", "air724ug", 1));
        assert!(!p.pdp_first);
        assert_eq!(p.activate[0].cmd, "AT+RNDISCALL=1");

        let p = plan(Vendor::Huawei, &ctx("ecm", "unisoc", "mt5700m-cn", 3));
        assert!(!p.pdp_first);
        assert_eq!(p.activate[0].cmd, "AT^SETAUTODIAL=1,4");
    }

    /// cid 必须原样进入指令，不能出现固定 1（用户可能用 cid 3）。
    #[test]
    fn cid_is_substituted_everywhere() {
        for cid in [1, 3, 12] {
            let p = plan(Vendor::Meig, &ctx("rndis", "qualcomm", "srm815", cid));
            assert!(p.activate[0].cmd.ends_with(&format!(",{cid}")));
            assert!(p.deactivate[0].cmd.ends_with(&format!(",{cid}")));
        }
    }

    /// 华为展锐自拨：挂断前先查 `AT^SETAUTODIAL?`，没开着才走通用 NDISDUP
    /// （依据 QModem `auto_dial_hang_huawei_unisoc`，modem_dial.sh:1546-1568）。
    #[test]
    fn huawei_unisoc_probes_autodial_before_hangup() {
        let p = plan(Vendor::Huawei, &ctx("ecm", "unisoc", "mt5700m-cn", 3));
        let probe = p.autodial_probe.expect("华为展锐必须带自拨状态查询");
        assert_eq!(probe.query, "AT^SETAUTODIAL?");
        assert_eq!(probe.fallback, "AT^NDISDUP=0,0");

        // 其它厂商不该被这条逻辑波及（含同为自拨的合宙：它不断模组）
        for v in [Vendor::Quectel, Vendor::Fibocom, Vendor::Openluat] {
            for platform in ["qualcomm", "unisoc", "mediatek"] {
                assert!(
                    plan(v, &ctx("ecm", platform, "x", 1))
                        .autodial_probe
                        .is_none(),
                    "{v:?}/{platform} 不该带自拨查询"
                );
            }
        }
    }
}
