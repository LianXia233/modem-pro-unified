//! 网络注册 / 接入信息（`GET /api/network`、`GET /api/network/status`）。
//!
//! 汇总六路数据，口径与 Go `module/kernel/net.go:35` 的 `NetworkStatus` 对齐：
//!
//! | 字段 | 来源 | 说明 |
//! | --- | --- | --- |
//! | 制式 / 频段 | `AT+QNWINFO`（移远私有） | 不支持时回落 `AT+COPS?`（只给 PLMN 与 `<AcT>`） |
//! | 注册状态 | `AT+CREG?`（CS）/ `AT+CGREG?`（PS） | 两者都未注册才补查 `AT+C5GREG?` → `AT+CEREG?` |
//! | 当前 APN / 地址 | `AT+CGCONTRDP` | 按 cid 归并（同一上下文的 v4 / v6 可能被拆成两行） |
//! | 服务小区 | 扫描缓存，没扫过才实时补一条 | 扫描要好几秒，不放在本路径触发 |
//! | 载波聚合 | 按厂商挑指令实时查（见 `model/ca.rs`） | |
//! | 飞行模式 | `AT+CFUN?` | 4 = 飞行模式 |

use serde::Serialize;

use crate::error::Result;

use super::super::at::{csv, extract_field, is_ok, Prio};
use super::addr;
use super::band;
use super::dial::DEFAULT_DATA_CID;
use super::modem::Modem;
use super::netselect::{self, NetSelect};
use super::regstate::{self, RegState};
use super::traffic::TrafficView;

