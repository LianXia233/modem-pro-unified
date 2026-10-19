//! SIM 卡与模组自身状态（`GET /api/modem/status`）。
//!
//! 汇总：ICCID / IMSI / 本机号码 / SIM 状态 / 温度 / 供电电压 / 模组时钟 / 选网方式 / 漫游。
//! 各厂商的专有指令只在「能可靠解析」时才查，取不到就省略对应字段（omitempty）。

use serde::Serialize;

use crate::error::Result;

use super::super::at::{csv, extract_field, is_ok, Prio};
use super::super::vendor::Vendor;
use super::modem::Modem;
use super::netselect::{self, NetSelect};
use super::regstate;

/// 一个温度传感器。
#[derive(Debug, Clone, Serialize)]
pub struct Sensor {
    pub name: String,
    pub celsius: i32,
}

/// 漫游判据结果。
#[derive(Debug, Clone, Serialize)]
pub struct RoamState {
    pub roaming: bool,
    #[serde(rename = "roam_scope")]
    pub scope: String,
    #[serde(rename = "home_mcc", skip_serializing_if = "Option::is_none")]
    pub home_mcc: Option<String>,
    #[serde(rename = "visit_mcc", skip_serializing_if = "Option::is_none")]
    pub visit_mcc: Option<String>,
    #[serde(rename = "roam_note", skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// SIM 卡与模组自身状态。
#[derive(Debug, Clone, Serialize)]
pub struct ModemStatus {
    pub modem_id: u32,
    pub queried_at: String,
    /// 设备序列号（IMEI）。
    ///
    /// 概览页「模组硬件参数」卡片要显示它。放这里的理由：这是**只读身份**，
    /// 一条 `AT+CGSN` 就够；而 `/api/imei` 属于「改写域」（带备份台账、Luhn
    /// 校验位、可恢复标记），不该为了刷个概览就去动那套语义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imei: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iccid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imsi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_state: Option<String>,
    pub sim_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_source: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub temps: Vec<Sensor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_max: Option<Sensor>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub temp_key: Vec<Sensor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supply_voltage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_time: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub module_unix: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub clock_skew_s: i64,
    pub net_select: NetSelect,
    pub roam: RoamState,
    pub roam_allowed: bool,
    pub roam_applies: bool,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

impl Modem {
    /// 查询一次模组状态。`allow_roaming` 来自 `[net] allow_roaming`。
    pub fn modem_status(&self, allow_roaming: bool) -> Result<ModemStatus> {
        // ICCID：合宙/泰雷兹/Telit 那一族标准写法不被认，要用 `AT+ICCID`；
        // 有方的 N510M 是 `AT$MYCCID`。候选顺序由型号特例表给（见 modem/quirk.rs）。
        let quirk_hint = self.quirk_hint();
        let iccid = self
            .query_first(&crate::modem::quirk::iccid_cmds(&quirk_hint))
            .as_deref()
            .and_then(parse_iccid);
        let imsi = self
            .query_first(&["AT+CIMI"])
            .as_deref()
            .and_then(parse_imsi);
        let (sim_state, sim_present) = self.query_cpin();
        let number = self
            .query_first(&["AT+CNUM"])
            .as_deref()
            .and_then(parse_cnum)
            .map(|n| (n, "cnum".to_string()));

        // 只读 IMEI：概览页的硬件参数卡片要显示它（写入相关语义仍归 `/api/imei`）
        let imei = self.imei();
        let operator = imsi.as_deref().and_then(operator_from_imsi);
        let temps = self.query_temps();
        let temp_max = temps.iter().max_by_key(|s| s.celsius).cloned();
        let temp_key = select_key_temps(&temps);
        // 供电电压：标准写法是 `AT+CBC`，但 Sierra/富士康 走 `AT!PCVOLT?`，
        // 且同厂只有部分型号支持 —— 按型号特例给候选，标准的仍留在最后兜底。
        let supply_voltage = self
            .query_first(&crate::modem::quirk::voltage_cmds(&quirk_hint))
            .as_deref()
            .and_then(parse_cbc);
        let (module_time, module_unix, clock_skew_s) = self.query_cclk();

        let net_select = self
            .with_at_prio(Prio::Data, |p| p.command("AT+COPS?", self.cmd_timeout))
            .map(|raw| netselect::parse(&raw))
            .unwrap_or_default();

        let ps = self
            .with_at_prio(Prio::Data, |p| p.command("AT+CGREG?", self.cmd_timeout))
            .map(|raw| regstate::parse(&raw, "+CGREG"))
            .unwrap_or_default();
        let roam = roam_state(ps.roaming, imsi.as_deref(), net_select.plmn.as_deref());
        let roam_applies = matches!(roam.scope.as_str(), "domestic" | "global");

        Ok(ModemStatus {
            modem_id: self.id,
            queried_at: crate::logging::timestamp(),
            imei,
            iccid,
            imsi,
            operator,
            sim_state,
            sim_present,
            number: number.as_ref().map(|(n, _)| n.clone()),
            number_source: number.map(|(_, src)| src),
            temps,
            temp_max,
            temp_key,
            temp_note: None,
            supply_voltage,
            module_time,
            module_unix,
            clock_skew_s,
            net_select,
            roam,
            roam_allowed: allow_roaming,
            // 「漫游策略是否适用」= 判据够不够（IMSI 与注册网 PLMN 都拿到才算）。
            // 原来这里写死 `true`，等于把「判不出来所以不拦」说成「判过且适用」，
            // 界面上的漫游提示会永远显示「已按策略处理」。
            roam_applies,
        })
    }

