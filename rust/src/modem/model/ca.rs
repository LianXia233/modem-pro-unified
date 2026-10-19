//! 载波聚合（Carrier Aggregation）查询与解析。
//!
//! ## 为什么单独挂一个模块
//!
//! CA 的应答格式**每家都不一样**，而且都不短：
//!
//! | 厂商 | 指令 | 形态 |
//! | --- | --- | --- |
//! | 广和通（MTK 平台） | `AT+GTCAINFO?` | `PCC:`/`SCC1:`/`SCC2:` 前缀 + 裸 CSV，分 LTE / NR 两段 |
//! | 移远 / 美格（高通） | `AT+QCAINFO` | `+QCAINFO: "PCC",…` 带引号的字段 |
//! | Telit | `AT#CAINFOEXT?` | `#CAINFOEXT:` 前缀（字段布局未移植） |
//!
//! 塞进 `network.rs` 或 `cells.rs` 都会把「网络总览」「基站扫描」这两个已经很长的
//! 文件再撑大，所以集中在这里；对外只暴露 [`Modem::ca_status`] 一个入口。
//!
//! ## 依据
//!
//! 广和通那一段**逐字段照抄官方手册《FIBOCOM FM350 AT Commands v2.2》§11.1.16
//! （+GTCAINFO, Display of CA information）**，不是猜的：
//!
//! - LTE：`PCC: <band>,<physical cellId>,<earfcn>,<dl_bandwidth>,<dl_mimo>,<ul_mimo>,
//!   <dl_modulation>,<ul_modulation>`
//! - NR：`PCC: <band>,<physical cellId>,<narfcn>,<nr_dl_bandwidth>,…`
//! - SCC 多三个前导字段：`SCCn:<scell_state>,<ul_configured>,<band>,…`
//! - `<band>` 是**编码值**：LTE = 100 + 频段号（101→B1，171→B71）；
//!   NR = 500 + 频段号（501→n1，509→n9，5010→n10，50512→n512）
//! - `<scell_state>`：1 = 已配置未激活，2 = 已配置**已激活**
//! - LTE 带宽：6/15/25/50/75/100 → 1.4/3/5/10/15/20 MHz
//! - NR 带宽：25/50/75/100/125/150/200/250/300/400/450/500/1000/2000 → 5…400 MHz
//!   （数值 = MHz × 5）
//!
//! 移远 `+QCAINFO` 的格式本仓库没有官方手册可依，因此只做**保守解析**：
//! 认 `PCC`/`SCC` 角色与 `"LTE BAND <n>"` / `"NR5G BAND n<n>"` 这类文本，
//! 取不到就留空 —— **不猜列序、不编造数值**。解析不出来的厂商返回 `None`，
//! 由调用方如实显示「未上报」，而不是拿空数组当「没有 CA」。

use super::network::CcInfo;

/// 一次 CA 查询的结果。
#[derive(Debug, Clone, Default)]
pub struct CaInfo {
    /// 分量载波（第一个通常是 PCC；顺序保留模组上报的原序）。
    pub ccs: Vec<CcInfo>,
    /// 是否存在**已激活**的辅载波（SCC）。
    ///
    /// 注意与「有没有 SCC 行」区分：广和通的 `scell_state=1` 表示「配了但没激活」，
    /// 那时不算真的在做载波聚合。
    pub ca_active: bool,
    /// 原始应答（去空行后用 `|` 连成一行）。解析不出任何分量载波时它还在，
    /// 界面和日志能看见模组到底回了什么 —— 这是排障的第一手材料。
    pub raw: String,
    /// 解析时用的指令（`AT+GTCAINFO?` / `AT+QCAINFO`…），日志里带上便于对账。
    pub cmd: String,
}

impl CaInfo {
    /// 没有任何分量载波（模组回了空、或指令不被支持）时的说明。
    pub fn empty_note(&self) -> &'static str {
        if self.raw.is_empty() {
            "模组对本机的 CA 查询指令没有应答（可能不支持，或当前未进入连接态）"
        } else {
            "模组应答里解析不出分量载波：空闲态下通常只上报主载波，进入连接态并\n由网络配置辅载波后才会出现 SCC"
        }
    }
}

// ---------------------------------------------------------------- 广和通

