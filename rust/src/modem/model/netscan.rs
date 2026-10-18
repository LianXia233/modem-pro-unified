//! 主动扫网（`GET /api/netscan`、`POST /api/netscan/stop`）。
//!
//! 与「实时扫描」（`AT+QENG` 那套，`/api/cells`）的区别：这里让**模组自己**去搜全网
//! （`AT+COPS=?`），可能要几分钟，而且这段时间**串口被它独占**（其它查询会排队）。
//! 所以它必须能被「停止扫描」打断 —— 中断后已收到的部分照样解析。
//!
//! 制式（`lte` / `nr` / `both`）只影响**结果过滤与步骤**：一次 `AT+COPS=?` 会把所有制式
//! 的可见网络一起报回来，切制式去改模组的网络模式会让正在上网的设备掉线，不划算。
//! 界面上「LTE+NR = 两个制式各扫一遍再合并」指的是把两个制式的结果都列出来（不去重）。
//!
//! 主动扫小区只有部分固件提供（实测芯讯通 `AT+CNWSEARCH`）；没有就给 `cell_note`，
//! 界面会照原话显示，不假装扫到了东西。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::super::at::{csv, extract_all, extract_field, is_ok};
use super::cells::CellInfo;
use super::modem::Modem;

/// 制式（与界面下拉一致，内部统一小写）。
pub const TECH_LTE: &str = "lte";
pub const TECH_NR: &str = "nr";
pub const TECH_BOTH: &str = "both";

/// 主动扫网的等待时长下限 / 上限（分钟）。
pub const WAIT_MIN: i64 = 1;
pub const WAIT_MAX: i64 = 10;

/// 一个可见运营商（`AT+COPS=?` 的一项）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Operator {
    /// 0 未知 / 1 可用 / 2 当前在网 / 3 禁用
    pub status: i32,
    pub long: String,
    pub short: String,
    pub numeric: String,
    /// 接入技术（原样保留模组给的数字，界面另有 `act` 列）
    pub act: String,
    /// 归一化制式：`lte` / `nr` / `wcdma` / `gsm` / 空
    pub tech: String,
}