    /// 依次尝试若干条只读指令，返回第一条「成功」的应答。
    pub(super) fn query_first(&self, cmds: &[&str]) -> Option<String> {
        for cmd in cmds {
            let resp = self
                .with_at_prio(Prio::Data, |p| p.command(cmd, self.cmd_timeout))
                .unwrap_or_default();
            if is_ok(&resp) {
                return Some(resp);
            }
        }
        None
    }

    /// `AT+CPIN?` →（状态文本, 卡是否在）。
    fn query_cpin(&self) -> (Option<String>, bool) {
        let resp = self
            .with_at_prio(Prio::Data, |p| p.command("AT+CPIN?", self.cmd_timeout))
            .unwrap_or_default();
        match extract_field(&resp, "+CPIN") {
            Some(state) => (Some(state), true),
            None => (None, false),
        }
    }

    /// 厂商温度查询。
    fn query_temps(&self) -> Vec<Sensor> {
        // 指令表在 `modem/vendor/<厂商>.rs`（MTK 平台的广和通用带参数的写形式，
        // 读形式 `AT+GTSENRDTEMP?` 在本模组上返回 `+CME ERROR: phone failure`）。
        //
        // 型号特例优先：同一家只有个别型号的温度走别的指令（FM160 是 `AT+GTLADC`，
        // `AT+QTEMP` 回 ERROR）。特例只**插到最前**，失败仍退回厂商候选。
        let hint = self.quirk_hint();
        let mut cmds: Vec<&str> = self
            .vendor()
            .spec()
            .temperature_cmds(self.is_fibocom_mtk())
            .to_vec();
        if let Some(q) = crate::modem::quirk::temp_cmd(&hint) {
            cmds.retain(|c| *c != q);
            cmds.insert(0, q);
        }
        if cmds.is_empty() {
            return Vec::new();
        }
        let Some(resp) = self.query_first(&cmds) else {
            return Vec::new();
        };
        // 解析仍按厂商：字段布局完全不同，不能一套正则硬套
        match self.vendor() {
            Vendor::Quectel => parse_qtemp(&resp),
            Vendor::Simcom => parse_cpmutemp(&resp).into_iter().collect(),
            Vendor::Fibocom => parse_gtsenrdtemp(&resp),
            // 有指令但没解析器（或根本不该问）的厂商：宁可空，也不瞎猜
            _ => Vec::new(),
        }
    }