/// 解析 `AT+GTCAINFO?`（广和通 MTK 平台，格式见模块级注释里手册 §11.1.16）。
///
/// 应答示例（手册原文，LTE）：
/// ```text
/// +GTCAINFO:
/// PCC: 103,120,1850,100,2,1,4,1
/// SCC1:2,0,101,355,100,100,2,1,4,1,120,-95,-10
/// OK
/// ```
pub fn parse_gtccinfo(resp: &str) -> Option<CaInfo> {
    let mut ccs = Vec::new();
    let mut ca_active = false;

    for line in resp.lines() {
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("OK") {
            continue;
        }
        // 只认 `PCC:` / `SCC1:` / `SCC2:`… 这两种前缀，其余（含 `+GTCAINFO:` 本身、ERROR）跳过
        let Some((role_key, payload)) = split_ca_prefix(line) else {
            continue;
        };

        let fields: Vec<i32> = payload
            .split(',')
            .map(|s| s.trim().parse::<i32>().ok())
            .collect::<Option<Vec<_>>>()?;

        // SCC 比 PCC 多两个前导字段：scell_state、ul_configured
        let is_scc = role_key.starts_with("SCC");
        let idx = if is_scc { 2 } else { 0 };
        if fields.len() < idx + 4 {
            continue;
        }
        let band_code = fields[idx];
        let _pci = fields[idx + 1];
        let arfcn = fields[idx + 2];
        let bw_code = fields[idx + 3];

        // 频段编码：LTE 是 101~171（→ B1~B71），NR 是 501+（→ n1~n512）
        let (band, is_nr) = decode_band(band_code)?;
        let cc = CcInfo {
            role: if is_scc {
                // `SCC1` → `SCC1`：保留编号，界面上能看出是第几个辅载波
                format!("SCC{}", &role_key[3..])
            } else {
                "PCC".to_string()
            },
            band,
            band_label: if is_nr {
                format!("n{band}")
            } else {
                format!("B{band}")
            },
            bandwidth: decode_bandwidth(bw_code, is_nr),
            earfcn: arfcn,
        };
        ccs.push(cc);

        // 只有「已激活」的 SCC 才算真的在做 CA（scell_state == 2）
        if is_scc && fields.first() == Some(&2) {
            ca_active = true;
        }
    }

    if ccs.is_empty() {
        return None;
    }
    Some(CaInfo {
        ccs,
        ca_active,
        raw: join(resp),
        cmd: "AT+GTCAINFO?".to_string(),
    })
}

// ---------------------------------------------------------------- 移远 / 美格

/// 解析 `AT+QCAINFO`（移远 / 美格，高通平台）。
///
/// ⚠️ 本仓库没有这两家的 CA 官方手册，因此这里**只认文本模式**：
/// 角色的 `"PCC"`/`"SCC"`、频段名的 `"LTE BAND <n>"` / `"NR5G BAND n<n>"`，
/// 以及第一个落在合理量程内的整数当频点。列序一概不猜 —— 解析不出来就留空。
pub fn parse_qcainfo(resp: &str) -> Option<CaInfo> {
    let mut ccs = Vec::new();
    let mut ca_active = false;

    for line in resp.lines() {
        let line = line.trim();
        let Some(rest) = line.find("+QCAINFO:").map(|i| &line[i + 9..]) else {
            continue;
        };
        let rest = rest.trim();
        let up = rest.to_ascii_uppercase();
        let is_scc = up.contains("\"SCC\"") || up.contains("SCC,") || up.contains("SCC\"");
        let is_pcc = up.contains("\"PCC\"") || up.contains("PCC,") || up.contains("PCC\"");
        if !is_scc && !is_pcc {
            continue;
        }

        // 频段名：`"LTE BAND 3"` / `"NR5G BAND n78"` / `"NR BAND n78"`
        let (band, is_nr) = band_from_text(&up)?;
        // 频点：取第一个落在 LTE EARFCN(0~65535) 或 NR ARFCN(0~3279165) 量程里的整数。
        // 注意要跳过紧跟在 band 名后面的那个数字（它就是 band 号）。
        let earfcn = first_arfcn(rest);

        ccs.push(CcInfo {
            role: if is_scc {
                "SCC".to_string()
            } else {
                "PCC".to_string()
            },
            band,
            band_label: if is_nr {
                format!("n{band}")
            } else {
                format!("B{band}")
            },
            // 高通的带宽列序没有手册可依，不猜：留 0 由界面按「未上报」渲染
            bandwidth: 0,
            earfcn: earfcn.unwrap_or(0),
        });
        if is_scc {
            ca_active = true;
        }
    }

    if ccs.is_empty() {
        return None;
    }
    Some(CaInfo {
        ccs,
        ca_active,
        raw: join(resp),
        cmd: "AT+QCAINFO".to_string(),
    })
}

// ---------------------------------------------------------------- 分派

/// 按指令名分派到对应解析器；没有对应实现时返回 `None`（调用方如实显示未上报）。
pub fn parse_ca(cmd: &str, resp: &str) -> Option<CaInfo> {
    let up = cmd.to_ascii_uppercase();
    if up.contains("GTCAINFO") {
        parse_gtccinfo(resp)
    } else if up.contains("QCAINFO") {
        parse_qcainfo(resp)
    } else {
        // Telit 的 `#CAINFOEXT` 字段布局未移植（混合主/辅载波），
        // 这里明确返回 None —— 好过按错误的列序编出一份假数据。
        None
    }
}

