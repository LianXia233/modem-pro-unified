//! 基站/小区扫描（`GET /api/cells`、`/api/cells/all`）。
//!
//! 各平台的服务小区来源不同：移远/广和通高通走 `+QENG` / `+FENG`（带标签形式），
//! 芯讯通走 `+CPSI`，广和通 MTK 平台（FM350 / RW350）走 `AT+GTCCINFO?` 的
//! **无标签行表**（见 [`parse_gtccinfo_rows`]）。
//! 带标签的那几种字段顺序各固件不一致，因此信号值**按量纲定位**而不是硬编码下标；
//! GTCCINFO 行表的列序则由手册按 `<rat>` + 角色钉死。

use serde::Serialize;

use super::super::at::{csv, to_i32, Prio};
use super::super::vendor::Vendor;
use super::band;
use super::modem::Modem;

/// 一个小区（服务小区或邻小区）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CellInfo {
    pub tech: String,
    pub state: String,
    pub mcc: String,
    pub mnc: String,
    pub cell_id: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pcid: i32,
    pub tac: i32,
    pub earfcn: i32,
    pub band: i32,
    /// 频段可读标签（`n78` / `B3`），由 `band` + `tech` 现算；未知时缺省。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_label: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub bandwidth: i32,
    pub rsrp: i32,
    pub rsrq: i32,
    #[serde(skip_serializing_if = "is_zero")]
    pub rssi: i32,
    pub sinr: i32,
    pub serving: bool,
    pub intra_cell: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// 一次基站扫描的完整结果。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CellScanResult {
    pub modem_id: u32,
    pub scanned_at: String,
    pub serving: Vec<CellInfo>,
    pub neighbours: Vec<CellInfo>,
    pub cells: Vec<CellInfo>,
    pub count: usize,
    pub serving_count: usize,
    pub neighbour_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_info: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_reg: Option<String>,
    pub ca_active: bool,
    /// 本机实际可用的载波聚合查询指令（页面「没有 CA」的空态里要显示它，
    /// 不能把移远的 `AT+QCAINFO` 写死 —— 换机型就是错的）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ca_cmds: Vec<String>,
    /// 空态说明（为什么没有 CA / 什么时候才会有）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ca_note: String,
}

/// 没有载波聚合时的说明（与厂商无关的事实，别写成"不支持"）。
const CA_EMPTY_NOTE: &str = "空闲态下模组通常只上报主载波；进入连接态并由网络配置辅载波后，\n才会出现 SCC。本页已按厂商指令实时查询（见 model/ca.rs），仍为空说明模组本轮未上报";

fn is_zero(v: &i32) -> bool {
    *v == 0
}

impl CellScanResult {
    /// 由小区列表构造结果（按服务/邻区分类并统计）。
    /// `ca` 是本次扫描顺带查到的载波聚合状态（见 [`super::ca`]）。
    pub fn from_cells(modem_id: u32, cells: Vec<CellInfo>, ca: Option<super::ca::CaInfo>) -> Self {
        let mut serving = Vec::new();
        let mut neighbours = Vec::new();
        for c in &cells {
            if c.serving {
                serving.push(c.clone());
            } else {
                neighbours.push(c.clone());
            }
        }
        fill_neighbour_plmn(&serving, &mut neighbours);
        mark_intra_frequency(&serving, &mut neighbours);
        let operator = serving.first().and_then(|c| c.operator.clone());
        // 频段可读标签统一在此现算：各厂商的拨号器只填 band 号，不各自拼标签
        let mut cells = cells;
        for c in cells.iter_mut() {
            if c.band > 0 {
                c.band_label = Some(band::band_label(&c.tech, c.band));
            }
        }
        for c in serving.iter_mut().chain(neighbours.iter_mut()) {
            if c.band > 0 {
                c.band_label = Some(band::band_label(&c.tech, c.band));
            }
        }
        Self {
            modem_id,
            scanned_at: crate::logging::timestamp(),
            serving_count: serving.len(),
            neighbour_count: neighbours.len(),
            count: cells.len(),
            serving,
            neighbours,
            cells,
            operator,
            net_info: None,
            network_reg: None,
            // 扫描完顺手查一次 CA：扫描本身就说明「用户在看小区」，
            // 且此时刚进过连接态，SCC 最容易拿到（见 model/ca.rs）
            ca_active: ca.map(|c| c.ca_active).unwrap_or(false),
            ca_cmds: Vec::new(),
            ca_note: String::new(),
        }
    }

    /// 空结果（缓存尚未填充时）。
    ///
    /// `scanned_at` 必须留空：填成"现在"会让页面把「从没扫过」显示成「刚刚扫过」。
    pub fn empty(modem_id: u32) -> Self {
        Self {
            modem_id,
            ..Default::default()
        }
    }
}

impl Modem {
    /// 只查**服务小区**（不扫邻区）。
    ///
    /// 与 [`Self::scan_cells`] 的分工：那个是「用户点了扫描基站」，会连邻区一起抓、
    /// 顺带查 CA、写扫描缓存，耗时几秒；本函数只发**一条**服务小区指令。
    ///
    /// 存在的理由：概览页要显示「工作频段 / 物理频点(EARFCN)」，而这两项来自
    /// 服务小区。早先只读 `cells_cached()`（扫描缓存），没扫过就恒为 `-` ——
    /// 让用户为了看频段先去点一次「扫描基站」是不合理的。
    ///
    /// 优先级用 `Data`：它是页面数据完整性的一部分，不属于「冷门」。
    pub fn serving_cell_live(&self) -> Option<CellInfo> {
        if self.vendor() == Vendor::TDTech {
            // 鼎桥 MT5700：QENG 全回 ERROR，服务小区在 ^MONSC（手册 §13.9，
            // 解析按 luci-app-mt5700 实机格式，LTE/NR 布局不同）
            return parse_monsc(&self.at_cmd_prio(Prio::Data, "AT^MONSC"));
        }
        if matches!(self.vendor(), Vendor::Simcom) {
            let resp = self.at_cmd_prio(Prio::Data, "AT+CPSI?");
            return resp.lines().find_map(parse_cpsi_cell);
        }
        if self.is_fibocom_mtk() {
            // FM350 / RW350（MTK 平台）：没有 QENG，服务小区在 GTCCINFO 行表里
            return parse_gtccinfo_rows(&self.at_cmd_prio(Prio::Data, "AT+GTCCINFO?"))
                .into_iter()
                .next();
        }
        for cmd in ["AT+QENG=\"servingcell\"", "AT+FENG=\"servingcell\""] {
            let parsed = parse_cell_lines(&self.at_cmd_prio(Prio::Data, cmd));
            if !parsed.is_empty() {
                return parsed.into_iter().next();
            }
        }
        None
    }