/// 一次主动扫网的结果。
#[derive(Debug, Clone, Default, Serialize)]
pub struct NetScanResult {
    pub modem_id: u32,
    /// 回显的制式（大写：`LTE` / `NR` / `BOTH`）
    pub tech: String,
    pub scanned_at: String,
    pub duration_ms: i64,
    /// 本次实际允许的等待时长（毫秒；界面用它与「耗时」区分开）
    pub wait_ms: i64,
    pub operators: Vec<Operator>,
    pub operator_cmd: String,
    pub cells: Vec<CellInfo>,
    pub cell_cmd: String,
    pub cell_note: String,
    /// 做了哪些步骤（界面「原始应答」上方的时间线）
    pub steps: Vec<String>,
    pub raw: Vec<String>,
    /// 是否是被「停止扫描」打断的
    pub stopped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Modem {
    /// 主动扫网（同步阻塞，最长 `minutes` 分钟）。
    ///
    /// 期间持有本模组的 AT 口锁 —— 别的查询会在这里排队，与界面文案一致。
    pub fn scan_network(&self, tech: &str, minutes: i64) -> NetScanResult {
        let started = Instant::now();
        let wait_min = minutes.clamp(WAIT_MIN, WAIT_MAX);
        let wait = Duration::from_secs(wait_min as u64 * 60);
        let tech = normalize_tech(tech);
        let flag = self.begin_netscan();

        let mut out = NetScanResult {
            modem_id: self.id,
            tech: tech.to_ascii_uppercase(),
            scanned_at: crate::logging::timestamp(),
            wait_ms: wait.as_millis() as i64,
            operator_cmd: "AT+COPS=?".to_string(),
            ..Default::default()
        };

        // 长名格式：短名/数字两种固件都有，统一要长名，界面上名字更好认
        let _ = self.at_cmd("AT+COPS=3,0");
        out.steps
            .push("设置运营商名为长名格式（AT+COPS=3,0）".to_string());

        let (resp, stopped) =
            self.at_cmd_cancel("AT+COPS=?", wait, &|| flag.load(Ordering::Relaxed));
        out.raw.push(resp.trim().to_string());
        out.stopped = stopped;
        if stopped {
            out.steps.push("收到停止指令，返回已扫到的部分".to_string());
        } else if is_ok(&resp) {
            out.steps
                .push("可见运营商搜索完成（AT+COPS=?）".to_string());
        } else {
            out.steps
                .push("可见运营商搜索未正常结束（模组无应答或超时）".to_string());
        }

        let mut ops = parse_cops_scan(&resp);
        if tech != TECH_BOTH {
            ops.retain(|o| o.tech == tech);
        }
        out.operators = ops;

        // 主动扫小区：只有少数固件提供，扫不到就原话说明
        let (cells, cmd, note) = self.scan_net_cells(wait, &flag);
        out.cells = cells;
        out.cell_cmd = cmd;
        out.cell_note = note;

        if out.operators.is_empty() {
            out.note = Some(if stopped {
                "已按你的要求停止扫描，结果可能不完整".to_string()
            } else {
                "没有扫到可见运营商（模组可能正在数据业务中，或该固件不支持）".to_string()
            });
        } else if stopped {
            out.note = Some("已按你的要求停止扫描，结果可能不完整".to_string());
        }

        self.end_netscan();
        out.duration_ms = started.elapsed().as_millis() as i64;
        out
    }

    /// 主动扫小区（仅支持该指令的固件；不支持时给出说明文案）。
    fn scan_net_cells(&self, wait: Duration, flag: &AtomicBool) -> (Vec<CellInfo>, String, String) {
        // 只有实测支持的那家给指令；其余固件去试只会白占串口几十秒
        let Some(cmd) = self.vendor().spec().cell_scan_cmd() else {
            return (
                Vec::new(),
                String::new(),
                "本固件没有提供主动扫小区结果（该能力仅部分固件支持）".to_string(),
            );
        };
        let (resp, stopped) = self.at_cmd_cancel(cmd, wait, &|| flag.load(Ordering::Relaxed));
        let cells = parse_cnwsearch(&resp);
        let note = if !cells.is_empty() {
            String::new()
        } else if stopped {
            "已按你的要求停止扫描，没有取到小区结果".to_string()
        } else {
            "本固件没有返回可解析的小区结果（AT+CNWSEARCH）".to_string()
        };
        (cells, cmd.to_string(), note)
    }

    /// 请求停止正在进行的扫网（没有在跑返回 false）。
    pub fn request_netscan_stop(&self) -> bool {
        match self.netscan_stop.lock().unwrap().as_ref() {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// 登记本次扫描的取消标志（同一模组同时只该有一个扫描在跑）。
    fn begin_netscan(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        *self.netscan_stop.lock().unwrap() = Some(flag.clone());
        flag
    }

    fn end_netscan(&self) {
        *self.netscan_stop.lock().unwrap() = None;
    }
}

/// 解析 `AT+COPS=?`：`+COPS: (2,"CHN-UNICOM","UNICOM","46001",7),(1,...),...`
///
/// 括号里可能带引号也可能不带，逐组切分后按位置取值（顺序是 3GPP 定死的：
/// 状态 / 长名 / 短名 / 数字 / ACT）。
pub fn parse_cops_scan(resp: &str) -> Vec<Operator> {
    let Some(payload) = extract_field(resp, "+COPS") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for group in split_groups(&payload) {
        let f = csv(&group);
        if f.len() < 2 {
            continue;
        }
        let status = f.first().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let long = f.get(1).cloned().unwrap_or_default();
        let short = f.get(2).cloned().unwrap_or_default();
        let numeric = f.get(3).cloned().unwrap_or_default();
        let act = f.get(4).cloned().unwrap_or_default();
        out.push(Operator {
            status,
            tech: act_tech(&act).to_string(),
            long,
            short,
            numeric,
            act,
        });
    }
    out
}

/// 把 `(a,b),(c,d)` 切成 `["a,b", "c,d"]`。
fn split_groups(payload: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in payload.chars() {
        match ch {
            '(' => {
                depth += 1;
                // 只收括号里的内容
                if depth == 1 {
                    continue;
                }
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                if depth <= 0 {
                    if !cur.trim().is_empty() {
                        out.push(cur.trim().to_string());
                    }
                    cur.clear();
                    depth = 0;
                    continue;
                }
                cur.push(ch);
            }
            _ if depth >= 1 => cur.push(ch),
            _ => {}
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// ACT 数字 → 制式（3GPP TS 27.007 的 `<AcT>`）。
pub fn act_tech(act: &str) -> &'static str {
    match act.trim().trim_matches('"').parse::<i32>().unwrap_or(-1) {
        7 => TECH_LTE,
        12 => TECH_NR,
        2 | 4 | 5 | 6 => "wcdma",
        0 | 1 | 3 => "gsm",
        _ => "",
    }
}

/// 制式参数归一化（认不出的按 both，不让界面选了个值却什么都不显示）。
pub fn normalize_tech(tech: &str) -> &'static str {
    match tech.trim().to_ascii_lowercase().as_str() {
        TECH_LTE => TECH_LTE,
        "nr" | "nr5g" | "5g" => TECH_NR,
        _ => TECH_BOTH,
    }
}

/// 解析主动扫小区的结果（`+CNWSEARCH: ...`）。
///
/// ⚠️ 这是厂商私有指令，字段顺序各固件不一，所以**按量纲定位**：
/// PLMN 是 5~6 位数字、频点是大整数、RSRP 落在 -156..-44、PCI 落在 0..1007。
/// 认不出的行直接跳过（宁可少列，也不要把速率当成频点显示出来）。
pub fn parse_cnwsearch(resp: &str) -> Vec<CellInfo> {
    let mut out = Vec::new();
    for line in extract_all(resp, "+CNWSEARCH") {
        let f = csv(&line);
        if f.len() < 3 {
            continue;
        }
        let mut plmn = None;
        let mut earfcn = None;
        let mut pcid = None;
        let mut rsrp = None;
        let mut rsrq = None;
        let mut tech = String::new();
        // 第一个字段是序号 / 状态，不参与量纲判定
        for raw in f.iter().skip(1) {
            let t = raw.trim().trim_matches('"');
            if t.is_empty() {
                continue;
            }
            let Ok(v) = t.parse::<i64>() else {
                if tech.is_empty() {
                    tech = t.to_string();
                }
                continue;
            };
            if plmn.is_none() && t.len() >= 5 && t.len() <= 6 && v > 0 {
                plmn = Some(t.to_string());
            } else if earfcn.is_none() && v > 1000 && v < 10_000_000 {
                earfcn = Some(v);
            } else if pcid.is_none() && v > 0 && v <= 1007 {
                pcid = Some(v);
            } else if rsrp.is_none() && (-156..=-44).contains(&v) {
                rsrp = Some(v);
            } else if rsrq.is_none() && (-43..=20).contains(&v) {
                rsrq = Some(v);
            }
        }
        if plmn.is_none() && earfcn.is_none() {
            continue;
        }
        let plmn = plmn.unwrap_or_default();
        out.push(CellInfo {
            tech: normalize_cell_tech(&tech),
            mcc: plmn.chars().take(3).collect(),
            mnc: plmn.chars().skip(3).collect(),
            pcid: pcid.unwrap_or(0) as i32,
            earfcn: earfcn.unwrap_or(0) as i32,
            rsrp: rsrp.unwrap_or(0) as i32,
            rsrq: rsrq.unwrap_or(0) as i32,
            serving: false,
            raw: Some(line.clone()),
            ..Default::default()
        });
    }
    out
}

fn normalize_cell_tech(tech: &str) -> String {
    let up = tech.to_ascii_uppercase();
    if up.contains("NR") {
        "NR".to_string()
    } else if up.contains("LTE") {
        "LTE".to_string()
    } else if up.contains("WCDMA") {
        "WCDMA".to_string()
    } else if up.contains("GSM") {
        "GSM".to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{act_tech, normalize_tech, parse_cnwsearch, parse_cops_scan, split_groups};
    use crate::modem::model::CellInfo;

    const COPS: &str = r#"+COPS: (2,"CHN-UNICOM","UNICOM","46001",7),(1,"CHINA MOBILE","CMCC","46000",7),(3,"46011","46011","46011",12),(1,"CHN-CT","CT","46003",0)
OK"#;

    #[test]
    fn groups_are_split_by_paren() {
        let g = split_groups("(1,\"A\",\"a\",\"46000\",7),(2,\"B\",\"b\",\"46001\",12)");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0], "1,\"A\",\"a\",\"46000\",7");
        assert_eq!(g[1], "2,\"B\",\"b\",\"46001\",12");
    }

    #[test]
    fn cops_scan_parses_operators() {
        let ops = parse_cops_scan(COPS);
        assert_eq!(ops.len(), 4);
        assert_eq!(ops[0].long, "CHN-UNICOM");
        assert_eq!(ops[0].short, "UNICOM");
        assert_eq!(ops[0].numeric, "46001");
        assert_eq!(ops[0].act, "7");
        assert_eq!(ops[0].tech, "lte");
        assert_eq!(ops[0].status, 2);
        // 数字型 PLMN 与 NR 制式
        assert_eq!(ops[2].numeric, "46011");
        assert_eq!(ops[2].tech, "nr");
        assert_eq!(ops[2].status, 3);
        // GSM（ACT=0）
        assert_eq!(ops[3].tech, "gsm");
    }

    #[test]
    fn cops_scan_handles_empty_and_errors() {
        assert!(parse_cops_scan("ERROR").is_empty());
        assert!(parse_cops_scan("+COPS: ()").is_empty());
    }

    #[test]
    fn act_numbers_map_to_tech() {
        assert_eq!(act_tech("7"), "lte");
        assert_eq!(act_tech("12"), "nr");
        assert_eq!(act_tech("\"6\""), "wcdma");
        assert_eq!(act_tech("0"), "gsm");
        assert_eq!(act_tech("x"), "");
    }

    #[test]
    fn tech_param_normalization() {
        assert_eq!(normalize_tech("LTE"), "lte");
        assert_eq!(normalize_tech("nr5g"), "nr");
        assert_eq!(normalize_tech("both"), "both");
        assert_eq!(normalize_tech("乱写"), "both");
    }

    #[test]
    fn cnwsearch_is_located_by_magnitude() {
        // 按量纲定位：PLMN 5~6 位、频点是最大的那个数、RSRP 落在 -156..-44
        let resp = "+CNWSEARCH: 0,\"LTE\",46001,1850,301,-95,-12,20\r\nOK\r\n";
        let cells = parse_cnwsearch(resp);
        assert_eq!(cells.len(), 1);
        let c: &CellInfo = &cells[0];
        assert_eq!(c.mcc, "460");
        assert_eq!(c.mnc, "01");
        assert_eq!(c.earfcn, 1850);
        assert_eq!(c.pcid, 301);
        assert_eq!(c.rsrp, -95);
        assert_eq!(c.rsrq, -12);
        assert_eq!(c.tech, "LTE");
    }

    #[test]
    fn cnwsearch_without_tech_field() {
        // 没有制式字段时留空，也不能把 PLMN 后面的频点认成 PCI
        let cells = parse_cnwsearch("+CNWSEARCH: 1,46011,627264,702,-88,-11\r\nOK\r\n");
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].mcc, "460");
        assert_eq!(cells[0].mnc, "11");
        assert_eq!(cells[0].earfcn, 627264);
        assert_eq!(cells[0].pcid, 702);
        assert_eq!(cells[0].rsrp, -88);
        assert_eq!(cells[0].rsrq, -11);
        assert_eq!(cells[0].tech, "");
    }

    #[test]
    fn cnwsearch_skips_unparsable_lines() {
        assert!(parse_cnwsearch("+CNWSEARCH: 0\r\nOK\r\n").is_empty());
        assert!(parse_cnwsearch("ERROR").is_empty());
    }
}