// ---------------------------------------------------------------- 查询入口

impl super::Modem {
    /// 查询一次载波聚合状态。
    ///
    /// 指令从厂商声明里挑第一条含 `CAINFO` 的（见 `Modem::ca_cmds`），
    /// 所以换机型不会用错方言。返回 `None` 的三种情况要区分对待 ——
    /// 都代表「拿不到 CA」，但原因完全不同：
    ///
    /// - 本机没有声明 CA 指令（厂商不支持）；
    /// - 串口打不开 / 指令回 ERROR（模组忙或未进入连接态）；
    /// - 应答解析不出分量载波（格式没移植，如 Telit）。
    ///
    /// 调用方应当原样把这三类都显示成「未上报」，**不要**拿空数组冒充「没有 CA」。
    pub fn ca_status(&self) -> Option<CaInfo> {
        let cmd = self.ca_cmds().into_iter().next()?;
        let resp = self
            .with_at(|p| p.command(&cmd, self.cmd_timeout))
            .unwrap_or_default();
        if resp.trim().is_empty() {
            log::debug!("模组 id={} 的 {} 无应答", self.id, cmd);
            return None;
        }
        let mut info = parse_ca(&cmd, &resp)?;
        info.cmd = cmd;
        Some(info)
    }
}

// ---------------------------------------------------------------- 内部工具

/// 拆出 `PCC:` / `SCC1:` 这样的前缀。返回 `(键, 冒号后的内容)`。
fn split_ca_prefix(line: &str) -> Option<(String, String)> {
    let (key, rest) = line.split_once(':')?;
    let key = key.trim().to_ascii_uppercase();
    if key == "PCC" {
        return Some((key, rest.trim().to_string()));
    }
    if let Some(n) = key.strip_prefix("SCC") {
        // SCC 后面必须是数字（SCC1/SCC2…），单独的 "SCC" 不带编号不认
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return Some((key, rest.trim().to_string()));
        }
    }
    None
}

/// 频段编码 →（频段号, 是否 NR）。取值口径见模块级注释（手册 §11.1.16）。
///
/// ⚠️ NR 那一侧是**十进制拼接**，不是减法：手册给的是
/// `501`→n1、`509`→n9、`5010`→n10、`50512`→n512，即「`50` 后面直接接频段号」。
/// 早期版本按 `code - 5000` 算，在 50512 上会得出 45512 —— 差了一个数量级，
/// 是单测抓出来的（见 `gtccinfo_nr_band_encoding`）。
fn decode_band(code: i32) -> Option<(i32, bool)> {
    if (101..=171).contains(&code) {
        return Some((code - 100, false));
    }
    let rest = code.to_string().strip_prefix("50")?.to_string();
    if rest.is_empty() {
        return None;
    }
    let n: i32 = rest.parse().ok()?;
    // 3GPP 目前定义的 NR 频段号上界是 n1023，留点余量做合理性校验
    if (1..=1023).contains(&n) {
        Some((n, true))
    } else {
        None
    }
}

/// 带宽编码 → MHz（手册 §11.1.16 的 Defined Values）。
///
/// LTE 用 6/15/25/50/75/100 表示 1.4/3/5/10/15/20 MHz；
/// NR 直接是 MHz × 5（25 → 5 MHz，2000 → 400 MHz）。
fn decode_bandwidth(code: i32, is_nr: bool) -> i32 {
    if is_nr {
        return (code / 5).max(0);
    }
    match code {
        6 => 1,
        15 => 3,
        25 => 5,
        50 => 10,
        75 => 15,
        100 => 20,
        other if other > 0 => other / 5,
        _ => 0,
    }
}

/// 从 `"LTE BAND 3"` / `"NR5G BAND n78"` 这类文本里取频段号。
fn band_from_text(up: &str) -> Option<(i32, bool)> {
    if let Some(p) = up.find("BAND") {
        let tail = up[p + 4..].trim_start();
        let is_nr = up[..p].contains("NR");
        let digits: String = tail
            .trim_start_matches(['n', 'N'])
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.parse::<i32>() {
            if n > 0 {
                return Some((n, is_nr));
            }
        }
    }
    None
}