    /// 实时扫描服务小区 + 邻小区。
    pub fn scan_cells(&self) -> CellScanResult {
        let mut cells = Vec::new();
        if self.vendor() == Vendor::TDTech {
            // 鼎桥 MT5700：QENG 全回 ERROR。服务小区 ^MONSC、邻区 ^MONNC
            // （手册 §13.9/13.10，解析按 luci-app-mt5700 实机格式）。
            cells.extend(parse_monsc(&self.at_cmd_prio(Prio::Bulk, "AT^MONSC")));
            cells.extend(parse_monnc_rows(&self.at_cmd_prio(Prio::Bulk, "AT^MONNC")));
        } else if matches!(self.vendor(), Vendor::Simcom) {
            let resp = self.at_cmd_prio(Prio::Bulk, "AT+CPSI?");
            for line in resp.lines() {
                if let Some(cell) = parse_cpsi_cell(line) {
                    cells.push(cell);
                }
            }
        } else if self.is_fibocom_mtk() {
            // FM350 / RW350 系列（MTK 平台）没有 QENG / FENG：服务小区与邻小区
            // 都在 AT+GTCCINFO? 的行表里（手册 §11.1.15），发 QENG 只会白等超时
            cells.extend(parse_gtccinfo_rows(
                &self.at_cmd_prio(Prio::Bulk, "AT+GTCCINFO?"),
            ));
        } else {
            for cmd in ["AT+QENG=\"servingcell\"", "AT+FENG=\"servingcell\""] {
                let parsed = parse_cell_lines(&self.at_cmd_prio(Prio::Bulk, cmd));
                if !parsed.is_empty() {
                    cells.extend(parsed);
                    break;
                }
            }
            for cmd in ["AT+QENG=\"neighbourcell\"", "AT+FENG=\"neighbourcell\""] {
                let parsed = parse_cell_lines(&self.at_cmd_prio(Prio::Bulk, cmd));
                if !parsed.is_empty() {
                    cells.extend(parsed);
                    break;
                }
            }
        }
        // 扫描完顺手查一次 CA：扫描本身就说明「用户在看小区」，此时刚过连接态，
        // 是最容易拿到 SCC 的时机（解析见 model/ca.rs）
        let ca = self.ca_status();
        let mut result = CellScanResult::from_cells(self.id, cells, ca);
        result.ca_cmds = self.ca_cmds();
        result.ca_note = CA_EMPTY_NOTE.to_string();
        // 扫描页底部那三行（运营商 / 网络接入信息 / 注册状态）：扫描本来就慢，
        // 顺手补上比让页面显示三个 `-` 有用。运营商在小区行里常常没有，
        // 用 `AT+COPS?` 的全名兜底。
        if result.operator.as_deref().unwrap_or("").is_empty() {
            result.operator = self.network().ok().and_then(|n| n.operator);
        }
        if let Ok(info) = self.net_info() {
            if !info.trim().is_empty() {
                result.net_info = Some(info);
            }
        }
        let reg = self
            .with_at_prio(Prio::Bulk, |p| p.command("AT+CGREG?", self.cmd_timeout))
            .map(|raw| super::regstate::parse(&raw, "+CGREG"))
            .unwrap_or_default();
        if reg.raw.is_some() {
            result.network_reg = Some(reg.text);
        }
        self.set_cells_cache(result.clone());
        result
    }

    /// 最近一次扫描缓存（不触发扫描）；尚未扫描时返回空结果。
    pub fn cells_cached(&self) -> CellScanResult {
        let mut r = self
            .cells_cache()
            .unwrap_or_else(|| CellScanResult::empty(self.id));
        // 缓存里不带「这台机器有哪些 CA 指令」（那是厂商属性，不是扫描结果），
        // 每次取出时补上，界面才能把空态说明写对。
        if r.ca_cmds.is_empty() {
            r.ca_cmds = self.ca_cmds();
        }
        if r.ca_note.is_empty() {
            r.ca_note = CA_EMPTY_NOTE.to_string();
        }
        r
    }

    /// 本机厂商声明过的 CA 查询指令（`AT+QCAINFO` / `AT+GTCAINFO?` / `AT#CAINFOEXT?`…）。
    pub fn ca_cmds(&self) -> Vec<String> {
        self.vendor()
            .spec()
            .vendor_cmds()
            .iter()
            .filter(|c| c.to_ascii_uppercase().contains("CAINFO"))
            .map(|c| (*c).to_string())
            .collect()
    }

    /// 最近一次扫描时间（不触发扫描；从没扫过就没有）。
    ///
    /// 不能直接用 [`Self::cells_cached`]：它的空结果会把 `scanned_at` 填成"现在"，
    /// 页面会把「没扫过」显示成「刚刚扫过」。
    pub fn cell_scan_at(&self) -> Option<String> {
        self.cells_cache()
            .map(|c| c.scanned_at)
            .filter(|s| !s.is_empty())
    }
}

/// 归一化接入技术：`NR5G-SA` / `NR5G-NSA` / `NR5G` → `NR`。
pub fn normalize_tech(tok: &str) -> String {
    let s = tok.trim().to_ascii_uppercase().replace(['_', ' '], "-");
    match s.as_str() {
        "LTE" | "LTE-A" | "LTE-A-PRO" => "LTE",
        "NR" | "NR5G" | "NR5G-SA" | "NR5G-NSA" => "NR",
        "WCDMA" | "UMTS" | "HSPA" => "WCDMA",
        "GSM" | "GPRS" | "EDGE" => "GSM",
        _ => return tok.trim().to_ascii_uppercase(),
    }
    .to_string()
}

/// 是否为已知接入技术 token。
pub fn is_tech_token(tok: &str) -> bool {
    matches!(normalize_tech(tok).as_str(), "LTE" | "NR" | "WCDMA" | "GSM")
}

/// 是否为显式 NR SA/NSA token（此类报文在 cellID 之后多一格 PCI）。
pub fn is_nrsa_token(tok: &str) -> bool {
    let s = tok.trim().to_ascii_uppercase().replace(['_', ' '], "-");
    s == "NR5G-SA" || s == "NR5G-NSA"
}

fn is_duplex_token(tok: &str) -> bool {
    matches!(
        tok.trim().to_ascii_uppercase().as_str(),
        "FDD" | "TDD" | "HD-FDD"
    )
}

/// MCC+MNC → 中国运营商名（认不出返回空串）。
///
/// 只是 [`super::siminfo::operator_from_plmn`] 的一个转发壳：运营商表**只有一份**，
/// 免得两处各维护一张、口径打架（历史上这里把 46004/46008 判成广电，
/// 而 SIM 侧判成移动，同一张卡两个页面显示两家运营商）。
pub fn operator_name(mcc: &str, mnc: &str) -> String {
    super::siminfo::operator_from_plmn(mcc, mnc).unwrap_or_default()
}

/// 由频点号推算频段号（3GPP TS 36.101 / TS 38.104 常用频段）。
pub fn band_from_earfcn(tech: &str, earfcn: i32) -> i32 {
    if earfcn <= 0 {
        return 0;
    }
    if tech == "NR" {
        return match earfcn {
            422000..=431999 => 1,
            361000..=376000 => 3,
            173800..=178800 => 5,
            185000..=192000 => 8,
            151600..=160600 => 28,
            499200..=537999 => 41,
            620000..=653333 => 78,
            693334..=733333 => 79,
            _ => 0,
        };
    }
    match earfcn {
        0..=599 => 1,
        600..=1199 => 2,
        1200..=1949 => 3,
        1950..=2399 => 4,
        2400..=2649 => 5,
        2750..=3449 => 7,
        3450..=3799 => 8,
        6150..=6449 => 20,
        9210..=9659 => 28,
        36200..=36349 => 34,
        37750..=38249 => 38,
        38250..=38649 => 39,
        38650..=39649 => 40,
        39650..=41589 => 41,
        41590..=43589 => 42,
        43590..=45589 => 43,
        _ => 0,
    }
}

fn valid_plmn(mcc: &str, mnc: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let all_zero = |s: &str| s.chars().all(|c| c == '0');
    mcc.len() == 3 && digits(mcc) && !all_zero(mcc) && (1..=3).contains(&mnc.len()) && digits(mnc)
}

/// 解析整数（先十进制、失败再按十六进制）。
pub fn parse_at_int(value: &str) -> Option<i32> {
    let s = value.trim().trim_matches('"');
    if s.is_empty() {
        return None;
    }
    if let Ok(v) = s.parse::<i32>() {
        return Some(v);
    }
    i64::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
        .ok()
        .map(|v| v as i32)
}

/// 从 `start` 起找第一个落在 RSRP 量纲区间（-156 ~ -44 dBm）的字段下标。
pub fn find_rsrp_token(f: &[String], start: usize) -> Option<usize> {
    (start..f.len()).find(|&i| matches!(to_i32(&f[i]), Some(v) if (-156..=-44).contains(&v)))
}

/// 从 RSRP 所在下标起按量纲解析 RSRP/RSRQ/RSSI/SINR。
pub fn apply_signal_fields(cell: &mut CellInfo, f: &[String], idx: usize) {
    cell.rsrp = to_i32(&f[idx]).unwrap_or(0);
    if let Some(v) = f.get(idx + 1).and_then(|s| to_i32(s)) {
        if (-43..=20).contains(&v) {
            cell.rsrq = v;
        }
    }
    if let Some(v) = f.get(idx + 2).and_then(|s| to_i32(s)) {
        if (-113..=-25).contains(&v) {
            cell.rssi = v;
            if let Some(s) = f.get(idx + 3).and_then(|s| to_i32(s)) {
                cell.sinr = s;
            }
        } else {
            cell.sinr = v;
        }
    }
}

