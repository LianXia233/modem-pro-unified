//! 网络注册 / 接入信息（`GET /api/network`、`GET /api/network/status`）。
//!
//! 汇总：制式/频段（`AT+QNWINFO`）、注册状态（`AT+CREG?`/`AT+CGREG?`）、
//! 激活上下文（`AT+CGCONTRDP`，给出当前 APN 与 IPv4 地址）。

use serde::Serialize;

use crate::error::Result;

use super::super::at::{csv, extract_field, is_ok};
use super::addr;
use super::band;
use super::modem::Modem;
use super::netselect::{self, NetSelect};
use super::regstate::{self, RegState};
use super::traffic::TrafficView;

/// 简版网络信息（`GET /api/network`）。
#[derive(Debug, Clone, Serialize)]
pub struct NetworkInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<String>,
    pub raw: String,
}

/// 网络状态总览（`GET /api/network/status`）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct NetworkStatus {
    pub modem_id: u32,
    pub queried_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_tech: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tech_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tech: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plmn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mnc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_text: Option<String>,
    /// FDD / TDD（从接入技术串里拆出来；认不出就没有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplex: Option<String>,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub band: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_label: Option<String>,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub earfcn: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    pub cs_reg: RegState,
    pub ps_reg: RegState,
    /// 5GS 域注册状态：5G SA 下 CREG/CGREG 会长期停在 0,0（看着像「未注册」），
    /// 真正的注册状态在 `+C5GREG` 里。**只在 CS/PS 都报未注册时才补查**。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reg_5g: Option<RegState>,
    /// EPS 域注册状态（5GS 也拿不到时的兜底）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reg_eps: Option<RegState>,
    /// 服务小区（取自基站扫描缓存；没扫过就没有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serving: Option<super::cells::CellInfo>,
    pub serving_count: usize,
    pub neighbour_count: usize,
    /// 上面这些缓存数据的扫描时间（没扫过就没有）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scanned_at: String,
    /// 载波聚合的分量载波（PCC + SCC）。当前数据面**只从扫描缓存取**，
    /// 没有实现实时 `AT+QCAINFO` 解析，所以通常为空（前端按空态渲染）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ca: Vec<CcInfo>,
    pub ca_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<String>,
    pub roaming: bool,
    /// 「数据漫游」开关（`[net] allow_roaming`，默认 true）
    pub roam_allowed: bool,
    /// 漫游范围：none / domestic / global / unknown
    #[serde(skip_serializing_if = "String::is_empty")]
    pub roam_scope: String,
    /// 飞行模式（`AT+CFUN?` 的 4 = 开）
    pub airplane: bool,
    /// 模组功能模式原值（1 = 全功能，4 = 飞行模式）
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub cfun: i32,
    /// 固件上报的 QNWINFO 原文（页面「刷新网络信息」的排查框要显示它）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub raw_net_info: String,
    /// 累计流量与实测速率（见 model/traffic.rs；不支持计量时 `supported=false`）
    pub traffic: Option<TrafficView>,
}

/// 一个分量载波（载波聚合）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CcInfo {
    /// PCC / SCC（空 = 未标注）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role: String,
    pub band: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub band_label: String,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub bandwidth: i32,
    #[serde(skip_serializing_if = "is_zero_i32")]
    pub earfcn: i32,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

/// 原始应答按行（去空行）用 `|` 连成一行（`GET /api/netinfo` 的 `data` 口径）。
fn join_lines(resp: &str) -> String {
    super::modem::split_at_lines(resp).join("|")
}

impl Modem {
    /// 简版网络信息（运营商 + 接入技术）。
    pub fn network(&self) -> Result<NetworkInfo> {
        let raw = self.with_at(|p| p.command("AT+COPS?", self.cmd_timeout))?;
        let ns = netselect::parse(&raw);
        let operator = ns.operator.clone().or_else(|| ns.plmn.clone());
        Ok(NetworkInfo {
            operator,
            act: ns.act.map(|a| a.to_string()),
            raw,
        })
    }

    /// 网络接入信息（`GET /api/netinfo`）：原样回厂商指令的应答行（`|` 连接）。
    ///
    /// 厂商声明「本机不提供」时指令为空串：直接返回空串，不往串口发空指令
    /// （空指令白占一个往返，共享慢线上要 2 秒）。
    pub fn net_info(&self) -> Result<String> {
        let cmd = self.vendor().net_info_cmd();
        if cmd.trim().is_empty() {
            return Ok(String::new());
        }
        let resp = self.with_at(|p| {
            let (text, _) = p.command_cancellable(cmd, self.cmd_timeout, &|| false)?;
            Ok(text)
        })?;
        Ok(join_lines(&resp))
    }