/// 取第一个落在合理频点量程内的整数。
///
/// 量程即手册给的范围：LTE EARFCN 0~65535、NR ARFCN 0~3279165。
/// 用「量程」而不是「列序」挑，是因为本仓库没有 `+QCAINFO` 的官方手册，
/// 硬按第 N 列取值等于赌 —— 宁可在取不到时留 0。
fn first_arfcn(s: &str) -> Option<i32> {
    let mut digits = String::new();
    let mut found: Option<i32> = None;
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if !digits.is_empty() {
            if let Ok(v) = digits.parse::<i32>() {
                if (10..=3_279_165).contains(&v) {
                    found = Some(v);
                    break;
                }
            }
            digits.clear();
        }
    }
    if found.is_none() && !digits.is_empty() {
        if let Ok(v) = digits.parse::<i32>() {
            if (10..=3_279_165).contains(&v) {
                found = Some(v);
            }
        }
    }
    found
}

/// 原始应答按行（去空行）用 `|` 连成一行。
fn join(resp: &str) -> String {
    super::modem::split_at_lines(resp).join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtccinfo_lte_pcc_and_activated_scc() {
        // 手册 §11.1.16 的字段顺序（SCC 前导 scell_state/ul_configured）
        let resp = "+GTCAINFO:\r\nPCC: 103,120,1850,100,2,1,4,1\r\nSCC1:2,0,101,355,100,100,2,1,4,1,120,-95,-10\r\nOK\r\n";
        let ca = parse_gtccinfo(resp).expect("应解析出两个分量载波");
        assert_eq!(ca.ccs.len(), 2);
        assert_eq!(ca.ccs[0].role, "PCC");
        assert_eq!(ca.ccs[0].band, 3, "103 → B3");
        assert_eq!(ca.ccs[0].band_label, "B3");
        assert_eq!(ca.ccs[0].earfcn, 1850);
        assert_eq!(ca.ccs[0].bandwidth, 20, "100 → 20 MHz");
        assert_eq!(ca.ccs[1].role, "SCC1");
        assert_eq!(ca.ccs[1].band, 1, "101 → B1");
        // scell_state=2 ⇒ 已激活
        assert!(ca.ca_active);
    }

    #[test]
    fn gtccinfo_deactivated_scc_is_not_active() {
        // scell_state=1：配了但没激活，不能算在做 CA
        let resp = "PCC: 103,120,1850,100,2,1,4,1\r\nSCC1:1,0,101,355,100,100,2,1,4,1\r\nOK";
        let ca = parse_gtccinfo(resp).unwrap();
        assert_eq!(ca.ccs.len(), 2);
        assert!(!ca.ca_active, "未激活的 SCC 不该点亮 CA");
    }

    #[test]
    fn gtccinfo_nr_band_encoding() {
        // 507 → n7；NR 带宽 100 → 20 MHz
        let resp = "PCC: 507,333,627264,100,4,2,6,2\r\nOK";
        let ca = parse_gtccinfo(resp).unwrap();
        assert_eq!(ca.ccs[0].band, 7);
        assert_eq!(ca.ccs[0].band_label, "n7");
        assert_eq!(ca.ccs[0].earfcn, 627264);
        assert_eq!(ca.ccs[0].bandwidth, 20);
        // 5010 → n10
        assert_eq!(decode_band(5010), Some((10, true)));
        assert_eq!(decode_band(50512), Some((512, true)));
    }

    #[test]
    fn gtccinfo_only_pcc_means_no_ca() {
        let resp = "+GTCAINFO:\r\nPCC: 103,120,1850,100,2,1,4,1\r\nOK";
        let ca = parse_gtccinfo(resp).unwrap();
        assert_eq!(ca.ccs.len(), 1);
        assert!(!ca.ca_active, "只有主载波就是没聚合");
    }

    #[test]
    fn gtccinfo_garbage_returns_none() {
        assert!(parse_gtccinfo("ERROR").is_none());
        assert!(parse_gtccinfo("").is_none());
        // 非数字字段：整行解析不了，不该编出半条记录
        assert!(parse_gtccinfo("PCC: a,b,c,d").is_none());
    }

    #[test]
    fn qcainfo_reads_role_and_band_text() {
        let resp = "+QCAINFO: \"PCC\",1850,100,\"LTE BAND 3\",1\r\n+QCAINFO: \"SCC\",100,100,\"LTE BAND 1\",1\r\nOK";
        let ca = parse_qcainfo(resp).unwrap();
        assert_eq!(ca.ccs.len(), 2);
        assert_eq!(ca.ccs[0].role, "PCC");
        assert_eq!(ca.ccs[0].band, 3);
        assert_eq!(ca.ccs[1].role, "SCC");
        assert!(ca.ca_active);
    }

    #[test]
    fn unknown_ca_command_is_not_guessed() {
        // Telit 的 #CAINFOEXT 没移植：宁可返回 None，也不按错的列序编数据
        assert!(parse_ca("AT#CAINFOEXT?", "whatever").is_none());
        assert!(parse_ca("AT+GTCAINFO?", "PCC: 103,120,1850,100,2,1,4,1").is_some());
    }
}