/// 取 AT 行里 `+XXX: "标签", ...` 的标签与剩余字段串。
fn cell_line_parts(line: &str) -> Option<(String, String)> {
    let colon = line.find(':')?;
    let after = &line[colon + 1..];
    let start = after.find('"')?;
    let rest = &after[start + 1..];
    let end = rest.find('"')?;
    let tag = rest[..end].to_string();
    let tail = rest[end + 1..].trim_start_matches([',', ' ']).to_string();
    Some((tag, tail))
}

/// 解析一行 AT 返回的小区信息（服务小区或邻小区）。
pub fn parse_cell_line(line: &str) -> Option<CellInfo> {
    let (tag, rest) = cell_line_parts(line)?;
    let tag = tag.trim().to_ascii_lowercase();
    if tag == "servingcell" {
        return parse_serving_cell(&rest, line);
    }
    if tag.starts_with("neighbourcell") {
        return parse_neighbour_cell(&rest, line, tag.contains("intra"));
    }
    None
}

/// 解析服务小区。
pub fn parse_serving_cell(rest: &str, raw: &str) -> Option<CellInfo> {
    let f = csv(rest);
    let mut i = 0usize;
    let mut state = String::new();
    if f.first().map(|t| !is_tech_token(t)).unwrap_or(true) {
        state = f.first()?.trim().to_ascii_uppercase();
        i = 1;
    }
    let tech_tok = f.get(i)?.clone();
    if !is_tech_token(&tech_tok) {
        return None;
    }
    let mut cell = CellInfo {
        tech: normalize_tech(&tech_tok),
        state,
        serving: true,
        raw: Some(raw.trim().to_string()),
        ..Default::default()
    };
    i += 1;
    if f.get(i).map(|t| is_duplex_token(t)).unwrap_or(false) {
        i += 1;
    }
    cell.mcc = f.get(i)?.clone();
    i += 1;
    cell.mnc = f.get(i)?.clone();
    i += 1;
    cell.cell_id = f.get(i)?.trim().to_ascii_uppercase();
    i += 1;

    if cell.tech == "LTE" {
        cell.pcid = f.get(i).and_then(|s| parse_at_int(s)).unwrap_or(0);
        i += 1;
        if let Some(v) = f.get(i).and_then(|s| to_i32(s)) {
            cell.earfcn = v;
        }
        i += 1;
        if let Some(v) = f.get(i).and_then(|s| to_i32(s)) {
            if (1..=261).contains(&v) {
                cell.band = v;
            }
        }
        i += 1;
        if let Some(idx) = find_rsrp_token(&f, i) {
            if idx > i {
                cell.tac = f.get(idx - 1).and_then(|s| parse_at_int(s)).unwrap_or(0);
                i = idx;
            }
        }
    } else {
        if is_nrsa_token(&tech_tok) {
            cell.pcid = f.get(i).and_then(|s| parse_at_int(s)).unwrap_or(0);
            i += 1;
        }
        if let Some(v) = f.get(i).and_then(|s| parse_at_int(s)) {
            cell.tac = v;
        }
        i += 1;
        if let Some(v) = f.get(i).and_then(|s| to_i32(s)) {
            cell.earfcn = v;
        }
        i += 1;
    }

    if !valid_plmn(&cell.mcc, &cell.mnc) {
        return None;
    }
    let op = operator_name(&cell.mcc, &cell.mnc);
    cell.operator = (!op.is_empty()).then_some(op);

    if let Some(idx) = find_rsrp_token(&f, i) {
        if idx > i {
            if let Some(b) = f.get(i).and_then(|s| to_i32(s)) {
                if (1..=261).contains(&b) {
                    cell.band = b;
                }
            }
        }
        apply_signal_fields(&mut cell, &f, idx);
    }
    if cell.band == 0 {
        cell.band = band_from_earfcn(&cell.tech, cell.earfcn);
    }
    Some(cell)
}

/// 解析邻小区（`neighbourcell intra|inter`）。
pub fn parse_neighbour_cell(rest: &str, raw: &str, intra: bool) -> Option<CellInfo> {
    let f = csv(rest);
    if f.len() < 2 || !is_tech_token(&f[0]) {
        return None;
    }
    let mut cell = CellInfo {
        tech: normalize_tech(&f[0]),
        intra_cell: intra,
        raw: Some(raw.trim().to_string()),
        ..Default::default()
    };
    cell.earfcn = to_i32(&f[1]).unwrap_or(0);
    cell.pcid = f.get(2).and_then(|s| parse_at_int(s)).unwrap_or(0);
    cell.band = band_from_earfcn(&cell.tech, cell.earfcn);

    const SIG_START: usize = 3;
    let Some(idx) = find_rsrp_token(&f, SIG_START) else {
        for tok in &f[SIG_START..] {
            if let Some(v) = to_i32(tok) {
                if (-43..=20).contains(&v) {
                    cell.rsrq = v;
                    break;
                }
            }
        }
        return Some(cell);
    };
    cell.rsrp = to_i32(&f[idx]).unwrap_or(0);

    let mut rsrq_at = None;
    if idx > SIG_START {
        if let Some(v) = to_i32(&f[idx - 1]) {
            if (-43..=20).contains(&v) {
                cell.rsrq = v;
                rsrq_at = Some(idx - 1);
            }
        }
    }
    if rsrq_at.is_none() {
        if let Some(v) = f.get(idx + 1).and_then(|s| to_i32(s)) {
            if (-43..=20).contains(&v) {
                cell.rsrq = v;
                rsrq_at = Some(idx + 1);
            }
        }
    }

    let mut got_rssi = false;
    let mut got_sinr = false;
    for (i, tok) in f.iter().enumerate().skip(SIG_START) {
        if i == idx || Some(i) == rsrq_at {
            continue;
        }
        let Some(v) = to_i32(tok) else { continue };
        if !got_rssi && (-113..=-25).contains(&v) {
            cell.rssi = v;
            got_rssi = true;
            continue;
        }
        if !got_sinr && (-23..=40).contains(&v) {
            cell.sinr = v;
            got_sinr = true;
        }
    }
    Some(cell)
}

/// 判断是否为「只有标记、没有数据」的服务小区行，返回其中的状态字段。
fn serving_header_only(line: &str) -> Option<String> {
    let (tag, rest) = cell_line_parts(line)?;
    if !tag.trim().eq_ignore_ascii_case("servingcell") {
        return None;
    }
    let f = csv(&rest);
    if f.first().map(|t| is_tech_token(t)).unwrap_or(false) {
        return None;
    }
    Some(
        f.first()
            .map(|s| s.trim().to_ascii_uppercase())
            .unwrap_or_default(),
    )
}

/// 解析不带 `servingcell` 标记的服务小区数据行（形如 `+QENG: "LTE","FDD",460,11,…`）。
pub fn parse_serving_data_line(line: &str) -> Option<CellInfo> {
    let (tag, rest) = cell_line_parts(line)?;
    let tag_l = tag.trim().to_ascii_lowercase();
    if tag_l.starts_with("neighbourcell") {
        return None;
    }
    let tech = tag.trim();
    let f = csv(&rest);
    if !is_tech_token(tech) && f.first().map(|t| !is_tech_token(t)).unwrap_or(true) {
        return None;
    }
    parse_serving_cell(&format!("{tech},{rest}"), line)
}

/// 批量解析 AT 应答中的小区信息（兼容「标记行 + 数据行」分开的写法）。
pub fn parse_cell_lines(resp: &str) -> Vec<CellInfo> {
    let mut cells = Vec::new();
    let mut serving_state: Option<String> = None;
    let mut pending = false;
    for line in resp.lines() {
        if let Some(cell) = parse_cell_line(line) {
            cells.push(cell);
            pending = false;
            continue;
        }
        if let Some(state) = serving_header_only(line) {
            serving_state = Some(state);
            pending = true;
            continue;
        }
        if !pending {
            continue;
        }
        if let Some(mut cell) = parse_serving_data_line(line) {
            if cell.state.is_empty() {
                cell.state = serving_state.clone().unwrap_or_default();
            }
            cells.push(cell);
            pending = false;
        }
    }
    cells
}