    /// `AT+CCLK?` →（去引号文本, unix 秒, 与主机 UTC 的偏差秒）。
    fn query_cclk(&self) -> (Option<String>, i64, i64) {
        let Some(resp) = self.query_first(&["AT+CCLK?"]) else {
            return (None, 0, 0);
        };
        let field = match extract_field(&resp, "+CCLK") {
            Some(v) => v,
            None => return (None, 0, 0),
        };
        // 界面要直接展示这个值：去掉模组应答自带的引号
        let text = field.trim().trim_matches('"').to_string();
        match parse_cclk(&text) {
            Some(unix) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                (Some(text.clone()), unix, unix - now)
            }
            None => (Some(text), 0, 0),
        }
    }
}

/// `+CCID: 8986…` 或裸 `8986…` → 纯数字 ICCID。
pub fn parse_iccid(resp: &str) -> Option<String> {
    let field = extract_field(resp, "+CCID").or_else(|| extract_field(resp, "+QCCID"));
    let raw = field.unwrap_or_else(|| resp.to_string());
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 10 {
        Some(digits)
    } else {
        None
    }
}

/// `AT+CIMI` → IMSI 数字串。
pub fn parse_imsi(resp: &str) -> Option<String> {
    for line in resp.lines() {
        let line = line.trim();
        if line.len() >= 14 && line.chars().all(|c| c.is_ascii_digit()) {
            return Some(line.to_string());
        }
    }
    None
}

/// `+CNUM: "line","+86138…",145` → 号码。
pub fn parse_cnum(resp: &str) -> Option<String> {
    let field = extract_field(resp, "+CNUM")?;
    csv(&field).into_iter().nth(1).filter(|s| !s.is_empty())
}

/// `+CBC: <bcs>,<bcl>,<mV>` → `"3.858 V"`。
pub fn parse_cbc(resp: &str) -> Option<String> {
    let field = extract_field(resp, "+CBC")?;
    let parts = csv(&field);
    // `AT+CBC` 有两种固件写法（上游怪癖表里两代机型各占一种，都实测过）：
    //
    //   三字段 `+CBC: 0,71,3861`  → <bcs>,<bcl%>,<mV>   （RM520N / EM05）
    //   两字段 `+CBC: 0,4130`     → <bcs>,<mV>         （FM160 / FM350-GL）
    //
    // 原来只认第三段，两字段的固件（本机 FM350 就是）永远解析出 None，
    // 页面「供电电压」一直显示 `-`，而实机 `AT+CBC` 明明回了 3971 mV。
    let mv = match parts.len() {
        2 => parts.get(1)?.parse::<i32>().ok()?,
        n if n >= 3 => parts.get(2)?.parse::<i32>().ok()?,
        _ => return None,
    };
    if mv <= 0 {
        return None;
    }
    Some(format!("{}.{:03} V", mv / 1000, mv % 1000))
}

/// `+QTEMP: <s0>,<s1>,…` → 传感器列表。
pub fn parse_qtemp(resp: &str) -> Vec<Sensor> {
    let Some(field) = extract_field(resp, "+QTEMP") else {
        return Vec::new();
    };
    csv(&field)
        .into_iter()
        .enumerate()
        .filter_map(|(i, v)| {
            v.parse::<i32>().ok().map(|c| Sensor {
                name: format!("sensor{i}"),
                celsius: c,
            })
        })
        .collect()
}

/// `+CPMUTEMP: <temp>` → 单个传感器。
pub fn parse_cpmutemp(resp: &str) -> Option<Sensor> {
    let field = extract_field(resp, "+CPMUTEMP")?;
    let celsius = csv(&field).first()?.parse::<i32>().ok()?;
    Some(Sensor {
        name: "cpmu".to_string(),
        celsius,
    })
}

/// 广和通 MTK 平台传感器编号 → 名称（手册 18.3 表；未列出的编号返回空串）。
pub fn gtsen_sensor_name(id: i32) -> &'static str {
    match id {
        1 => "soc_max",
        2 => "cpu_little0",
        3 => "cpu_little1",
        4 => "cpu_little2",
        7 => "gpu1",
        8 => "dramc",
        9 => "mmsys",
        10 => "md_5g",
        13 => "soc_dram_ntc",
        14 => "ltepa_ntc",
        15 => "nrpa_ntc",
        16 => "rf_ntc",
        19 => "pmic",
        20 => "pmic_vcore",
        21 => "pmic_vproc",
        22 => "pmic_vgpu",
        _ => "",
    }
}