    /// 网络状态总览。
    ///
    /// `allow_roaming` 来自 `[net] allow_roaming`（页面「漫游」提示要用它判断
    /// 「国际漫游 + 开关关闭 = 数据链路该停」）。
    pub fn network_status(&self, allow_roaming: bool) -> NetworkStatus {
        let qnw = self
            .with_at(|p| p.command("AT+QNWINFO", self.cmd_timeout))
            .unwrap_or_default();
        let info = parse_qnwinfo(&qnw);

        // `AT+QNWINFO` 是厂商私有指令，广和通 MTK 机型（FM350/RW350）直接回
        // ERROR，制式 / PLMN / 频段全空。此时用 3GPP 通用的 `AT+COPS?` 兜底：
        // format=2 只给 PLMN，但足够推出 mcc/mnc 与注册制式（`<AcT>`），
        // `operator` 也能拿到中文全名。
        //
        // 只在 QNWINFO 没给出 PLMN 时才多发这一条 —— 指令可用的厂商不增加
        // 往返。`AT+CSPPLMN?` 在该机型同样回 ERROR，没有第二条兜底路。
        let cops = if info.plmn.is_none() {
            self.with_at(|p| p.command("AT+COPS?", self.cmd_timeout))
                .ok()
                .map(|raw| netselect::parse(&raw))
                .filter(|ns| ns.plmn.is_some() || ns.operator.is_some() || ns.act.is_some())
        } else {
            None
        };
        let f = merge_network(&info, cops.as_ref());

        let cs_reg = self
            .with_at(|p| p.command("AT+CREG?", self.cmd_timeout))
            .map(|raw| regstate::parse(&raw, "+CREG"))
            .unwrap_or_default();
        let ps_reg = self
            .with_at(|p| p.command("AT+CGREG?", self.cmd_timeout))
            .map(|raw| regstate::parse(&raw, "+CGREG"))
            .unwrap_or_default();

        let (apn, ipv4, ipv6) = self.query_active_context();
        let roaming = ps_reg.roaming;

        // 5GS / EPS 兜底：只在 CS/PS 都说未注册时补查（平时正常的模组不多发指令）
        let (mut reg_5g, mut reg_eps) = (None, None);
        if !cs_reg.registered && !ps_reg.registered {
            let raw = self
                .with_at(|p| p.command("AT+C5GREG?", self.cmd_timeout))
                .unwrap_or_default();
            if raw.contains("+C5GREG") {
                let st = regstate::parse(&raw, "+C5GREG");
                if st.stat > 0 {
                    reg_5g = Some(st);
                }
            }
            if reg_5g.is_none() {
                let raw = self
                    .with_at(|p| p.command("AT+CEREG?", self.cmd_timeout))
                    .unwrap_or_default();
                if raw.contains("+CEREG") {
                    let st = regstate::parse(&raw, "+CEREG");
                    if st.stat > 0 {
                        reg_eps = Some(st);
                    }
                }
            }
        }

        // 服务小区 / 数量：只读扫描缓存，**不触发扫描**（扫描要好几秒）
        let cells = self.cells_cached();
        let serving = cells.serving.first().cloned();

        // 载波聚合：实时查一次（此前 `ca` / `ca_active` 是恒空占位）。
        // 指令按厂商挑，解析见 model/ca.rs。手册标的耗时 < 1s，
        // 相对本函数已有的 6~8 条 AT 是可接受的增量；查询失败只是这两项为空，
        // 不会连累其余字段。
        let ca = self.ca_status();

        // 漫游范围：不漫游时不用多读 IMSI
        let roam_scope = if roaming {
            let imsi = self
                .with_at(|p| p.command("AT+CIMI", self.cmd_timeout))
                .map(|raw| super::siminfo::parse_imsi(&raw))
                .unwrap_or(None);
            super::siminfo::roam_state(true, imsi.as_deref(), f.plmn.as_deref()).scope
        } else {
            "none".to_string()
        };

        // 飞行模式：CFUN 原值同时给页面（4 = 飞行模式）
        let air = self.airplane();
        let duplex = f.net_tech.as_deref().map(duplex_of);
        let cfun = if air.supported {
            if air.enabled {
                4
            } else {
                1
            }
        } else {
            0
        };

        NetworkStatus {
            modem_id: self.id,
            queried_at: crate::logging::timestamp(),
            net_tech: f.net_tech,
            tech_label: f.tech_label,
            tech: f.tech,
            mcc: f.mcc,
            mnc: f.mnc,
            plmn: f.plmn,
            band_text: f.band_text,
            duplex,
            band: f.band.unwrap_or(0),
            band_label: f.band_label,
            earfcn: f.earfcn,
            operator: f.operator,
            cs_reg,
            ps_reg,
            reg_5g,
            reg_eps,
            serving,
            serving_count: cells.serving_count,
            neighbour_count: cells.neighbour_count,
            // 没扫过就是空串（cells_cached 的空结果不带时间戳）
            scanned_at: self.cell_scan_at().unwrap_or_default(),
            // 实时 CA 查询（此前这两项是恒空占位）：指令按厂商挑，
            // 解析见 model/ca.rs（广和通那段照官方手册 §11.1.16）
            ca: ca.as_ref().map(|c| c.ccs.clone()).unwrap_or_default(),
            ca_active: ca.as_ref().map(|c| c.ca_active).unwrap_or(false),
            apn,
            ipv4,
            ipv6,
            roaming,
            roam_allowed: allow_roaming,
            roam_scope,
            airplane: air.enabled,
            cfun,
            raw_net_info: raw_lines(&qnw),
            traffic: Some(self.traffic_view()),
        }
    }