/// 解析 `+CPSI` 服务小区上报（裸数字频段那套写法）。
pub fn parse_cpsi_cell(line: &str) -> Option<CellInfo> {
    let colon = line.find("CPSI:")?;
    let f = csv(&line[colon + 5..]);
    if f.len() < 8 {
        return None;
    }
    let tech = normalize_tech(&f[0]);
    if tech != "LTE" && tech != "NR" {
        return None;
    }
    let cell_id = f[3].trim().to_ascii_uppercase();
    let cell_id = cell_id
        .trim_start_matches("0X")
        .trim_start_matches("0x")
        .to_string();
    if cell_id.is_empty() || cell_id == "-" || !cell_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let earfcn = to_i32(&f[4])?;
    let pcid = parse_at_int(&f[5])?;
    let band = to_i32(&f[6])?;
    if !(0..=1007).contains(&pcid) || earfcn <= 0 || !(1..=256).contains(&band) {
        return None;
    }
    let plmn: String = f[2].trim().replace('-', "");
    if plmn.len() < 5 {
        return None;
    }
    let mut cell = CellInfo {
        tech,
        state: f[1].trim().to_string(),
        mcc: plmn[..3].to_string(),
        mnc: plmn[3..].to_string(),
        cell_id,
        earfcn,
        pcid,
        band,
        serving: true,
        raw: Some(line.trim().to_string()),
        ..Default::default()
    };
    let op = operator_name(&cell.mcc, &cell.mnc);
    cell.operator = (!op.is_empty()).then_some(op);
    let sig = csv(line);
    if let Some(idx) = find_rsrp_token(&sig, 0) {
        apply_signal_fields(&mut cell, &sig, idx);
    }
    (cell.rsrp != 0).then_some(cell)
}

/// `AT+GTCCINFO?` 行内各信号列的定位（《FM350 AT Commands》§11.1.15）。
///
/// 服务小区与邻小区的列序不同：邻区没有 `<band>` 列，NR 邻区连 `<bandwidth>`
/// 都没有，LTE 邻区还没有 `<rssnr_value>`。因此必须按 `<rat>` + `<IsServiceCell>`
/// 分别定位，`None` 表示这一行没有该列。
struct GtccCols {
    band: Option<usize>,
    bw: Option<usize>,
    sinr: Option<usize>,
    rsrp: Option<usize>,
    rsrq: Option<usize>,
}

fn gtccinfo_cols(rat: &str, serving: bool) -> Option<GtccCols> {
    Some(match (rat, serving) {
        // LTE/eMTC/NB-IoT 服务小区：…,<band>,<bandwidth>,<rssnr>,<rxlev>,<rsrp>,<rsrq>
        ("4", true) => GtccCols {
            band: Some(8),
            bw: Some(9),
            sinr: Some(10),
            rsrp: Some(12),
            rsrq: Some(13),
        },
        // LTE 邻区没有 <band> 与 <rssnr>，<bandwidth> 顶到第 9 列
        ("4", false) => GtccCols {
            band: None,
            bw: Some(8),
            sinr: None,
            rsrp: Some(10),
            rsrq: Some(11),
        },
        // NR 服务小区：…,<band>,<bandwidth>,<ss_sinr>,<rxlev>,<ss_rsrp>,<ss_rsrq>
        ("9", true) => GtccCols {
            band: Some(8),
            bw: Some(9),
            sinr: Some(10),
            rsrp: Some(12),
            rsrq: Some(13),
        },
        // NR 邻区既无 <band> 也无 <bandwidth>，<ss_sinr> 顶到第 9 列
        ("9", false) => GtccCols {
            band: None,
            bw: None,
            sinr: Some(8),
            rsrp: Some(10),
            rsrq: Some(11),
        },
        // WCDMA 只取身份与 <band>（服务小区第 9 列）；ecno / rscp / rxlev / Ec/Io
        // 与本结构的信号格口径不同，不硬套
        ("2", true) => GtccCols {
            band: Some(8),
            bw: None,
            sinr: None,
            rsrp: None,
            rsrq: None,
        },
        _ => return None,
    })
}

/// `n41` / `B41` → `41`（解码器的产出形如 `n`/`B` 前缀 + 频段号）。
fn band_no_of(label: &str) -> i32 {
    label.trim_start_matches(['n', 'B']).parse().unwrap_or(0)
}

/// 解析 `AT+GTCCINFO?` 的行表（广和通 MTK 平台：FM350 / RW350 系列）。
///
/// 该平台不像移远 / 广和通高通那样回带标签的 `"servingcell","CONNECT",…`，
/// 而是先回一行 `+GTCCINFO:` 头，随后**每行一个小区、首列区分角色**
/// （1 = 服务小区，2 = 邻区），`<rat>` 决定字段布局（手册 §11.1.15）：
///
/// ```text
/// <role>,<rat>,<mcc>,<mnc>,<tac|lac>,<cellid>,<arfcn>,<pci>,<band>,<bandwidth>,<sinr>,<rxlev>,<rsrp>,<rsrq>
/// ```
///
/// - `<rat>`：2 = WCDMA、4 = LTE/eMTC/NB-IoT、9 = NR（与 `band::bandwidth_text` 同口径）；
/// - `<band>` / `<bandwidth>` 是**编码值不是可读名**，交给 `band` 模块解；
///   `<band>` 实测可能为空，此时按频点反推频段（NR 只有这一条路）；
/// - 服务小区的 `<sinr>`（`<rssnr_value>` / `<ss_sinr>`）**本身就是 dB**
///   （-100 ~ 100，255 = 未上报）——LTE 制式下这是 SINR 的唯一来源，
///   `AT+CESQ` 不带这一格；
/// - `<rsrp>` / `<rsrq>` 是阶梯索引，换算口径与 `AT+CESQ` 同一套函数
///   （见 [`super::signal`]）；
/// - `<rxlev>` 在三种制式下都与 `<rsrp>` 同表（手册明示），不重复取；
/// - 邻区的 `<mcc>` / `<mnc>` 常为空，由 [`fill_neighbour_plmn`] 用服务小区补齐。

// ====================== 鼎桥 MT5700 系列（^MONSC / ^MONNC / ^CELLSCAN）======================
//
// 三条鼎桥私有指令的解析。格式依据（按可信度排序）：
//   1. luci-app-mt5700 的实机抓样（rpc.js parseMONSC，同一作者同型号）；
//   2. 官方手册《MT5700M-CN 5G 系列模组 AT 命令手册》V200R001C20 §13.9/13.10/5.35。
//
// 与 QENG 解析的关键差异（写错的后果是整行解析失败）：
//   · `^MONSC` 有**两种实测格式**（NR 与 LTE 的参数布局不同、4G 无 SINR 末位是 RSSI），
//     还有第三种「旧编码格式」（无前导制式、信号字段要按编码表还原）；
//   · PCI / TAC / Cell_ID 是**十六进制**，但部分固件按十进制上报 —— PCI 带越界回退
//     （先 hex，越出 3GPP 上限且 dec 合法才用 dec），ARFCN 直接十进制；
//   · `^CELLSCAN` 一行 15 个字段，band 是十六进制，测量值 99 = 无效，
//     LTE 的 SINR 在第 14 列（LTERSINR）、NR 在第 13 列（5GRSINR）。

/// PCI 十六进制解析（带越界回退，移植自 luci-app-mt5700 `parsePciHex`）。
///
/// 先按十六进制解析；仅当结果越出该制式上限、而十进制解释落在合法区间时才回退。
fn td_pci_hex(raw: &str, max_allowed: i32) -> i32 {
    let t = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
    if t.is_empty() {
        return 0;
    }
    let hex = i32::from_str_radix(t, 16).unwrap_or(-1);
    let dec = t.parse::<i32>().unwrap_or(-1);
    if hex < 0 {
        return if dec >= 0 { dec } else { 0 };
    }
    if dec < 0 {
        return hex;
    }
    if hex > max_allowed && dec <= max_allowed {
        dec
    } else {
        hex
    }
}