/// 简版网络信息（`GET /api/network`）。
#[derive(Debug, Clone, Serialize)]
pub struct NetworkInfo {
    /// 运营商（模组上报名为空时退回 PLMN）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// `AT+COPS?` 的接入技术原值（如 `"NR5G"`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<String>,
    /// `AT+COPS?` 的原始应答（排查框要显示它）
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
    /// 归一化的接入技术：`nr` / `lte` / `wcdma` / `gsm`（认不出就没有这个字段）。
    ///
    /// 取值来自 QNWINFO 的制式串，或 `AT+COPS?` 的 `<AcT>`（见 [`cops_act_tech`]）。
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
    /// CS（电路域，`AT+CREG?`）与 PS（分组域，`AT+CGREG?`）的注册状态。
    ///
    /// `stat` 按 3GPP TS 27.007：`0` 未注册未在搜索、`1` 已注册本地、`2` 搜索中、
    /// `3` 注册被拒绝、`4` 未知、`5` 已注册且漫游（见 `model/regstate.rs`）。
    /// 数据业务看 `ps_reg`。
    pub cs_reg: RegState,
    pub ps_reg: RegState,
    /// 5GS 域注册状态：5G SA 下 CREG/CGREG 会长期停在 0,0（看着像「未注册」），
    /// 真正的注册状态在 `+C5GREG` 里。**只在 CS/PS 都报未注册时才补查**
    ///（依据 Go `module/kernel/net.go:667-678`，实测 RM502Q-GL 上只有它有值）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reg_5g: Option<RegState>,
    /// EPS 域注册状态（5GS 也拿不到时的兜底）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reg_eps: Option<RegState>,
    /// 服务小区（取自基站扫描缓存；没扫过就没有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serving: Option<super::cells::CellInfo>,
    /// 服务小区条数（取自扫描缓存）。没扫过、但已实时补到一条时为 1
    ///（与 Go `module/kernel/net.go:773` 同一口径，避免出现「有 serving 却显示 0」）。
    pub serving_count: usize,
    pub neighbour_count: usize,
    /// 上面这些缓存数据的扫描时间（没扫过就没有）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scanned_at: String,
    /// 载波聚合的分量载波（PCC + SCC），**实时查询**（指令按厂商挑，解析见 `model/ca.rs`）。
    ///
    /// 不能复用扫描缓存：否则会出现「频段是实时的 n78、CA 却是几分钟前扫到的 B3」
    /// 这种自相矛盾的展示（依据 Go `module/kernel/net.go:679-687`）。
    /// 模组不支持时 `ca` 为空、`ca_active` 为 false，前端按空态渲染。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ca: Vec<CcInfo>,
    pub ca_active: bool,
    /// 当前**在用**的 APN（`AT+CGCONTRDP`，取第一个有数据的上下文）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apn: Option<String>,
    /// PDP 分配的 IPv4 / IPv6（双栈时两个都有）
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
        let raw = self.with_at_prio(Prio::Data, |p| p.command("AT+COPS?", self.cmd_timeout))?;
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
        let resp = self.with_at_prio(Prio::Data, |p| {
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
            .with_at_prio(Prio::Data, |p| p.command("AT+QNWINFO", self.cmd_timeout))
            .unwrap_or_default();
        let info = parse_qnwinfo(&qnw);

        // `AT+QNWINFO` 是厂商私有指令，广和通 MTK 机型（FM350/RW350）直接回
        // ERROR，制式 / PLMN / 频段全空。此时用 3GPP 通用的 `AT+COPS?` 兜底：
        // format=2 只给 PLMN，但足够推出 mcc/mnc 与注册制式（`<AcT>`），
        // `operator` 也能拿到中文全名。
        //
        // 只在 QNWINFO 没给出 PLMN 时才多发这一条 —— 指令可用的厂商不增加
        // 往返。`AT+CSPPLMN?` 在该机型同样回 ERROR，没有第二条兜底路。
        //
        // ⚠️ 与 Go 的**有意差异**：Go 的兜底触发条件是 `net_tech` 为空、且走的是
        // **服务小区指令**（`module/kernel/net.go:651` `applyServingNetInfo`）；
        // 本仓库按「QNWINFO 没给 PLMN」触发、走 `AT+COPS?`（FM350 实机上
        // QNWINFO 回 ERROR 而 COPS 有值），服务小区兜底另有一路（见下面的 `serving`）。
        let cops = if info.plmn.is_none() {
            self.with_at_prio(Prio::Data, |p| p.command("AT+COPS?", self.cmd_timeout))
                .ok()
                .map(|raw| netselect::parse(&raw))
                .filter(|ns| ns.plmn.is_some() || ns.operator.is_some() || ns.act.is_some())
        } else {
            None
        };
        let f = merge_network(&info, cops.as_ref());

        let cs_reg = self
            .with_at_prio(Prio::Data, |p| p.command("AT+CREG?", self.cmd_timeout))
            .map(|raw| regstate::parse(&raw, "+CREG"))
            .unwrap_or_default();
        let ps_reg = self
            .with_at_prio(Prio::Data, |p| p.command("AT+CGREG?", self.cmd_timeout))
            .map(|raw| regstate::parse(&raw, "+CGREG"))
            .unwrap_or_default();

        let (apn, ipv4, ipv6) = self.query_active_context();
        let roaming = ps_reg.roaming;

        // 5GS / EPS 兜底：只在 CS/PS 都说未注册时补查（平时正常的模组不多发指令）
        let (mut reg_5g, mut reg_eps) = (None, None);
        if !cs_reg.registered && !ps_reg.registered {
            let raw = self
                .with_at_prio(Prio::Data, |p| p.command("AT+C5GREG?", self.cmd_timeout))
                .unwrap_or_default();
            if raw.contains("+C5GREG") {
                let st = regstate::parse(&raw, "+C5GREG");
                if st.stat > 0 {
                    reg_5g = Some(st);
                }
            }
            if reg_5g.is_none() {
                let raw = self
                    .with_at_prio(Prio::Data, |p| p.command("AT+CEREG?", self.cmd_timeout))
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
        let mut serving = cells.serving.first().cloned();
        if serving.is_none() {
            // 没扫过小区：概览页的「工作频段 / 物理频点」会是空的。
            // 单独问一次服务小区（1 条命令、优先级 Data），比让用户先点
            // 「扫描基站」才看得到频段合理得多（依据 Go `module/kernel/net.go:157`
            // 的 `ensureServingCell`；那边同样只补小区详情，不覆盖 QNWINFO 的制式/频段）。
            serving = self.serving_cell_live();
        }

        // 载波聚合：实时查一次（指令按厂商挑，解析见 model/ca.rs）。手册标的耗时 < 1s，
        // 相对本函数已有的 6~8 条 AT 是可接受的增量；查询失败只是这两项为空，
        // 不会连累其余字段。
        let ca = self.ca_status();

        // 漫游范围：不漫游时不用多读 IMSI
        let roam_scope = if roaming {
            let imsi = self
                .with_at_prio(Prio::Data, |p| p.command("AT+CIMI", self.cmd_timeout))
                .map(|raw| super::siminfo::parse_imsi(&raw))
                .unwrap_or(None);
            super::siminfo::roam_state(true, imsi.as_deref(), f.plmn.as_deref()).scope
        } else {
            "none".to_string()
        };

        // 扫描缓存没数、但上面实时补到了一条 ⇒ 记 1（口径见字段注释）
        let serving_count = if cells.serving_count == 0 && serving.is_some() {
            1
        } else {
            cells.serving_count
        };

        // 飞行模式：CFUN 原值同时给页面（4 = 飞行模式）
        let air = self.airplane();
        // 认不出双工时**不给这个字段**（空串也算一种值，前端会渲染成空白 pill）。
        // 依据 Go `module/kernel/net.go:641`：那边只在非空时才写 `Duplex`。
        let duplex = f
            .net_tech
            .as_deref()
            .map(duplex_of)
            .filter(|d| !d.is_empty());
        let cfun = if air.supported {
            if air.enabled {
                4
            } else {
                1
            }
        } else {
            0
        };

        // ---- 顶层汇总字段的兜底 ----
        //
        // 概览页读的是**顶层** `band` / `band_label` / `earfcn`（与 `/api/network`
        // 同名字段），而它们的原始来源是 QNWINFO —— 广和通 MTK 平台（FM350/RW350）
        // 不支持该指令，于是恒为 0/None，页面上「工作频段」「物理频点」永远是 `-`，
        // 尽管同一份数据在服务小区里明明有（实机 `serving.band=1`、`earfcn=428910`）。
        //
        // 这里用服务小区兜底：有 QNWINFO 就用它（信息更全），没有就用小区值。
        let serving_band = serving.as_ref().map(|c| c.band).unwrap_or(0);
        let serving_tech = serving.as_ref().map(|c| c.tech.as_str()).unwrap_or("");
        let band = f.band.filter(|b| *b > 0).unwrap_or(serving_band);
        let band_label = f.band_label.or_else(|| {
            let l = super::band::band_label(serving_tech, serving_band);
            if l.is_empty() {
                None
            } else {
                Some(l)
            }
        });
        // ⚠️ 与 Go 的**有意差异**：Go 在这里填 `BandTextFor(tech, band)`
        // （`module/kernel/net.go:186`，形如 `NR5G BAND 78`），本仓库填一句中文说明 ——
        // 这一栏在页面上直接显示，写清「值来自服务小区、本机不支持 QNWINFO」
        // 比再显示一个和 `band_label` 重复的串更有排查价值。
        let band_text = f.band_text.or_else(|| {
            band_label
                .as_ref()
                .map(|_| "取自服务小区（本机不支持 QNWINFO）".to_string())
        });
        let earfcn = if f.earfcn > 0 {
            f.earfcn
        } else {
            serving.as_ref().map(|c| c.earfcn).unwrap_or(0)
        };
        let tech = f.tech.or_else(|| {
            let t = serving_tech.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        });

        NetworkStatus {
            modem_id: self.id,
            queried_at: crate::logging::timestamp(),
            net_tech: f.net_tech,
            tech_label: f.tech_label,
            tech,
            mcc: f.mcc,
            mnc: f.mnc,
            plmn: f.plmn,
            band_text,
            duplex,
            band,
            band_label,
            earfcn,
            operator: f.operator,
            cs_reg,
            ps_reg,
            reg_5g,
            reg_eps,
            serving,
            serving_count,
            neighbour_count: cells.neighbour_count,
            // 没扫过就是空串（cells_cached 的空结果不带时间戳）
            scanned_at: self.cell_scan_at().unwrap_or_default(),
            // 实时 CA 查询，解析见 model/ca.rs（广和通那段照官方手册 §11.1.16）
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

    /// 当前激活上下文的 APN / 地址（`AT+CGCONTRDP`，依据 Go `module/kernel/net.go:689`）。
    ///
    /// 与 Go 的差异：Go 的 APN 走 `QueryAPN`（`AT+CGDCONT?` + `AT+CGCONTRDP`）、
    /// 地址走 `AT+CGPADDR` 两条指令；本仓库只用 `AT+CGCONTRDP` 一条 ——
    /// 它同时带 APN 与地址，且实测展锐之外的大多数固件都支持。
    fn query_active_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        let resp = self
            .with_at_prio(Prio::Data, |p| p.command("AT+CGCONTRDP", self.cmd_timeout))
            .unwrap_or_default();
        let (apn, mut v4, mut v6) = parse_active_context(&resp);
        // 350 系列（FM350 / RW350，广和通 MTK）的地址**只认 CGPADDR**：
        // 对齐参考实现 luci-app-fm350 的 `modem/pdp.rs` ——
        //   * 该固件的 `<local_addr>` / `<gw_addr>` 两列常年是空串，地址只在
        //     `AT+CGPADDR` 里，所以 CGCONTRDP 的地址在这里直接丢弃（只剩 APN 可用）；
        //   * `AT+CGPADDR` 的 IPv6 位出现 `::1` 占位即判「无 IPv6」，不回落到
        //     CGCONTRDP 的残留值（否则会配出一条永远不通的 v6 路由）；
        //   * `dial_state_*` 就是 CGPADDR 读到的值（拨号器与 `query_own_ipv4/6`
        //     都写它），所以这里不必每轮再发一条 AT；冷启动 / PDP 由外部激活时
        //     缓存是空的，才去模组现查一次。
        if crate::modem::model::is_fibocom_350(&self.model().unwrap_or_default()) {
            let (c4, c6) = (self.dial_state_ipv4(), self.dial_state_ipv6());
            let (a4, a6) = if c4.trim().is_empty() && c6.trim().is_empty() {
                let cid = self.dial_state.lock().unwrap().cid;
                let cid = if cid > 0 { cid } else { DEFAULT_DATA_CID };
                self.query_pdp_addrs(cid).unwrap_or_default()
            } else {
                (c4, c6)
            };
            v4 = (!a4.trim().is_empty()).then(|| a4.trim().to_string());
            // 占位残值（`::1` / 只有接口标识）不许显示，否则页面又会冒出个假 v6
            v6 = crate::netprov::v6::usable_static_v6(&a6);
        } else if v4.is_none() {
            // 其它厂商：CGCONTRDP 的地址列是可信的（上面已按 3GPP 字段位取过）。
            // `<local_addr>` 为空时退回拨号状态里那份，避免卡片整片空白。
            let cached = self.dial_state_ipv4();
            if !cached.trim().is_empty() {
                v4 = Some(cached);
            }
        }
        (apn, v4, v6)
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
        // 字段下标按 3GPP TS 27.007 §10.1.24（= 广和通手册 §12.2.10）：
        //   f[0]=cid  f[1]=bearer_id  f[2]=apn
        //   f[3]=<local_addr and subnet_mask>   f[4]=<gw_addr>
        //   f[5]=<DNS_prim>  f[6]=<DNS_sec>  f[7]/f[8]=<P-CSCF> …
        //
        // **只读 f[3]（外加 f[4] 里另一个家族那一列），绝不往后扫**。
        // 早先是「从第 3 位起找第一个像地址的字段」，遇到 f[3]/f[4] 为空串的
        // 固件就会一路扫进 DNS 列：实测 FM350-GL（81600.0000.00.29.24.02）
        // f[3]/f[4] 空、f[5]/f[6] 正是 `AT+GTDNS=1` 报的那两台 DNS ——
        // 于是页面把运营商下发的那台 IPv6 DNS 当成「模组本机 IPv6」显示出来，
        // 而 `AT+CGPADDR` 那边只有 `::1` 占位、链路上也没有 RA/DHCPv6，
        // 表现就是「模组有 V6、接口没 V6」。DNS 列绝对不能当本机地址。
        //
        // f[4] 只在「f[3] 已经给出另一个地址家族」时才收：那是同一上下文
        // v4/v6 拆成相邻两列时的写法；f[3] 为空时 f[4] 就是 <gw_addr>，
        // 把它当本机地址同样会配出一条不通的路由。
        let (mut v4, mut v6) = split_local_addr(f.get(3).map(String::as_str).unwrap_or(""));
        if let Some(gw) = f.get(4) {
            let (g4, g6) = split_local_addr(gw);
            if v4.is_some() && v6.is_none() {
                v6 = g6;
            } else if v6.is_some() && v4.is_none() {
                v4 = g4;
            }
        }
        if ctx.v4.is_none() {
            ctx.v4 = v4;
        }
        if ctx.v6.is_none() {
            ctx.v6 = v6;
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
        out.mcc = Some(plmn_mcc(plmn));
        out.mnc = Some(plmn_mnc(plmn));
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
/// 它把 `11/12` 写成 eMTC/NB-IoT。实机（FM350-GL）注册在 NR n78 时
/// `AT+COPS?` 回 `+COPS:0,2,"46011",11` —— 此固件 `11` 就是 NG-RAN。
///
/// 已知限制：NR NSA 下模组常以 LTE 注册，`<AcT>` 报 `7`，这里只能照报，
/// 不猜「其实是 5G」。（3GPP 的 `13` = EN-DC 双连接目前也走这条「不猜」的规矩，
/// 与 Go `module/kernel/net.go:307` 的 `ratFromACT` 不同 —— 那边把 13 显示成
/// 「5G NR (NSA)」。要不要跟，见 A05 报告的「风险与未决」。）
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

/// 拆 `AT+CGCONTRDP` 的 `<local_addr and subnet_mask>`（手册 §12.2.10）。
///
/// 与 [`split_addr`] 的差别只有一个：「地址 + 掩码」连写成一列时要先切掉掩码。
/// 出厂 `+CGPIAF: 0,0,0,0` 下这个字段是点分十进制，三种形态：
///
///   * IPv4：`a.b.c.d`（4 段）或 `a.b.c.d.m.m.m.m`（8 段，后 4 段是掩码）
///   * IPv6：16 段（地址）或 32 段（前 16 段地址 + 后 16 段掩码）
///   * 冒号写法（`+CGPIAF` 打开 IPv6 格式时）：原样交给 [`addr::normalize_ipv6`]
///
/// ⚠️ 这个函数**只该喂本机地址那一列**：同一条 `+CGCONTRDP` 里后面的 DNS /
/// P-CSCF 列格式完全一样，也正是「把 DNS 当本机地址」那个 bug 的来源。
pub fn split_local_addr(field: &str) -> (Option<String>, Option<String>) {
    let mut v4 = None;
    let mut v6 = None;
    for part in field.split_whitespace() {
        // 先把掩码切掉，再交给通用的 [`split_addr`] 做家族判定与归一化 ——
        // 判据只有一份，两条通路（`AT+CGPADDR` / `AT+CGCONTRDP`）不会分叉。
        let segs: Vec<&str> = part.split('.').collect();
        let addr_only = match segs.len() {
            8 => segs[..4].join("."),
            32 => segs[..16].join("."),
            _ => part.to_string(),
        };
        let (a, b) = split_addr(&addr_only);
        if v4.is_none() {
            v4 = a;
        }
        if v6.is_none() {
            v6 = b;
        }
    }
    (v4, v6)
}

#[cfg(test)]
mod tests {
    use super::netselect;
    use super::{
        band_label, cops_act_tech, merge_network, operator_display, parse_active_context,
        parse_band, parse_qnwinfo, split_addr, split_local_addr, tech_label,
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

    // ---------- <local_addr and subnet_mask>（地址 + 掩码连写） ----------

    #[test]
    fn split_local_addr_strips_ipv4_mask() {
        // 手册 §12.2.10：IPv4 是 a1.a2.a3.a4.m1.m2.m3.m4
        assert_eq!(
            split_local_addr("10.42.4.240.255.255.255.0"),
            (Some("10.42.4.240".into()), None)
        );
        // 不带掩码的写法照旧
        assert_eq!(
            split_local_addr("10.42.4.240"),
            (Some("10.42.4.240".into()), None)
        );
    }

    #[test]
    fn split_local_addr_strips_ipv6_mask() {
        // IPv6 是 16 段地址 + 16 段掩码
        assert_eq!(
            split_local_addr(
                "32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8.\
                 255.255.255.255.255.255.255.255.0.0.0.0.0.0.0.0"
            ),
            (None, Some("2001:db8:1:2::8".into()))
        );
        // 冒号写法（+CGPIAF 打开 IPv6 格式时）同样认
        assert_eq!(
            split_local_addr("2001:db8:1:2::8"),
            (None, Some("2001:db8:1:2::8".into()))
        );
    }

    // ---------- 激活上下文：APN / 地址 ----------

    /// 实机（FM350-GL，固件 81600.0000.00.29.24.02）原始应答形态：
    /// 同一个 cid 报两行（v4 一行、v6 一行），但 local_addr / gw_addr 两列
    /// 都是空串，f[5] / f[6] 才是 DNS —— 与 AT+GTDNS=1 报的完全一致。
    ///
    /// 这是「模组有 V6、接口没 V6」的根因夹具：早先的实现从 f[3] 往后扫第一个
    /// 「像地址」的字段，于是把 DNS 当成模组本机地址，页面显示出一个模组根本
    /// 没有的 IPv6（AT+CGPADDR 那边只有 ::1 占位），接口自然永远配不上。
    const FIBOCOM_MTK_CGCONTRDP_DNS_ONLY: &str = concat!(
        "AT+CGCONTRDP\r\n",
        "+CGCONTRDP: 1,,\"cmiot5g\",\"\",\"\",\"203.0.113.53\",",
        "\"203.0.113.54\",\"\",\"\",0,,0\r\n",
        "+CGCONTRDP: 1,,\"cmiot5g\",\"\",\"\",",
        "\"32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.8\",",
        "\"32.1.13.184.0.0.0.0.0.0.0.0.0.0.4.8\",\"\",\"\",0,,0\r\n",
        "OK\r\n"
    );

    #[test]
    fn active_context_never_reads_dns_columns_as_local_address() {
        // 只有 APN 是可信的；地址列（f[3] / f[4]）是空串 ⇒ 两个家族都没地址。
        // 尤其不能把点分 16 段的 DNS（= 2001:db8::8）当成模组本机 IPv6。
        let (apn, v4, v6) = parse_active_context(FIBOCOM_MTK_CGCONTRDP_DNS_ONLY);
        assert_eq!(apn.as_deref(), Some("cmiot5g"));
        assert_eq!(v4, None);
        assert_eq!(v6, None);
    }

    #[test]
    fn active_context_merges_v4_and_v6_rows() {
        // 同一个 cid 拆两行（一行 v4、一行把 IPv6 展开成 16 段点分十进制）：
        // 地址都在 f[3]，归并后两个家族都要取到。
        let resp = concat!(
            "+CGCONTRDP: 1,,\"ctnet\",\"10.42.4.240\",\"10.42.4.1\",",
            "\"203.0.113.53\",\"203.0.113.54\",\"\",\"\"\r\n",
            "+CGCONTRDP: 1,,\"ctnet\",\"32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8\",",
            "\"32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.1\",\"\",\"\",\"\",\"\"\r\n",
            "OK\r\n"
        );
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn.as_deref(), Some("ctnet"));
        assert_eq!(v4.as_deref(), Some("10.42.4.240"));
        assert_eq!(v6.as_deref(), Some("2001:db8:1:2::8"));
    }

    #[test]
    fn active_context_reads_3gpp_style_address_field() {
        // 3GPP 规定地址在 f[3]（移远/广和通其它固件常见写法）；
        // f[4] 是 gw_addr，但这里给的是**另一个家族**，按「拆成相邻两列」收下
        let resp = "+CGCONTRDP: 1,,\"cmiot5g\",\"10.42.4.240\",\"2001:db8:1:2::8\",\"\",\"\"\r\nOK";
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn.as_deref(), Some("cmiot5g"));
        assert_eq!(v4.as_deref(), Some("10.42.4.240"));
        assert_eq!(v6.as_deref(), Some("2001:db8:1:2::8"));
    }

    #[test]
    fn active_context_ignores_gateway_as_address() {
        // f[3] 为空时 f[4] 就是网关，不能当成本机地址（否则会配出一条不通的路由）
        let resp = "+CGCONTRDP: 1,,\"cmiot5g\",\"\",\"10.42.4.1\",\"203.0.113.53\",\"\"\r\nOK";
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn.as_deref(), Some("cmiot5g"));
        assert_eq!(v4, None);
        assert_eq!(v6, None);
    }

    #[test]
    fn active_context_v6_only_context_is_not_skipped() {
        // 纯 IPv6 上下文：无 APN、无 v4。原先的选中判据含 v4，会整条跳过
        let resp = "+CGCONTRDP: 1,,\"\",\"2001:db8:1:2::8\"\r\nOK";
        let (apn, v4, v6) = parse_active_context(resp);
        assert_eq!(apn, None);
        assert_eq!(v4, None);
        assert_eq!(v6.as_deref(), Some("2001:db8:1:2::8"));
    }

    #[test]
    fn active_context_drops_untrustworthy_addresses() {
        // 点分的 ::1 占位：既不是合法 IPv4，也不该当成 IPv6 显示
        let resp = "+CGCONTRDP: 1,,\"ctnet\",\"0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1\"\r\nOK";
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
        let resp = "+CGCONTRDP: 1,,\"\",\"\",\"\"\r\nOK";
        assert_eq!(parse_active_context(resp), (None, None, None));
    }

    #[test]
    fn active_context_keeps_distinct_cids_separate() {
        // 两个不同 cid 各自一行：只取第一个有数据的，不会把 cid 2 的地址
        // 混进 cid 1 的结果里
        let resp = concat!(
            "+CGCONTRDP: 1,,\"apn-a\",\"10.0.0.2\"\r\n",
            "+CGCONTRDP: 2,,\"apn-b\",\"10.0.0.9\"\r\n",
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