    /// 当前激活上下文的 APN / 地址（`AT+CGCONTRDP`）。
    fn query_active_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        let resp = self
            .with_at(|p| p.command("AT+CGCONTRDP", self.cmd_timeout))
            .unwrap_or_default();
        parse_active_context(&resp)
    }
}

/// 从 `AT+CGCONTRDP` 应答里取「当前激活上下文」的 (APN, IPv4, IPv6)。
///
/// 这里是单包测试的落点：地址字段的下标、点分 16 段的 IPv6、按 cid 归并
/// 三件事都有实机夹具（见 `tests`）。
pub fn parse_active_context(resp: &str) -> (Option<String>, Option<String>, Option<String>) {
    if !is_ok(resp) {
        return (None, None, None);
    }

    /// 一个上下文归并后的 APN / 地址。
    #[derive(Default)]
    struct Ctx {
        apn: Option<String>,
        v4: Option<String>,
        v6: Option<String>,
    }

    // 按 cid 归并：实测广和通 MTK 平台会把**同一个**上下文的 IPv4 与
    // IPv6 拆成两行报（cid 相同、地址字段一个回点分 IPv4、一个回 16 段
    // 点分的 IPv6），只取首行就永远看不到 IPv6。
    let mut merged: Vec<(String, Ctx)> = Vec::new();
    for field in super::super::at::extract_all(resp, "+CGCONTRDP") {
        let f = csv(&field);
        let cid = f.first().cloned().unwrap_or_default();
        if !merged.iter().any(|(c, _)| *c == cid) {
            merged.push((cid.clone(), Ctx::default()));
        }
        let ctx = &mut merged.iter_mut().find(|(c, _)| *c == cid).unwrap().1;
        if ctx.apn.is_none() {
            ctx.apn = f.get(2).cloned().filter(|s| !s.is_empty());
        }
        // 地址字段从第 3 位起扫，找到**第一个**含地址的字段就停：再往后是
        // DNS（也是合法 IPv4/IPv6），再当地址收就会把 DNS 显示成「本机地址」。
        //
        // 但要把「紧邻的下一个字段」也看一眼：实测两种固件会把 v4 / v6
        // 拆开报 —— 广和通 MTK 平台是同一 cid 拆两行，移远等是同一行里
        // 相邻两列。只认第一个字段就会漏掉另一个家族。
        //
        // 地址字段的下标**不能写死**：3GPP 规定它在第 3 位，但实测
        // 广和通 MTK 平台把它放到了第 5 位（f[3] / f[4] 是空串），
        // 照 3GPP 取下标只会得到空串 —— 地址（含 IPv6）整条读不到。
        let Some(at) = f.iter().skip(3).position(|raw| {
            let (a, b) = split_addr(raw);
            a.is_some() || b.is_some()
        }) else {
            continue;
        };
        let take = |ctx: &mut Ctx, raw: &str| {
            let (a, b) = split_addr(raw);
            if ctx.v4.is_none() {
                ctx.v4 = a;
            }
            if ctx.v6.is_none() {
                ctx.v6 = b;
            }
        };
        take(ctx, &f[at + 3]);
        if let Some(next) = f.get(at + 4) {
            take(ctx, next);
        }
    }

    // 取第一个「有数据」的上下文：`AT+CGCONTRDP` 对未激活的上下文回空
    // 地址字段，所以这是可用的近似判据。纯 IPv6 上下文（无 APN、无 v4）
    // 也因此在列 —— 原先会被整条跳过，网络卡片上「本机地址」永远空着。
    for (_, ctx) in &merged {
        if ctx.apn.is_some() || ctx.v4.is_some() || ctx.v6.is_some() {
            return (ctx.apn.clone(), ctx.v4.clone(), ctx.v6.clone());
        }
    }
    (None, None, None)
}

/// `+QNWINFO` 解析结果。
#[derive(Debug, Clone, Default)]
pub struct QnwInfo {
    pub tech: Option<String>,
    pub plmn: Option<String>,
    pub mcc: Option<String>,
    pub mnc: Option<String>,
    pub band_text: Option<String>,
    pub band: Option<i32>,
    pub earfcn: i32,
}