/// 读数值保留 1 位小数。
///
/// `Sensor::celsius` 是 i32，而本指令的单位是 0.001 ℃（实测
/// `+GTSENRDTEMP: 10,46066` → 46.066 ℃），必须在此收敛掉小数噪声：
/// 直接截断会让 46.9 ℃ 显示成 46 ℃。
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// `+GTSENRDTEMP: <id>,<raw>,…` → 传感器列表。
///
/// 每行形如 `+GTSENRDTEMP: 10,46066`，单位 0.001 ℃。
/// **读数为 0 表示该路未装配**，必须跳过而不是记成 0 ℃。
pub fn parse_gtsenrdtemp(resp: &str) -> Vec<Sensor> {
    let mut out = Vec::new();
    for line in resp.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("+GTSENRDTEMP") else {
            continue;
        };
        let rest = rest.trim_start_matches(':').trim();
        // 手册允许一次返回多对 `<id>,<raw>`，按逗号成对消费
        let mut it = rest.split(',');
        while let (Some(id), Some(raw)) = (it.next(), it.next()) {
            let (Ok(id), Ok(raw)) = (id.trim().parse::<i32>(), raw.trim().parse::<i32>()) else {
                continue;
            };
            if raw == 0 || id <= 0 {
                continue;
            }
            let celsius = round1(raw as f64 / 1000.0) as i32;
            let name = gtsen_sensor_name(id);
            out.push(Sensor {
                name: if name.is_empty() {
                    format!("sensor{id}")
                } else {
                    name.to_string()
                },
                celsius,
            });
        }
    }
    out
}

/// `AT+CCLK?` 的 `"26/09/29,04:31:55+32"` → unix 秒（按 UTC 解释，忽略时区尾巴）。
pub fn parse_cclk(field: &str) -> Option<i64> {
    let field = field.trim().trim_matches('"');
    let (date, rest) = field.split_once(',')?;
    let time: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let mut d = date.split('/');
    let yy: i64 = d.next()?.parse().ok()?;
    let mm: i64 = d.next()?.parse().ok()?;
    let dd: i64 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hh: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let ss: i64 = t.next().unwrap_or("0").parse().ok()?;
    let year = 2000 + yy;
    let days = days_from_civil(year, mm as u32, dd as u32);
    Some(days * 86_400 + hh * 3600 + mi * 60 + ss)
}

/// Howard Hinnant 的 days_from_civil（纯整数，无依赖）。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// MCC+MNC → 中国运营商名（认不出返回 `None`）。
///
/// ## 这是全仓库**唯一**的一张运营商表
///
/// 早先 `cells.rs::operator_name` 里还有第二张，两表口径冲突：
/// `46004` / `46008` 在 `cells.rs` 被判成「中国广电」，在这里被判成「中国移动」——
/// 同一张卡，`/api/cells` 和 `/api/modem/status` 会显示成两家运营商。
///
/// 以本表为准（46004 / 46008 是中国移动的 4G / 物联网号段；广电是 46015）。
/// `cells.rs::operator_name` 现在只是它的一个转發壳，页面口径因此统一。
pub fn operator_from_plmn(mcc: &str, mnc: &str) -> Option<String> {
    let name = match format!("{mcc}{mnc}").as_str() {
        "46000" | "46002" | "46004" | "46007" | "46008" => "中国移动",
        "46001" | "46006" | "46009" => "中国联通",
        "46003" | "46005" | "46011" => "中国电信",
        "46015" => "中国广电",
        _ => return None,
    };
    Some(name.to_string())
}