/// TAC/LAC 解析：**十进制优先、失败回退十六进制**。
///
/// 为什么与 PCI 相反：TAC 的固件行为不一致 —— 手册写十六进制，但实机抓样里
/// `12345` 是十进制（0x12345=74565 会差出一个数量级）。十进制优先对两边都安全：
/// 合法 TAC 的十进制串（如 12345）按 dec；真正的 hex 串（如 1A2B）dec 失败转 hex。
fn td_tac_dec_first(raw: &str) -> i32 {
    let t = raw.trim();
    if t.is_empty() {
        return 0;
    }
    t.parse::<i32>()
        .unwrap_or_else(|_| i32::from_str_radix(t, 16).unwrap_or(0))
}

/// 格式 B（旧编码）的信号还原表（移植自 luci-app-mt5700 的 convert*）。
fn td_convert_rsrp(raw: i32) -> i32 {
    if raw == 0 {
        -140
    } else if raw >= 97 {
        -44
    } else {
        -140 + raw
    }
}
fn td_convert_rsrq(raw: i32) -> i32 {
    if raw == 0 {
        -19
    } else if raw >= 34 {
        -3
    } else {
        -19 + raw / 2
    }
}
fn td_convert_sinr(raw: i32) -> i32 {
    let v = if raw == 0 {
        -20
    } else if raw >= 251 {
        30
    } else {
        -20 + raw / 5
    };
    v.clamp(-20, 30)
}
fn td_convert_rssi(raw: i32) -> i32 {
    if raw == 0 {
        -120
    } else if raw >= 96 {
        -25
    } else {
        -121 + raw
    }
}

/// 解析一行 `^MONSC: …`（服务小区）。
///
/// 实测格式（luci-app-mt5700 抓样）：
/// ```text
/// NR : ^MONSC: NR,460,00,501234,1,C1A2B3C,80,12345,-64,-10,25
///      NR,<mcc>,<mnc>,<arfcn>,<flag>,<cid_hex>,<pci_hex>,<tac_hex>,<rsrp>,<rsrq>,<sinr>
/// LTE: ^MONSC: LTE,460,00,38400,D975244,8,24C8,-85,-10,-54
///      LTE,<mcc>,<mnc>,<tac_hex>,<cid_hex>,<pci_hex>,<arfcn>,<rsrp>,<rsrq>,<rssi>
/// 旧  : ^MONSC: <mcc>,<mnc>,<lac>,<cid>,<pci>,<ch>,<rsrp_raw>,<rsrq_raw>,<sinr_raw>,<sysmode>
/// ```
pub fn parse_monsc(line: &str) -> Option<CellInfo> {
    let idx = line.find("^MONSC:")?;
    let body = line[idx + "^MONSC:".len()..].trim();
    if body.is_empty() {
        return None;
    }
    let f: Vec<String> = body.split(',').map(|x| x.trim().to_string()).collect();
    let first_is_num = f.first().map(|x| x.parse::<i32>().is_ok()).unwrap_or(false);
    let mut cell = CellInfo {
        serving: true,
        raw: Some(line.trim().to_string()),
        ..Default::default()
    };
    if !first_is_num {
        let mode = f[0].trim_matches('"').to_ascii_uppercase();
        if mode == "NONE" {
            return None; // 无服务
        }
        cell.tech = normalize_tech(&mode);
        if mode == "LTE" {
            // LTE,<mcc>,<mnc>,<tac_hex>,<cid_hex>,<pci_hex>,<arfcn>,<rsrp>,<rsrq>,<rssi>
            cell.mcc = f.get(1).cloned().unwrap_or_default();
            cell.mnc = f.get(2).cloned().unwrap_or_default();
            cell.tac = td_tac_dec_first(f.get(3).map(|x| x.as_str()).unwrap_or(""));
            cell.cell_id = f
                .get(4)
                .map(|x| x.trim().to_ascii_uppercase())
                .unwrap_or_default();
            cell.pcid = td_pci_hex(f.get(5).map(|x| x.as_str()).unwrap_or(""), 503);
            cell.earfcn = f.get(6).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
            cell.rsrp = f.get(7).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
            cell.rsrq = f.get(8).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
            cell.rssi = f.get(9).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        } else {
            // NR,<mcc>,<mnc>,<arfcn>,<flag>,<cid_hex>,<pci_hex>,<tac_hex>,<rsrp>,<rsrq>,<sinr>
            cell.mcc = f.get(1).cloned().unwrap_or_default();
            cell.mnc = f.get(2).cloned().unwrap_or_default();
            cell.earfcn = f.get(3).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
            cell.cell_id = f
                .get(5)
                .map(|x| x.trim().to_ascii_uppercase())
                .unwrap_or_default();
            cell.pcid = td_pci_hex(f.get(6).map(|x| x.as_str()).unwrap_or(""), 1007);
            cell.tac = td_tac_dec_first(f.get(7).map(|x| x.as_str()).unwrap_or(""));
            cell.rsrp = f.get(8).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
            cell.rsrq = f.get(9).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
            cell.sinr = f.get(10).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        }
    } else {
        // 旧编码格式（无前导制式）：信号字段要按编码表还原，制式在末位
        if f.len() < 10 {
            return None;
        }
        cell.mcc = f[0].clone();
        cell.mnc = f[1].clone();
        cell.tac = td_tac_dec_first(&f[2]);
        cell.cell_id = f[3].trim().to_ascii_uppercase();
        cell.pcid = f[4].trim().parse().unwrap_or(0);
        cell.earfcn = f[5].trim().parse().unwrap_or(0);
        cell.rsrp = td_convert_rsrp(f[6].trim().parse().unwrap_or(0));
        cell.rsrq = td_convert_rsrq(f[7].trim().parse().unwrap_or(0));
        cell.sinr = td_convert_sinr(f[8].trim().parse().unwrap_or(0));
        let mode = f[9].trim().trim_matches('"').to_ascii_uppercase();
        cell.tech = normalize_tech(&mode);
    }
    if cell.tech.is_empty() || !valid_plmn(&cell.mcc, &cell.mnc) {
        return None;
    }
    let op = operator_name(&cell.mcc, &cell.mnc);
    cell.operator = (!op.is_empty()).then_some(op);
    if cell.band == 0 {
        cell.band = band_from_earfcn(&cell.tech, cell.earfcn);
    }
    Some(cell)
}

/// 解析多行 `^MONNC: …`（邻区，格式与服务小区一致，手册 §13.10）。
///
/// 只取 LTE / NR 行 —— MT5700 是 5G 模组，GSM/WCDMA 邻区的参数布局（PSC/BSIC）
/// 在 [`CellInfo`] 里没有对应字段，硬塞会填错。
pub fn parse_monnc_rows(resp: &str) -> Vec<CellInfo> {
    let mut out = Vec::new();
    for line in resp.lines() {
        let line = line.trim();
        if !line.contains("^MONNC:") {
            continue;
        }
        // 行前缀是 ^MONNC:，复用服务小区解析前先换成 ^MONSC:
        let line = line.replacen("^MONNC:", "^MONSC:", 1);
        let Some(mut c) = parse_monsc(&line) else {
            continue;
        };
        if c.tech != "LTE" && c.tech != "NR" {
            continue;
        }
        c.serving = false;
        c.raw = Some(line.to_string());
        out.push(c);
    }
    out
}