/// 解析 `AT+QNWINFO`：
/// `+QNWINFO: "TDD NR5G","46011","NR5G BAND 78",627264`（PLMN 可能不带引号）。
pub fn parse_qnwinfo(resp: &str) -> QnwInfo {
    let mut out = QnwInfo::default();
    let Some(field) = extract_field(resp, "+QNWINFO") else {
        return out;
    };
    let f = csv(&field);
    out.tech = f.first().filter(|s| !s.is_empty()).cloned();
    if let Some(plmn) = f.get(1).filter(|s| !s.is_empty()) {
        out.mcc = Some(plmn.chars().take(3).collect());
        out.mnc = Some(plmn.chars().skip(3).collect());
        out.plmn = Some(plmn.clone());
    }
    out.band_text = f.get(2).filter(|s| !s.is_empty()).cloned();
    out.band = out.band_text.as_deref().and_then(parse_band);
    out.earfcn = f.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    out
}

/// AT 应答 → 一行（去掉命令回显与空行，用 ` | ` 连接）。
fn raw_lines(resp: &str) -> String {
    let mut lines: Vec<&str> = resp
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("AT+"))
        .collect();
    if lines.len() > 1 {
        lines.retain(|l| *l != "OK");
    }
    lines.join(" | ")
}

/// 从接入技术串里取双工方式（`TDD NR5G` → `TDD`）。
fn duplex_of(net_tech: &str) -> String {
    let u = net_tech.to_ascii_uppercase();
    if u.contains("TDD") {
        "TDD".to_string()
    } else if u.contains("FDD") {
        "FDD".to_string()
    } else {
        String::new()
    }
}

/// PLMN → MCC（前三位，国家码）。
fn plmn_mcc(plmn: &str) -> String {
    plmn.chars().take(3).collect()
}

/// PLMN → MNC（国家码之后的网络码，2~3 位）。
fn plmn_mnc(plmn: &str) -> String {
    plmn.chars().skip(3).collect()
}

/// QNWINFO 与 COPS 兜底解合并后的网络字段（[`Modem::network_status`] 的取值来源）。
#[derive(Debug, Clone, Default, PartialEq)]
struct NetFields {
    net_tech: Option<String>,
    tech_label: Option<String>,
    tech: Option<String>,
    plmn: Option<String>,
    mcc: Option<String>,
    mnc: Option<String>,
    operator: Option<String>,
    band_text: Option<String>,
    band: Option<i32>,
    band_label: Option<String>,
    earfcn: i32,
}

/// 合并 `AT+QNWINFO` 与（可选的）`AT+COPS?` 兜底结果。
///
/// 优先级：QNWINFO 给出的原始字段优先；它没有的（PLMN / 制式）才用 COPS 补。
/// 纯函数 —— 「QNWINFO 可用时不多发 COPS」的取舍在调用方（只有
/// `info.plmn.is_none()` 才发），这里只负责合并，单测直接覆盖。
fn merge_network(info: &QnwInfo, cops: Option<&NetSelect>) -> NetFields {
    let plmn = info
        .plmn
        .clone()
        .or_else(|| cops.and_then(|ns| ns.plmn.clone()));
    let mcc = info.mcc.clone().or_else(|| plmn.as_deref().map(plmn_mcc));
    let mnc = info.mnc.clone().or_else(|| plmn.as_deref().map(plmn_mnc));
    let tech = info.tech.clone().or_else(|| {
        cops.and_then(|ns| ns.act)
            .and_then(cops_act_tech)
            .map(str::to_string)
    });
    let operator = operator_display(
        mcc.as_deref().unwrap_or_default(),
        mnc.as_deref().unwrap_or_default(),
        cops.and_then(|ns| ns.operator.as_deref()),
        plmn.as_deref(),
    );
    NetFields {
        net_tech: info.tech.clone(),
        tech_label: tech.as_deref().map(tech_label),
        // 归一化后是空串（认不出的制式）时不给字段，别往前端送 ""
        tech: tech
            .as_deref()
            .map(normalize_tech)
            .filter(|t| !t.is_empty()),
        plmn,
        mcc,
        mnc,
        operator,
        band_text: info.band_text.clone(),
        band: info.band,
        band_label: info
            .band
            .map(|b| band_label(tech.as_deref().unwrap_or_default(), b)),
        earfcn: info.earfcn,
    }
}

/// `operator` 三级取值：内置 MCC/MNC → 运营商全名 → 模组上报名 → 原始 PLMN。
///
/// * 第一级是 [`super::cells::operator_name`]（四家中国运营商；认不出返回空串，
///   此时自然落到下一级）；
/// * 第二级只有 `AT+COPS?` 用 format=0（长名）时才拿得到，且已被
///   [`super::netselect::parse`] 滤掉 `????` 占位；
/// * 第三级是 format=2（纯 PLMN，本机实测就是这种）下的最后兜底 —— 至少让
///   网络卡片显示 `46011`，而不是一整行空。
fn operator_display(
    mcc: &str,
    mnc: &str,
    module_name: Option<&str>,
    plmn: Option<&str>,
) -> Option<String> {
    let known = super::cells::operator_name(mcc, mnc);
    if !known.is_empty() {
        return Some(known);
    }
    if let Some(name) = module_name.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(name.to_string());
    }
    plmn.map(str::to_string)
}