/// 由 IMSI 前 5 位推出运营商名（与网络侧 PLMN 互相印证）。
pub fn operator_from_imsi(imsi: &str) -> Option<String> {
    if imsi.len() < 5 {
        return None;
    }
    operator_from_plmn(&imsi[..3], &imsi[3..5])
}

/// 挑出机身 / 5G 基带 / CPU 三个关键温度（能识别几个算几个）。
///
/// 早先这里直接 `temps.iter().take(3)` —— 取的是列表**前三项**，跟注释里写的
/// 「按名称挑机身 / 5G 基带 / CPU」完全不是一回事。不同固件的上报顺序不一样
/// （广和通按 sensor id 升序、移远按 slot 序），前三项经常是无关的通路。
/// 现在按名字匹配；一个都对不上时才退回前三项，保证界面不至于全空。
fn select_key_temps(temps: &[Sensor]) -> Vec<Sensor> {
    const WANTED: [&[&str]; 3] = [
        &["机身", "board", "case", "shell", "pa", "tsens"],
        &["5g", "nr", "md_5g", "modem", "基带"],
        &["cpu", "ap", "soc", "little", "big"],
    ];
    let mut picked: Vec<Sensor> = Vec::new();
    for keys in WANTED {
        let hit = temps.iter().find(|s| {
            let n = s.name.to_ascii_lowercase();
            keys.iter().any(|k| n.contains(k))
        });
        if let Some(s) = hit {
            // 同一个传感器不会被两类同时挑中（分类互斥），这里只防重复
            if !picked.iter().any(|p| p.name == s.name) {
                picked.push(s.clone());
            }
        }
    }
    if picked.is_empty() {
        return temps.iter().take(3).cloned().collect();
    }
    picked
}