/// 解析多行 `^CELLSCAN: …`（主动扫频，手册 §5.35；解析移植自 luci-app-mt5700
/// `parseScanLine`，其字段序是实机验证的）。
///
/// 15 字段：`<rat>,<plmn>,<freq>,[pci],<band>,<lac>,<cid>,[rxlev],[bsic],[psc],
/// [5GSCS],[5GRSRP],[5GRSRQ],[5GRSINR],[LTERSINR]`；band 十六进制；
/// 测量值 99 = 无效；LTE 的 SINR 在第 14 列（×0.125），NR 在第 13 列（×0.5）。
pub fn parse_cellscan_rows(resp: &str) -> Vec<CellInfo> {
    const RAT_NAMES: [&str; 4] = ["GSM", "WCDMA", "LTE", "NR"];
    const INVALID: i32 = 99;
    let mut out = Vec::new();
    for line in resp.lines() {
        let Some(idx) = line.find("^CELLSCAN:") else {
            continue;
        };
        let body = line[idx + "^CELLSCAN:".len()..].trim();
        if body.is_empty()
            || body.eq_ignore_ascii_case("STARTED")
            || body.eq_ignore_ascii_case("OK")
            || body.eq_ignore_ascii_case("RUNNING")
        {
            continue;
        }
        let mut f: Vec<String> = body.split(',').map(|x| x.trim().to_string()).collect();
        while f.len() < 15 {
            f.push(String::new());
        }
        let Some(rat) = f[0].trim().parse::<usize>().ok().filter(|r| *r < 4) else {
            continue;
        };
        let tech = RAT_NAMES[rat].to_string();
        if tech != "LTE" && tech != "NR" {
            continue; // GSM/WCDMA 行不进 CellInfo（同 parse_monnc_rows 的口径）
        }
        let dec = |v: &str| v.trim().parse::<i32>().ok();
        // 测量值：99 视为无效；scale=1 原值，否则乘系数取整
        let measure = |v: &str, scale: f64| -> i32 {
            let n = dec(v).unwrap_or(INVALID);
            if n == INVALID {
                return 0;
            }
            (n as f64 * scale) as i32
        };
        let plmn = f[1].trim_matches('"').to_string();
        // 频段：十六进制，1~512
        let band = i32::from_str_radix(f[4].trim(), 16)
            .ok()
            .filter(|b| *b > 0 && *b <= 512)
            .unwrap_or(0);
        let mut c = CellInfo {
            tech: tech.clone(),
            serving: false,
            raw: Some(line.trim().to_string()),
            ..Default::default()
        };
        // PLMN 前 3 位 MCC、余下 MNC
        if plmn.len() >= 5 {
            c.mcc = plmn[..3].to_string();
            c.mnc = plmn[3..].to_string();
        }
        c.earfcn = dec(&f[2]).unwrap_or(0);
        c.pcid = dec(&f[3]).unwrap_or(0);
        c.band = band;
        c.cell_id = f[6].trim().to_ascii_uppercase();
        c.rssi = dec(&f[7]).unwrap_or(0);
        c.rsrp = measure(&f[11], 1.0);
        c.rsrq = measure(&f[12], 0.5);
        c.sinr = if rat == 2 {
            measure(&f[14], 0.125)
        } else {
            measure(&f[13], 0.5)
        };
        let op = operator_name(&c.mcc, &c.mnc);
        c.operator = (!op.is_empty()).then_some(op);
        if !valid_plmn(&c.mcc, &c.mnc) {
            continue;
        }
        out.push(c);
    }
    out
}