/// `AT+COPS?` 的 `<AcT>` → 归一化制式（认不出返回 `None`）。
///
/// 按 3GPP TS 27.007：`7`=E-UTRAN(LTE)、`11`=NG-RAN(NR)、`2/4/5/6`=UTRAN(WCDMA)、
/// `0/1/3`=GSM；`8~10` 是 CDMA/EVDO 与 IoT 制式，本包不显示。
///
/// ⚠️ **别照 FM350 AT 手册 v2.2 的表写**：那张表是 2021 年的，NG-RAN 还没入表，
/// 它把 `11/12` 写成 eMTC/NB-IoT。实机（固件 <固件版本>）注册在
/// NR n78 时 `AT+COPS?` 回 `+COPS:0,2,"46011",11` —— 此固件 `11` 就是 NG-RAN。
///
/// 已知限制：NR NSA 下模组常以 LTE 注册，`<AcT>` 报 `7`，这里只能照报，
/// 不猜「其实是 5G」。
fn cops_act_tech(act: i32) -> Option<&'static str> {
    match act {
        0 | 1 | 3 => Some("gsm"),
        2 | 4 | 5 | 6 => Some("wcdma"),
        7 => Some("lte"),
        11 | 12 => Some("nr"),
        _ => None,
    }
}

/// 从 `"NR5G BAND 78"` / `"LTE BAND 3"` 取频段号。
pub fn parse_band(text: &str) -> Option<i32> {
    let upper = text.to_ascii_uppercase();
    let idx = upper.find("BAND")?;
    let rest = upper[idx + 4..].trim();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// 制式友好名。
pub fn tech_label(tech: &str) -> String {
    let upper = tech.to_ascii_uppercase();
    let duplex = if upper.contains("TDD") {
        "TDD"
    } else if upper.contains("FDD") {
        "FDD"
    } else {
        ""
    };
    let base = if upper.contains("NR") {
        "5G NR"
    } else if upper.contains("LTE") {
        "4G LTE"
    } else if upper.contains("WCDMA") || upper.contains("UMTS") || upper.contains("HSDPA") {
        "3G WCDMA"
    } else if upper.contains("GSM") || upper.contains("GPRS") || upper.contains("EDGE") {
        "2G GSM"
    } else {
        return tech.to_string();
    };
    if duplex.is_empty() {
        base.to_string()
    } else {
        format!("{base} ({duplex})")
    }
}

/// 归一化接入技术（`nr` / `lte` / `wcdma` / `gsm`）。
pub fn normalize_tech(tech: &str) -> String {
    let upper = tech.to_ascii_uppercase();
    if upper.contains("NR") {
        "nr"
    } else if upper.contains("LTE") {
        "lte"
    } else if upper.contains("WCDMA") || upper.contains("UMTS") {
        "wcdma"
    } else if upper.contains("GSM") {
        "gsm"
    } else {
        ""
    }
    .to_string()
}

/// 频段短标签：NR 用 `n78`，LTE/WCDMA 用 `B3`。
///
/// `tech` 既接受归一化值（`nr` / `lte` / `wcdma` / `gsm`），也接受模组原样回值
/// （`TDD NR5G` / `FDD LTE`）；`band <= 0` 返回空串。
pub fn band_label(tech: &str, band: i32) -> String {
    band::band_label(tech, band)
}

/// 把 `AT+CGCONTRDP` 的 `<ip addr>` 字段拆成 (v4, v6)。
///
/// 实机见到的三种写法：
///
/// ```text
/// +CGCONTRDP: 1,,"cmiot5g","","","10.42.4.240","","2001:db8:1:2::8"   # 双栈，两个字段
/// +CGCONTRDP: 1,,"cmiot5g","","","10.42.4.240 2001:db8:1:2::8",...  # 双栈，空格分隔
/// +CGCONTRDP: 1,,"cmiot5g","","","32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8",...
/// ```
///
/// 两个家族都过 [`addr`] 的归一化，与 `AT+CGPADDR` 那条路（`dial::split_addrs`）
/// **同一套判据**：
///   * v6 做结构校验、剥 zone id / 方括号、拒占位与不可信地址；
///   * v4 校验四段 0~255、拒全零；
///   * 16 段点分十进制按 IPv6 解（广和通 / 移远部分固件的怪写法）。
///
/// ⚠️ 这里原先只按 `contains(':')` / `contains('.')` 分家族、零校验，
/// 后果是把 `0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1`（点分的 `::1` 占位）当成
/// **IPv4** 显示到页面上，也会让 `gg::1` 这类脏值原样进接口配置。
pub fn split_addr(addr: &str) -> (Option<String>, Option<String>) {
    let mut v4 = None;
    let mut v6 = None;
    for part in addr.split_whitespace() {
        // 冒号写法：统一归一化（结构 + 占位 + zone id / 方括号）
        if part.contains(':') {
            if v6.is_none() {
                v6 = addr::normalize_ipv6(part);
            }
            continue;
        }
        let segs: Vec<&str> = part.split('.').collect();
        match segs.len() {
            // 4 段 = IPv4
            4 if v4.is_none() => {
                if addr::is_valid_ipv4(part) {
                    v4 = Some(part.to_string());
                }
            }
            // 16 段 = 按字节展开的 IPv6（不是 IPv4，别按点分数段误判）
            16 if v6.is_none() => v6 = addr::normalize_ipv6(part),
            _ => {}
        }
    }
    (v4, v6)
}

#[cfg(test)]
mod tests {
    use super::netselect;
    use super::{
        band_label, cops_act_tech, merge_network, operator_display, parse_active_context,
        parse_band, parse_qnwinfo, split_addr, tech_label,
    };

    // ---------- AT+CGCONTRDP 的 <ip addr> 拆解 ----------

    #[test]
    fn split_addr_single_v4() {
        assert_eq!(
            split_addr("10.42.4.240"),
            (Some("10.42.4.240".into()), None)
        );
    }

    #[test]
    fn split_addr_dual_stack_space_separated() {
        // 双栈有两种写法：两个字段各一个，或一个字段里空格分隔
        assert_eq!(
            split_addr("10.42.4.240 2001:db8:1:2::8"),
            (Some("10.42.4.240".into()), Some("2001:db8:1:2::8".into()))
        );
    }

    #[test]
    fn split_addr_v6_only() {
        assert_eq!(
            split_addr("2001:db8:1:2::8"),
            (None, Some("2001:db8:1:2::8".into()))
        );
    }

    #[test]
    fn split_addr_rejects_untrustworthy_addresses() {
        // 点分的 16 段十进制是 IPv6，不是 IPv4 —— 原先按 contains('.') 分类，
        // 会把广和通 MTK 平台的 ::1 占位当成 IPv4 显示到页面上
        assert_eq!(split_addr("0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1"), (None, None));
        assert_eq!(
            split_addr("0.0.0.0.0.0.0.0.24.217.217.24.12.29.50.199"),
            (None, None)
        );
        // 冒号占位与脏值
        assert_eq!(split_addr("::1"), (None, None));
        assert_eq!(split_addr("::"), (None, None));
        assert_eq!(split_addr("gg::1"), (None, None));
        assert_eq!(split_addr("2001::8::1"), (None, None));
        // 非法 IPv4
        assert_eq!(split_addr("999.1.1.1"), (None, None));
        assert_eq!(split_addr("0.0.0.0"), (None, None));
    }

    #[test]
    fn split_addr_strips_zone_id_and_brackets() {
        assert_eq!(split_addr("fe80::1%eth2"), (None, Some("fe80::1".into())));
        assert_eq!(
            split_addr("[2001:db8::8]"),
            (None, Some("2001:db8::8".into()))
        );
    }

    #[test]
    fn split_addr_decodes_dotted_byte_form() {
        // 广和通 / 移远部分固件把 IPv6 按字节展开成 16 段十进制
        assert_eq!(
            split_addr("32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8"),
            (None, Some("2001:db8:1:2::8".into()))
        );
    }

    #[test]
    fn split_addr_takes_first_of_each_family() {
        // 同一家族出现多次时取第一个合法的，不被后面的脏值覆盖
        assert_eq!(
            split_addr("10.42.4.240 gg::1 2001:db8::8 ::1"),
            (Some("10.42.4.240".into()), Some("2001:db8::8".into()))
        );
    }

    // ---------- 激活上下文：APN / 地址 ----------

    /// 实机（FM350-GL，中国电信）原始应答：同一个 cid 报两行，
    /// 一行 IPv4、一行把 IPv6 展开成 16 段点分十进制，地址在第 5 个字段。
    const FIBOCOM_MTK_CGCONTRDP: &str = concat!(
        "AT+CGCONTRDP\r\n",
        "+CGCONTRDP: 0,5,\"ctnet.MNC011.MCC460.GPRS\",\"\",\"\",\"203.0.113.53\",",
        "\"203.0.113.54\",\"\",\"\",0,,0,,,,,,,,,,,,0\r\n",
        "+CGCONTRDP: 0,5,\"ctnet.MNC011.MCC460.GPRS\",\"\",\"\",",
        "\"32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.1\",",
        "\"32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.2\",\"\",\"\",0,,0,,,,,,,,,,,,0\r\n",
        "OK\r\n"
    );

    #[test]
    fn active_context_merges_v4_and_v6_rows() {
        // 归并后两行都要取到：IPv4 一行、IPv6（点分 16 段）一行
        let (apn, v4, v6) = parse_active_context(FIBOCOM_MTK_CGCONTRDP);
        assert_eq!(apn.as_deref(), Some("ctnet.MNC011.MCC460.GPRS"));
        assert_eq!(v4.as_deref(), Some("203.0.113.53"));
        // 32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.1 → 2001:db8::1
        assert_eq!(v6.as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn active_context_reads_3gpp_style_address_field() {
        // 3GPP 规定地址在第 3 位（移远/广和通其它固件常见写法）
        let resp = "+CGCONTRDP: 1,,\"cmiot5g\",\"10.42.4.240\",\"2001:db8:1:2::8\",\"\",\"\"\r\nOK";
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn.as_deref(), Some("cmiot5g"));
        assert_eq!(v4.as_deref(), Some("10.42.4.240"));
        assert_eq!(v6.as_deref(), Some("2001:db8:1:2::8"));
    }

    #[test]
    fn active_context_v6_only_context_is_not_skipped() {
        // 纯 IPv6 上下文：无 APN、无 v4。原先的选中判据含 v4，会整条跳过
        let resp = "+CGCONTRDP: 1,,\"\",\"\",\"2001:db8:1:2::8\"\r\nOK";
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn, None);
        assert_eq!(v4, None);
        assert_eq!(v6.as_deref(), Some("2001:db8:1:2::8"));
    }

    #[test]
    fn active_context_drops_untrustworthy_addresses() {
        // 点分的 ::1 占位：既不是合法 IPv4，也不该当成 IPv6 显示
        let resp = "+CGCONTRDP: 1,,\"ctnet\",\"\",\"\",\"0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1\"\r\nOK";
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn.as_deref(), Some("ctnet"));
        assert_eq!(v4, None);
        assert_eq!(v6, None);
    }

    #[test]
    fn active_context_ignores_error_and_empty() {
        assert_eq!(
            parse_active_context("AT+CGCONTRDP\r\nERROR\r\n"),
            (None, None, None)
        );
        assert_eq!(parse_active_context(""), (None, None, None));
        // 所有上下文都没数据时也返回三空
        let resp = "+CGCONTRDP: 1,,\"\",\"\",\"\",\"\"\r\nOK";
        assert_eq!(parse_active_context(resp), (None, None, None));
    }

    #[test]
    fn active_context_keeps_distinct_cids_separate() {
        // 两个不同 cid 各自一行：只取第一个有数据的，不会把 cid 2 的地址
        // 混进 cid 1 的结果里
        let resp = concat!(
            "+CGCONTRDP: 1,,\"apn-a\",\"\",\"\",\"10.0.0.2\"\r\n",
            "+CGCONTRDP: 2,,\"apn-b\",\"\",\"\",\"10.0.0.9\"\r\n",
            "OK\r\n"
        );
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn.as_deref(), Some("apn-a"));
        assert_eq!(v4.as_deref(), Some("10.0.0.2"));
        assert_eq!(v6, None);
    }

    #[test]
    fn qnwinfo_quoted_plmn() {
        let info = parse_qnwinfo("+QNWINFO: \"TDD NR5G\",\"46011\",\"NR5G BAND 78\",627264\r\nOK");
        assert_eq!(info.tech.as_deref(), Some("TDD NR5G"));
        assert_eq!(info.plmn.as_deref(), Some("46011"));
        assert_eq!(info.mcc.as_deref(), Some("460"));
        assert_eq!(info.mnc.as_deref(), Some("11"));
        assert_eq!(info.band, Some(78));
        assert_eq!(info.earfcn, 627264);
    }

    #[test]
    fn qnwinfo_bare_plmn() {
        let info = parse_qnwinfo("+QNWINFO: \"FDD LTE\",46011,\"LTE BAND 3\",1850\r\nOK");
        assert_eq!(info.plmn.as_deref(), Some("46011"));
        assert_eq!(info.band, Some(3));
    }

    #[test]
    fn labels_and_bands() {
        assert_eq!(parse_band("NR5G BAND 78"), Some(78));
        // NR 前缀 n，LTE 前缀 B —— 以前 LTE 会被错标成 n3
        assert_eq!(band_label("TDD NR5G", 78), "n78");
        assert_eq!(band_label("FDD LTE", 3), "B3");
        assert_eq!(band_label("lte", 0), "");
        assert_eq!(tech_label("TDD NR5G"), "5G NR (TDD)");
        assert_eq!(tech_label("FDD LTE"), "4G LTE (FDD)");
    }

    #[test]
    fn addr_split() {
        let (v4, v6) = split_addr("10.0.0.2 2001:db8::1");
        assert_eq!(v4.as_deref(), Some("10.0.0.2"));
        assert_eq!(v6.as_deref(), Some("2001:db8::1"));
    }

    // ---------- AT+COPS? 兜底与 operator 取值 ----------

    #[test]
    fn cops_act_tech_maps_3gpp_act() {
        // 实机（FM350-GL）注册在 NR n78 时回 +COPS:0,2,"46011",11 —— 11 就是 NG-RAN
        assert_eq!(cops_act_tech(11), Some("nr"));
        assert_eq!(cops_act_tech(12), Some("nr"));
        assert_eq!(cops_act_tech(7), Some("lte"));
        for a in [2, 4, 5, 6] {
            assert_eq!(cops_act_tech(a), Some("wcdma"));
        }
        for a in [0, 1, 3] {
            assert_eq!(cops_act_tech(a), Some("gsm"));
        }
        // CDMA / EVDO / IoT 制式本包不显示；没上报（None）也不猜
        for a in [8, 9, 10, 13, 99] {
            assert_eq!(cops_act_tech(a), None);
        }
    }

    #[test]
    fn operator_display_prefers_known_carrier_table() {
        // 第一级：内置 MCC/MNC 表，模组上报名与 PLMN 都靠后
        assert_eq!(
            operator_display("460", "11", Some("China Telecom"), Some("46011")).as_deref(),
            Some("中国电信")
        );
        // 表里没有的 MCC/MNC：用模组上报名（format=0 才拿得到）
        assert_eq!(
            operator_display("310", "260", Some("T-Mobile"), Some("310260")).as_deref(),
            Some("T-Mobile")
        );
        // 上报名也没有（format=2 只给 PLMN）：回落原始 PLMN
        assert_eq!(
            operator_display("310", "260", None, Some("310260")).as_deref(),
            Some("310260")
        );
        // 空白上报名视作没有
        assert_eq!(operator_display("999", "99", Some("   "), None), None);
        // 一路都没有：字段不出现
        assert_eq!(operator_display("", "", None, None), None);
    }

    #[test]
    fn merge_network_falls_back_to_cops() {
        // 实机：AT+QNWINFO 回 ERROR（解出来全空），AT+COPS? 是 format=2
        let info = parse_qnwinfo("AT+QNWINFO\r\nERROR\r\n");
        let cops = netselect::parse("+COPS: 0,2,\"46011\",11\r\nOK");
        let f = merge_network(&info, Some(&cops));
        assert_eq!(f.plmn.as_deref(), Some("46011"));
        assert_eq!(f.mcc.as_deref(), Some("460"));
        assert_eq!(f.mnc.as_deref(), Some("11"));
        assert_eq!(f.tech.as_deref(), Some("nr"));
        assert_eq!(f.tech_label.as_deref(), Some("5G NR"));
        assert_eq!(f.operator.as_deref(), Some("中国电信"));
        // QNWINFO 没给的原始串不造：net_tech / band / earfcn 一律空着
        assert_eq!(f.net_tech, None);
        assert_eq!(f.band, None);
        assert_eq!(f.band_label, None);
        assert_eq!(f.earfcn, 0);
    }

    #[test]
    fn merge_network_prefers_qnwinfo() {
        // QNWINFO 可用时它的原始串优先，COPS 只补空缺（也不覆盖已有值）
        let info = parse_qnwinfo("+QNWINFO: \"FDD LTE\",46001,\"LTE BAND 3\",1850\r\nOK");
        let cops = netselect::parse("+COPS: 0,2,\"46011\",11\r\nOK");
        let f = merge_network(&info, Some(&cops));
        assert_eq!(f.net_tech.as_deref(), Some("FDD LTE"));
        assert_eq!(f.tech.as_deref(), Some("lte"));
        assert_eq!(f.tech_label.as_deref(), Some("4G LTE (FDD)"));
        assert_eq!(f.plmn.as_deref(), Some("46001"));
        assert_eq!(f.mcc.as_deref(), Some("460"));
        assert_eq!(f.mnc.as_deref(), Some("01"));
        assert_eq!(f.operator.as_deref(), Some("中国联通"));
        assert_eq!(f.band, Some(3));
        assert_eq!(f.band_label.as_deref(), Some("B3"));
        assert_eq!(f.earfcn, 1850);
    }

    #[test]
    fn merge_network_band_label_uses_resolved_tech() {
        // 古怪固件：给了频段却没给 PLMN（于是走 COPS 兜底补制式）——
        // band_label 的 n/B 前缀要跟着兜底来的制式走，不能退成 B78
        let info = parse_qnwinfo("+QNWINFO: ,,\"NR5G BAND 78\",627264\r\nOK");
        let cops = netselect::parse("+COPS: 0,2,\"46011\",11\r\nOK");
        let f = merge_network(&info, Some(&cops));
        assert_eq!(f.band, Some(78));
        assert_eq!(f.band_label.as_deref(), Some("n78"));
        assert_eq!(f.tech.as_deref(), Some("nr"));
    }

    #[test]
    fn merge_network_without_any_source() {
        // 两条路都没有（无卡 / 模组不响应）：什么都不造
        let info = parse_qnwinfo("AT+QNWINFO\r\nERROR\r\n");
        let f = merge_network(&info, None);
        assert_eq!(f.plmn, None);
        assert_eq!(f.mcc, None);
        assert_eq!(f.mnc, None);
        assert_eq!(f.operator, None);
        assert_eq!(f.tech, None);
        assert_eq!(f.tech_label, None);
        assert_eq!(f.net_tech, None);
    }
}