/// 漫游判据：数据域漫游 + 卡归属 MCC + 注册网 MCC。
pub fn roam_state(roaming: bool, imsi: Option<&str>, visit_plmn: Option<&str>) -> RoamState {
    if !roaming {
        return RoamState {
            roaming: false,
            scope: "none".to_string(),
            home_mcc: None,
            visit_mcc: None,
            note: None,
        };
    }
    let home = imsi.filter(|s| s.len() >= 3).map(|s| s[..3].to_string());
    let visit = visit_plmn
        .filter(|s| s.len() >= 3)
        .map(|s| s[..3].to_string());
    let (scope, note) = match (&home, &visit) {
        (None, _) => (
            "unknown",
            Some(
                "读不到卡的归属（IMSI），判不出是国际漫游还是同国异网漫游 —— 所以不拦".to_string(),
            ),
        ),
        (_, None) => (
            "unknown",
            Some("读不到当前注册网的 PLMN（MCC），判不出国别 —— 所以不拦".to_string()),
        ),
        (Some(h), Some(v)) if h == v => ("domestic", None),
        _ => ("global", None),
    };
    RoamState {
        roaming: true,
        scope: scope.to_string(),
        home_mcc: home,
        visit_mcc: visit,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::{operator_from_plmn, select_key_temps, Sensor};

    #[test]
    fn operator_table_is_single_source_of_truth() {
        // 46004 / 46008 是中国移动的 4G / 物联网号段，广电是 46015。
        // 这里同时钉住 `cells.rs::operator_name`（它是本表的转发壳），
        // 保证两个页面不会再显示成两家运营商。
        assert_eq!(operator_from_plmn("460", "04").as_deref(), Some("中国移动"));
        assert_eq!(operator_from_plmn("460", "08").as_deref(), Some("中国移动"));
        assert_eq!(operator_from_plmn("460", "15").as_deref(), Some("中国广电"));
        assert_eq!(
            crate::modem::model::cells::operator_name("460", "04"),
            "中国移动"
        );
        assert_eq!(operator_from_plmn("999", "99"), None);
    }

    #[test]
    fn imsi_and_plmn_agree() {
        // IMSI 前 5 位与 PLMN 走同一张表
        assert_eq!(
            super::operator_from_imsi("460041234567890").as_deref(),
            Some("中国移动")
        );
        assert_eq!(
            super::operator_from_imsi("460151234567890").as_deref(),
            Some("中国广电")
        );
        assert_eq!(
            super::operator_from_imsi("4601"),
            None,
            "太短的 IMSI 不该瞎猜"
        );
    }

    #[test]
    fn key_temps_are_picked_by_name_not_position() {
        let temps = vec![
            Sensor {
                name: "sensor0".into(),
                celsius: 31,
            },
            Sensor {
                name: "md_5g".into(),
                celsius: 52,
            },
            Sensor {
                name: "cpu_little".into(),
                celsius: 44,
            },
            Sensor {
                name: "board".into(),
                celsius: 38,
            },
        ];
        let key = select_key_temps(&temps);
        let names: Vec<&str> = key.iter().map(|s| s.name.as_str()).collect();
        // 旧的 take(3) 会挑出 sensor0/md_5g/cpu_little —— 前两项纯属占位
        assert!(names.contains(&"board"), "机身温度应按名字挑到：{names:?}");
        assert!(names.contains(&"md_5g"), "5G 基带应按名字挑到：{names:?}");
        assert!(names.contains(&"cpu_little"), "CPU 应按名字挑到：{names:?}");
        assert!(
            !names.contains(&"sensor0"),
            "无关通路不该进关键温度：{names:?}"
        );
    }

    #[test]
    fn key_temps_fall_back_to_first_three() {
        // 名字一个都对不上时退回前三项，界面不至于全空
        let temps = vec![
            Sensor {
                name: "a".into(),
                celsius: 1,
            },
            Sensor {
                name: "b".into(),
                celsius: 2,
            },
            Sensor {
                name: "c".into(),
                celsius: 3,
            },
            Sensor {
                name: "d".into(),
                celsius: 4,
            },
        ];
        assert_eq!(select_key_temps(&temps).len(), 3);
    }

    use super::{
        gtsen_sensor_name, operator_from_imsi, parse_cbc, parse_cclk, parse_cnum,
        parse_gtsenrdtemp, parse_iccid, parse_imsi, parse_qtemp, roam_state, round1,
    };

    #[test]
    fn iccid_forms() {
        assert_eq!(
            parse_iccid("+CCID: 89860404102390000000\r\nOK").as_deref(),
            Some("89860404102390000000")
        );
        assert_eq!(
            parse_iccid("89860012345678901234\r\nOK").as_deref(),
            Some("89860012345678901234")
        );
        assert_eq!(parse_iccid("ERROR"), None);
    }

    #[test]
    fn imsi_and_operator() {
        let imsi = parse_imsi("460110123456789\r\nOK").unwrap();
        assert_eq!(imsi, "460110123456789");
        assert_eq!(operator_from_imsi(&imsi).as_deref(), Some("中国电信"));
        assert_eq!(operator_from_imsi("46000"), Some("中国移动".to_string()));
    }

    #[test]
    fn cnum_and_cbc() {
        assert_eq!(
            parse_cnum("+CNUM: \"\",\"+8613800138000\",145\r\nOK").as_deref(),
            Some("+8613800138000")
        );
        assert_eq!(
            parse_cbc("+CBC: 0,100,3858\r\nOK").as_deref(),
            Some("3.858 V")
        );
        assert_eq!(parse_cbc("+CBC: 0,0,0\r\nOK"), None);
        // 两字段写法（FM160 / FM350-GL 实测）：第二个就是 mV，没有电量百分比
        assert_eq!(
            parse_cbc("+CBC: 0,3971\r\nOK").as_deref(),
            Some("3.971 V"),
            "两字段固件必须能解析，否则电压恒为 -"
        );
        assert_eq!(parse_cbc("+CBC: 0,4130\r\nOK").as_deref(), Some("4.130 V"));
        // 只有一段 / 空值：不猜
        assert_eq!(parse_cbc("+CBC: 0\r\nOK"), None);
        assert_eq!(parse_cbc("+CBC: 0,0\r\nOK"), None);
        assert_eq!(parse_cbc("ERROR"), None);
    }

    #[test]
    fn qtemp_list() {
        let t = parse_qtemp("+QTEMP: 43,41,41,0,0\r\nOK");
        assert_eq!(t.len(), 5);
        assert_eq!(t[0].celsius, 43);
    }

    #[test]
    fn gtsenrdtemp_parses_mtk_sensors() {
        // 一次返回全部通路；单位 0.001 ℃
        let r = "+GTSENRDTEMP: 1,47200\r\n+GTSENRDTEMP: 10,45485\r\n+GTSENRDTEMP: 14,51000\r\n\r\nOK\r\n";
        let t = parse_gtsenrdtemp(r);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].name, "soc_max");
        assert_eq!(t[0].celsius, 47);
        assert_eq!(t[1].name, "md_5g");
        assert_eq!(t[1].celsius, 45);
        assert_eq!(t[2].name, "ltepa_ntc");
        assert_eq!(t[2].celsius, 51);
    }

    #[test]
    fn gtsenrdtemp_skips_unpopulated_and_bad_lines() {
        // 0 = 该路未装配，不得记成 0 ℃；非本指令前缀直接忽略
        let r = "+GTSENRDTEMP: 10,0\r\n+GTSENRDTEMP: 1,47200\r\n+QTEMP: 40\r\n\r\nOK\r\n";
        let t = parse_gtsenrdtemp(r);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].celsius, 47);
        // 空响应 / 纯 ERROR 都不产出传感器
        assert!(parse_gtsenrdtemp("\r\nERROR\r\n").is_empty());
        assert!(parse_gtsenrdtemp("").is_empty());
    }

    #[test]
    fn gtsenrdtemp_handles_multi_pair_and_unnamed() {
        // 手册允许一行多对；未列出的编号退化成 sensorN
        let r = "+GTSENRDTEMP: 10,46066,99,30000\r\nOK\r\n";
        let t = parse_gtsenrdtemp(r);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].name, "md_5g");
        assert_eq!(t[0].celsius, 46); // 46.066 收敛为 46
        assert_eq!(t[1].name, "sensor99");
        // 46.9 不得被截断成 46
        assert_eq!(round1(46.966), 47.0);
    }

    #[test]
    fn gtsen_sensor_names() {
        assert_eq!(gtsen_sensor_name(10), "md_5g");
        assert_eq!(gtsen_sensor_name(1), "soc_max");
        assert_eq!(gtsen_sensor_name(15), "nrpa_ntc");
        assert_eq!(gtsen_sensor_name(5), "");
        assert_eq!(gtsen_sensor_name(0), "");
        assert_eq!(gtsen_sensor_name(999), "");
    }

    #[test]
    fn cclk_to_unix() {
        // 2000-01-01 00:00:00 UTC
        assert_eq!(parse_cclk("\"00/01/01,00:00:00+00\""), Some(946_684_800));
        // +1 天 = +86400
        assert_eq!(
            parse_cclk("00/01/02,00:00:00+00"),
            Some(946_684_800 + 86_400)
        );
        // 时分秒
        assert_eq!(parse_cclk("00/01/01,01:02:03+00"), Some(946_684_800 + 3723));
    }

    #[test]
    fn roam_scopes() {
        let s = roam_state(true, Some("460110123456789"), Some("46000"));
        assert_eq!(s.scope, "domestic");
        let g = roam_state(true, Some("460110123456789"), Some("26201"));
        assert_eq!(g.scope, "global");
        let n = roam_state(false, Some("46011"), None);
        assert_eq!(n.scope, "none");
        assert!(!n.roaming);
    }
}