pub fn parse_gtccinfo_rows(resp: &str) -> Vec<CellInfo> {
    use super::signal::{
        lte_rsrp_index_dbm, lte_rsrq_index_db, nr_ss_rsrp_index_dbm, nr_ss_rsrq_index_db,
    };

    let mut out = Vec::new();
    for line in resp.lines() {
        let mut l = line.trim();
        if l.is_empty() || l.eq_ignore_ascii_case("OK") {
            continue;
        }
        // 剥掉 `+GTCCINFO:` 前缀：头行与数据行都带（兼容已被上层剥掉的形式）
        if let Some((head, rest)) = l.split_once(':') {
            if head.trim().starts_with("+GTCCINFO") {
                l = rest.trim();
            }
        }
        // 数据行以角色数字开头；其余（表头、空行、URC）一律跳过
        if !l.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let f = csv(l);
        if f.len() < 10 {
            continue;
        }
        let serving = match f[0].trim() {
            "1" => true,
            "2" => false,
            _ => continue,
        };
        let rat = f[1].trim();
        let tech = match rat {
            "2" => "WCDMA",
            "4" => "LTE",
            "9" => "NR",
            _ => continue,
        };
        let Some(cols) = gtccinfo_cols(rat, serving) else {
            continue;
        };
        let col = |c: Option<usize>| c.and_then(|i| f.get(i)).map(|s| s.as_str());

        let mut cell = CellInfo {
            tech: tech.to_string(),
            serving,
            mcc: f[2].trim().to_string(),
            mnc: f[3].trim().to_string(),
            cell_id: f[5].trim().to_ascii_uppercase(),
            tac: parse_at_int(&f[4]).unwrap_or(0),
            earfcn: to_i32(&f[6]).unwrap_or(0),
            pcid: parse_at_int(&f[7]).unwrap_or(0),
            raw: Some(l.to_string()),
            ..Default::default()
        };

        // 频段：先试 <band> 编码列（WCDMA 是裸频段号，LTE / NR 是拼接编码），
        // 为空或解不出时按频点反推
        let mut band = 0;
        if let Some(raw_band) = col(cols.band).filter(|s| !s.is_empty()) {
            band = match tech {
                "NR" => band::nr_band_from_code(raw_band).map(|s| band_no_of(&s)),
                "LTE" => band::lte_band_from_code(raw_band).map(|s| band_no_of(&s)),
                "WCDMA" => raw_band.parse::<i32>().ok(),
                _ => None,
            }
            .filter(|v| *v > 0)
            .unwrap_or(0);
        }
        if band == 0 && tech != "WCDMA" {
            band = band_from_earfcn(tech, cell.earfcn);
        }
        cell.band = band;

        // 带宽编码 → MHz（解不出留 0，原始码保留在 raw 里）
        if let Some(raw_bw) = col(cols.bw).filter(|s| !s.is_empty()) {
            if let Some(text) = band::bandwidth_text(rat, raw_bw) {
                cell.bandwidth = text
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }

        // RSRP / RSRQ：阶梯索引，与 AT+CESQ 同一套换算
        if let Some(v) = col(cols.rsrp).and_then(|s| s.parse::<i32>().ok()) {
            cell.rsrp = if tech == "NR" {
                nr_ss_rsrp_index_dbm(v)
            } else {
                lte_rsrp_index_dbm(v)
            }
            .unwrap_or(0);
        }
        if let Some(v) = col(cols.rsrq).and_then(|s| s.parse::<i32>().ok()) {
            cell.rsrq = if tech == "NR" {
                nr_ss_rsrq_index_db(v)
            } else {
                lte_rsrq_index_db(v)
            }
            .unwrap_or(0);
        }

        // SINR：模组直接上报 dB（-100 ~ 100，越界视为未上报）
        if let Some(v) = col(cols.sinr).and_then(|s| s.parse::<i32>().ok()) {
            if (-100..=100).contains(&v) {
                cell.sinr = v;
            }
        }

        if !cell.mcc.is_empty() && !cell.mnc.is_empty() {
            let op = operator_name(&cell.mcc, &cell.mnc);
            cell.operator = (!op.is_empty()).then_some(op);
        }
        out.push(cell);
    }
    out
}

/// 用服务小区的 PLMN/运营商补齐邻小区。
pub fn fill_neighbour_plmn(serving: &[CellInfo], neighbours: &mut [CellInfo]) {
    let Some(base) = serving
        .iter()
        .find(|c| !c.mcc.is_empty() && !c.mnc.is_empty())
    else {
        return;
    };
    let op = base
        .operator
        .clone()
        .unwrap_or_else(|| operator_name(&base.mcc, &base.mnc));
    for n in neighbours.iter_mut() {
        if n.mcc.is_empty() || n.mnc.is_empty() {
            n.mcc = base.mcc.clone();
            n.mnc = base.mnc.clone();
            if n.operator.is_none() && !op.is_empty() {
                n.operator = Some(op.clone());
            }
        }
    }
}

/// 按频点是否与服务小区相同补上同频邻区标记。
pub fn mark_intra_frequency(serving: &[CellInfo], neighbours: &mut [CellInfo]) {
    let flag: Vec<i32> = serving
        .iter()
        .filter(|c| c.earfcn > 0)
        .map(|c| c.earfcn)
        .collect();
    if flag.is_empty() {
        return;
    }
    for n in neighbours.iter_mut() {
        if n.earfcn > 0 && flag.contains(&n.earfcn) {
            n.intra_cell = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_cellscan_rows, parse_monnc_rows, parse_monsc};

    // ---------- 鼎桥 MT5700（^MONSC / ^MONNC / ^CELLSCAN）----------

    #[test]
    fn monsc_nr_layout_matches_field_test() {
        // luci-app-mt5700 实机抓样：NR,<mcc>,<mnc>,<arfcn>,<flag>,<cid_hex>,<pci_hex>,<tac_hex>,<rsrp>,<rsrq>,<sinr>
        let c = parse_monsc("^MONSC: NR,460,00,501234,1,C1A2B3C,80,12345,-64,-10,25").expect("nr");
        assert_eq!(c.tech, "NR");
        assert_eq!(c.mcc, "460");
        assert_eq!(c.mnc, "00");
        assert_eq!(c.earfcn, 501234);
        assert_eq!(c.cell_id, "C1A2B3C");
        assert_eq!(c.pcid, 128, "PCI 十六进制 0x80 → 128");
        assert_eq!(c.tac, 12345);
        assert_eq!(c.rsrp, -64);
        assert_eq!(c.rsrq, -10);
        assert_eq!(c.sinr, 25);
        assert!(c.serving);
    }

    #[test]
    fn monsc_lte_layout_has_rssi_not_sinr() {
        // LTE 布局：LTE,<mcc>,<mnc>,<tac_hex>,<cid_hex>,<pci_hex>,<arfcn>,<rsrp>,<rsrq>,<rssi>
        // 套 NR 布局会全部错位（实测过的坑）
        let c = parse_monsc("^MONSC: LTE,460,00,24C8,D975244,8,38400,-85,-10,-54").expect("lte");
        assert_eq!(c.tech, "LTE");
        assert_eq!(c.tac, 0x24C8);
        assert_eq!(c.cell_id, "D975244");
        assert_eq!(c.pcid, 8);
        assert_eq!(c.earfcn, 38400);
        assert_eq!(c.rsrp, -85);
        assert_eq!(c.rsrq, -10);
        assert_eq!(c.rssi, -54);
        assert_eq!(c.sinr, 0, "LTE 无 SINR 字段");
    }

    #[test]
    fn monsc_pci_hex_falls_back_to_decimal() {
        // 固件按十进制上报 500 时：hex 解析 0x500=1280 越出 LTE 上限 503 → 回退十进制
        let c = parse_monsc("^MONSC: LTE,460,00,24C8,D975244,1F4,38400,-85,-10,-54").expect("pci");
        assert_eq!(c.pcid, 500, "0x1F4=500 恰好合法 → 用 hex");
        // 真正越界：0x500 → 1280 > 503，dec=500 ≤ 503 → 回退
        let c = parse_monsc("^MONSC: LTE,460,00,24C8,D975244,500,38400,-85,-10,-54").expect("pci2");
        assert_eq!(c.pcid, 500);
    }

    #[test]
    fn monsc_none_and_garbage() {
        assert!(parse_monsc("^MONSC: NONE").is_none(), "无服务");
        assert!(parse_monsc("+CME ERROR: 100").is_none());
        assert!(parse_monsc("^MONSC: LTE").is_none(), "缺参数");
    }

    #[test]
    fn monnc_rows_filter_non_lte_nr() {
        let resp = "^MONNC: LTE,460,00,24C8,D975244,8,38400,-90,-12,-60\r\n\
                    ^MONNC: NR,460,00,501234,1,C1A2B3C,80,12345,-70,-8,18\r\n\
                    ^MONNC: GSM,460,00,3,1234,A1B2,45\r\nOK";
        let v = parse_monnc_rows(resp);
        assert_eq!(v.len(), 2, "GSM 行不进（CellInfo 无对应字段）");
        assert!(v.iter().all(|c| !c.serving));
        assert_eq!(v[0].tech, "LTE");
        assert_eq!(v[1].tech, "NR");
    }

    #[test]
    fn cellscan_row_is_15_fields() {
        // 一行 15 字段（手册 §5.35）；LTE 的 SINR 在 f[14]（LTERSINR）
        let line = "^CELLSCAN: 2,46001,38400,128,29,12AB,34CD,-78,0,0,,-80,-12,-9,-6";
        let v = parse_cellscan_rows(line);
        assert_eq!(v.len(), 1);
        let c = &v[0];
        assert_eq!(c.tech, "LTE");
        assert_eq!(c.mcc, "460");
        assert_eq!(c.mnc, "01");
        assert_eq!(c.earfcn, 38400);
        assert_eq!(c.pcid, 128);
        assert_eq!(c.band, 0x29, "band 十六进制");
        assert_eq!(c.rsrp, -80);
        assert_eq!(c.rsrq, -6, "RSRQ ×0.5：-12×0.5=-6");
        assert_eq!(c.sinr, 0, "LTE 的 SINR 取 f[14]（-6×0.125≈0 取整）");
    }

    #[test]
    fn cellscan_skips_state_lines_and_invalid_measures() {
        let resp = "^CELLSCAN: STARTED\r\n^CELLSCAN: RUNNING\r\n\
                    ^CELLSCAN: 3,46000,501234,80,29,,1A2B,,-99,0,1,-110,-8,25,\r\nOK";
        let v = parse_cellscan_rows(resp);
        assert_eq!(v.len(), 1, "STARTED/RUNNING/OK 行跳过");
        let c = &v[0];
        assert_eq!(c.tech, "NR");
        assert_eq!(c.rsrp, -110);
        assert_eq!(c.sinr, 12, "NR 的 SINR 取 f[13]（×0.5：25×0.5=12）");
        assert_eq!(c.rsrq, -4, "-8×0.5=-4");
    }

    use super::{
        band_from_earfcn, mark_intra_frequency, operator_name, parse_cell_line, parse_cell_lines,
        parse_cpsi_cell, parse_gtccinfo_rows,
    };

    #[test]
    fn serving_lte() {
        let line = "+QENG: \"servingcell\",\"NOCONN\",\"LTE\",\"FDD\",460,11,400E21E,155,2452,5,3,3,404A,-104,-16,-69,7,0,-,19";
        let c = parse_cell_line(line).unwrap();
        assert!(c.serving);
        assert_eq!(c.tech, "LTE");
        assert_eq!(c.state, "NOCONN");
        assert_eq!((c.mcc.as_str(), c.mnc.as_str()), ("460", "11"));
        assert_eq!(c.cell_id, "400E21E");
        assert_eq!(c.pcid, 155);
        assert_eq!(c.earfcn, 2452);
        assert_eq!(c.band, 5);
        assert_eq!(c.tac, 0x404A);
        assert_eq!(c.rsrp, -104);
        assert_eq!(c.rsrq, -16);
        assert_eq!(c.rssi, -69);
        assert_eq!(c.sinr, 7);
        assert_eq!(c.operator.as_deref(), Some("中国电信"));
    }

    #[test]
    fn serving_nr() {
        let line = "+QENG: \"servingcell\",\"NR\",\"FDD\",460,00,1051E8F2,123,0,-95,-12,14,35";
        let c = parse_cell_line(line).unwrap();
        assert_eq!(c.tech, "NR");
        assert_eq!(c.mcc, "460");
        assert_eq!(c.mnc, "00");
        assert_eq!(c.cell_id, "1051E8F2");
        assert_eq!(c.tac, 123);
        assert_eq!(c.rsrp, -95);
        assert_eq!(c.rsrq, -12);
        assert_eq!(c.sinr, 14);
    }

    #[test]
    fn neighbour_lte_dimension_order() {
        let line = "+QENG: \"neighbourcell intra\",\"LTE\",2452,155,-15,-103,-70,-,21,3,20,2,54";
        let c = parse_cell_line(line).unwrap();
        assert!(!c.serving);
        assert!(c.intra_cell);
        assert_eq!(c.earfcn, 2452);
        assert_eq!(c.band, 5);
        assert_eq!(c.pcid, 155);
        assert_eq!(c.rsrp, -103);
        assert_eq!(c.rsrq, -15);
        assert_eq!(c.rssi, -70);
        assert_eq!(c.sinr, 21);
    }

    #[test]
    fn split_serving_header_plus_data() {
        let resp = "+QENG: \"servingcell\",\"CONNECT\"\r\n+QENG: \"LTE\",\"FDD\",460,11,3FA6836,140,1850,3,5,5,404A,-117,-14,-83,-2,6,22,7\r\n+QENG: \"NR5G-NSA\",0,0,0,0,0,0,0,0\r\nOK\r\n";
        let cells = parse_cell_lines(resp);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].state, "CONNECT");
        assert_eq!(cells[0].mcc, "460");
        assert_eq!(cells[0].pcid, 140);
        assert_eq!(cells[0].earfcn, 1850);
        assert_eq!(cells[0].band, 3);
        assert_eq!(cells[0].tac, 0x404A);
        assert_eq!(cells[0].rsrp, -117);
    }

    #[test]
    fn cpsi_numeric() {
        let line = "+CPSI: NR5G,Online,460-11,0x1051E8F2,627264,504,78,100,-,-95,-12,-7,13";
        let c = parse_cpsi_cell(line).unwrap();
        assert_eq!(c.tech, "NR");
        assert_eq!((c.mcc.as_str(), c.mnc.as_str()), ("460", "11"));
        assert_eq!(c.cell_id, "1051E8F2");
        assert_eq!(c.earfcn, 627264);
        assert_eq!(c.pcid, 504);
        assert_eq!(c.band, 78);
        assert_eq!(c.rsrp, -95);
        assert_eq!(c.rsrq, -12);
        assert_eq!(c.sinr, -7);
    }

    #[test]
    fn band_and_operator() {
        assert_eq!(band_from_earfcn("LTE", 1850), 3);
        assert_eq!(band_from_earfcn("LTE", 2452), 5);
        assert_eq!(band_from_earfcn("NR", 627264), 78);
        assert_eq!(band_from_earfcn("NR", 176190), 5);
        assert_eq!(operator_name("460", "00"), "中国移动");
        assert_eq!(operator_name("460", "03"), "中国电信");
    }

    #[test]
    fn intra_marking() {
        use super::CellInfo;
        let serving = vec![CellInfo {
            earfcn: 627264,
            ..Default::default()
        }];
        let mut neighbours = vec![
            CellInfo {
                earfcn: 627264,
                ..Default::default()
            },
            CellInfo {
                earfcn: 636666,
                ..Default::default()
            },
        ];
        mark_intra_frequency(&serving, &mut neighbours);
        assert!(neighbours[0].intra_cell);
        assert!(!neighbours[1].intra_cell);
    }

    #[test]
    fn gtccinfo_serving_lte_real_device() {
        // FM350-GL 实机抓包（LTE 服务小区，<band> / <bandwidth> 为空）
        let line = "1,4,460,1,7B10,006414DEE,3740,119,,,-5,23,23,9";
        let cells = parse_gtccinfo_rows(line);
        assert_eq!(cells.len(), 1);
        let c = &cells[0];
        assert!(c.serving);
        assert_eq!(c.tech, "LTE");
        assert_eq!((c.mcc.as_str(), c.mnc.as_str()), ("460", "1"));
        assert_eq!(c.tac, 0x7B10);
        assert_eq!(c.cell_id, "006414DEE");
        assert_eq!(c.earfcn, 3740);
        assert_eq!(c.pcid, 119);
        assert_eq!(c.band, 8); // <band> 为空 → 由 EARFCN 反推
        assert_eq!(c.bandwidth, 0);
        assert_eq!(c.sinr, -5); // <rssnr_value> 本身就是 dB
        assert_eq!(c.rsrp, -118); // 索引 23 → -140 + (23-1)
        assert_eq!(c.rsrq, -15); // 索引 9 → -19.5 + (9-1)*0.5 = -15.5，截断
    }

    #[test]
    fn gtccinfo_neighbour_lte_real_device() {
        // 实机抓包的 LTE 邻区行：没有 <band> 也没有 <rssnr> 列
        let line = "2,4,,,FFFF,00FFFFFFF,3740,172,,22,22,6";
        let cells = parse_gtccinfo_rows(line);
        assert_eq!(cells.len(), 1);
        let c = &cells[0];
        assert!(!c.serving);
        assert_eq!(c.tech, "LTE");
        assert_eq!(c.earfcn, 3740);
        assert_eq!(c.pcid, 172);
        assert_eq!(c.rsrp, -119); // 索引 22
        assert_eq!(c.rsrq, -17); // 索引 6 → -19.5 + (6-1)*0.5
        assert_eq!(c.sinr, 0); // 邻区没有 SINR 列
    }

    #[test]
    fn gtccinfo_nr_serving_reference_sample() {
        // 参考实现实机样本（NR 服务小区，<band> 为空靠 NR-ARFCN 反推）
        let line = "1,9,460,0,149002,C2840C002,504990,128,,,15,74,74,64";
        let cells = parse_gtccinfo_rows(line);
        assert_eq!(cells.len(), 1);
        let c = &cells[0];
        assert!(c.serving);
        assert_eq!(c.tech, "NR");
        assert_eq!(c.tac, 149002);
        assert_eq!(c.cell_id, "C2840C002");
        assert_eq!(c.earfcn, 504990);
        assert_eq!(c.pcid, 128);
        assert_eq!(c.band, 41); // 504990 → 2524.95 MHz → n41
        assert_eq!(c.sinr, 15);
        assert_eq!(c.rsrp, -83); // 索引 74 → -156 + 73
        assert_eq!(c.rsrq, -12); // 索引 64 → -43 + 63*0.5 = -11.5，四舍五入
    }

    #[test]
    fn gtccinfo_nr_band_and_bandwidth_codes() {
        // <band> = 5041 → n41；<bandwidth> = 500 → 100 MHz（手册 p191 编码）
        let line = "1,9,460,11,149002,C2840C002,504990,128,5041,500,15,74,74,64";
        let c = &parse_gtccinfo_rows(line)[0];
        assert_eq!(c.band, 41);
        assert_eq!(c.bandwidth, 100);
        assert_eq!(c.operator.as_deref(), Some("中国电信"));
    }

    #[test]
    fn gtccinfo_batch_skips_header_and_ok() {
        // 整包抓样：头行 + 1 个服务小区 + 3 个邻区
        let resp = "+GTCCINFO: \r\n1,4,460,1,7B10,006414DEE,3740,119,,,-5,23,23,9\r\n2,4,,,FFFF,00FFFFFFF,3740,172,,22,22,6\r\n2,4,,,FFFF,00FFFFFFF,3740,254,,19,19,0\r\n2,4,,,FFFF,00FFFFFFF,1800,294,,13,13,2\r\n\r\nOK\r\n";
        let cells = parse_gtccinfo_rows(resp);
        assert_eq!(cells.len(), 4);
        assert_eq!(cells.iter().filter(|c| c.serving).count(), 1);
        assert_eq!(cells.iter().filter(|c| !c.serving).count(), 3);
        assert_eq!(cells[3].earfcn, 1800);
        assert_eq!(cells[3].band, 3);
    }

    #[test]
    fn gtccinfo_skips_noise_lines() {
        let resp = "AT+GTCCINFO?\r\n\r\n+GTCCINFO: \r\n\r\n+GTCCINFO: \r\nOK\r\n";
        assert!(parse_gtccinfo_rows(resp).is_empty());
        // 角色列不是 1/2、制式列认不出的行一律跳过
        assert!(parse_gtccinfo_rows("3,4,460,1,7B10,006414DEE,3740,119,,,-5,23,23,9").is_empty());
        assert!(parse_gtccinfo_rows("1,7,460,1,7B10,006414DEE,3740,119,,,-5,23,23,9").is_empty());
        // 字段不足 10 列的残行也不硬解
        assert!(parse_gtccinfo_rows("1,4,460,1,7B10").is_empty());
    }
}
