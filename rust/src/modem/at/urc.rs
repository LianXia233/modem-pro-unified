//! AT 主动上报（URC）的识别与过滤。
//!
//! ## 为什么需要这一层
//!
//! 串口 `read` 不保证按行返回，URC 也可能被切在任意字节边界；而 URC 行一旦
//! 混进命令应答，解析层就会把「主动上报」当成「查询结果」。实测症状：注册态
//! URC `+CEREG: 5` 抢先于查询应答 `+CEREG: 1,1` 被取到，`GET /api/network`
//! 的注册态偶发误报成漫游。
//!
//! ## 安全边界（改这里的代价很高，务必看清）
//!
//! 只吞**明确是主动上报**的行，判据分两类：
//!
//! 1. **私有 URC 前缀表**：前缀在 [`PRIVATE_URC_PREFIXES`] 里，且本程序从不
//!    查询该前缀。查过的前缀一律不吞 —— 参考实现用 `+GT` 宽匹配，结果把
//!    `AT+GTDNS` 的**查询响应**一起吞掉，巡检的 DNS 解析恒为空。这里逐条
//!    列前缀就是为了不重蹈。
//! 2. **注册态 URC 靠字段数区分**：主动上报是单字段 `+CEREG: 5`，
//!    查询应答是双字段 `+CEREG: 1,1`（3GPP TS 27.007 §10.1.19）。
//!
//! `+CPIN` **刻意不吞**：它的主动上报与 `AT+CPIN?` 的应答格式完全相同
//! （都是 `+CPIN: READY`），吞了会把「SIM 卡状态查询」变成永远空应答。
//! 同理，在线与否只能由注册态判据决定，绝不能拿 `+CPIN` URC 推断 ——
//! 实测 `+CREG=1` 的 URC 会被误读成「已注册在线」。
//!
//! ## 洪水阈值为什么只作用于 URC 行
//!
//! [`FLOOD_LINES_PER_SEC`] 只对**被判定为 URC 的行**计数，超限即丢弃，
//! 绝不让普通应答行参与计数。`AT+COPS=?` 扫网能一次回上百行运营商列表，
//! 拿它一起限流会把查询结果截断 —— 那比 URC 污染严重得多。
//!
//! ## 本模块的边界
//!
//! 只做**过滤与暂存**，不解释事件语义：每条被吞的 URC 原样进
//! [`LineGate::take_urc`] 队列，解析留给消费端（`model::urc` 折算脏标记 +
//! `http::loops::spawn_urc` 驱动巡检）。队列容量
//! [`URC_QUEUE_CAP`]，满员丢最旧 —— 堆积本身就说明没人消费。

use std::time::{Duration, Instant};

/// 单秒 URC 行数上限；超过就丢弃同秒后续 URC（防串口垃圾淹没）。
const FLOOD_LINES_PER_SEC: u32 = 60;

/// URC 暂存队列容量（满员丢最旧）。
const URC_QUEUE_CAP: usize = 64;

/// 私有 URC 前缀表：`(前缀, 类别)`。
///
/// **加前缀前先确认本程序不查询它**：`grep` 一遍 `at_cmd` / preset 里有没有
/// 同名查询，有就不能进这张表。
///
/// 前缀统一**不带前导 `+`**，匹配时先剥掉行首的一个 `+`，
/// 这样 `+GTSTATUS` 与个别固件不带 `+` 的写法能共用一条。
///
/// `+CUSD` **不在这张表里**：它的查询应答 `AT+CUSD=?` → `+CUSD: (0-2)`
/// 与主动上报 `+CUSD: 0,"text",15` 同前缀，进表就会把 USSD 支持情况的查询
/// 应答一起吞掉（正是 [`PRIVATE_URC_PREFIXES`] 开头警告的那类事故）。
/// 它走 [`cusd_is_urc`] 的形态判据。
const PRIVATE_URC_PREFIXES: &[(&str, UrcKind)] = &[
    // 广和通 MTK 平台（FM350 / RW350 系列）原生 URC
    ("GTSTATUS", UrcKind::GtStatus),
    ("GTRGC", UrcKind::GtRgc),
    ("GTUSBMODE", UrcKind::GtUsbmode),
    ("GTSIM", UrcKind::GtSim),
    ("GTSRV", UrcKind::GtSrv),
    ("GTPIN", UrcKind::GtPin),
    ("GTDSS", UrcKind::GtDss),
    // 小区信息变化（部分固件不带前导 +）
    ("GTCELLINFO", UrcKind::GtCellInfo),
    // 固件版本上报
    ("VER", UrcKind::Ver),
    // 新短信到达（`+CMTI: "SM",3`）。本程序只发 `AT+CMGL` / `AT+CPMS` /
    // `AT+CMGF`，没有任何一条的应答会长这样，进表安全。
    ("CMTI", UrcKind::SmsNew),
];

/// USSD 应答（`+CUSD`）的形态判据：`rest` 是前缀之后的部分（含冒号）。
///
/// `AT+CUSD=?` 的应答是**取值范围**形式（3GPP TS 27.007 §7.6）：
///
/// ```text
/// AT+CUSD=?      → +CUSD: (0-2)
/// AT+CUSD=1,*100#→ +CUSD: 0,"余额 12.5 元",15     ← 主动上报
/// ```
///
/// 主动上报的第一字段永远是数字状态码，所以「冒号之后是不是 `(`」就够区分。
/// 放行查询应答还有一个兜底：`parse_cusd_line` 对 `(0-2)` 解析状态码会失败，
/// 真误入了也变不成一条假应答。
fn cusd_is_urc(rest: &str) -> bool {
    let after_colon = rest.trim_start_matches(':').trim_start();
    !after_colon.starts_with('(')
}

/// 注册态 URC 前缀（单字段才是 URC，双字段是查询应答）。
const REG_URC_PREFIXES: &[&str] = &["CGREG", "CEREG", "CREG"];

/// 一条被吞下的 URC。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrcLine {
    /// 去掉行尾 `\r` 后的原文（保留大小写，解析交给消费端）。
    pub line: String,
    /// 命中的类别，便于消费端分流而不必再扫一遍前缀。
    pub kind: UrcKind,
}

/// URC 类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrcKind {
    /// `VER:` 固件版本上报。
    Ver,
    /// `+GTSTATUS:`。
    GtStatus,
    /// `+GTRGC:`。
    GtRgc,
    /// `+GTUSBMODE:`。
    GtUsbmode,
    /// `GTCELLINFO` / `+GTCELLINFO`。
    GtCellInfo,
    /// `+GTSIM:`。
    GtSim,
    /// `+GTSRV:`。
    GtSrv,
    /// `+GTPIN:`。
    GtPin,
    /// `+GTDSS:`。
    GtDss,
    /// `+CGREG:` / `+CEREG:` / `+CREG:` 的单字段形态。
    Reg,
    /// `+CUSD:`（USSD 应答；查询应答 `+CUSD: (0-2)` 已由 [`cusd_is_urc`] 放行）。
    Cusd,
    /// `+CMTI:`（新短信到达存储位）。
    SmsNew,
}

/// 判定一行是否为「明确可吞」的 URC。
///
/// 返回 `None` 表示放行（进命令应答数据流）。
/// 这一步是纯函数、无 IO，所有判据都在这里，测试直接打它。
pub fn classify_urc(line: &str) -> Option<UrcKind> {
    let l = line.trim();
    if l.is_empty() {
        return None;
    }
    // 剥掉行首的一个 '+'：前缀表统一不带 '+'，两种写法共用一条
    let body = l.strip_prefix('+').unwrap_or(l);
    // 私有前缀：整条吞掉
    for (p, kind) in PRIVATE_URC_PREFIXES {
        if !body.starts_with(p) {
            continue;
        }
        // 前缀后面必须紧跟 ':' 或 ' '，否则是别的词以同样字母开头
        // （例如 "VERSION" 不是 "VER" 的 URC）
        let rest = &body[p.len()..];
        if rest.starts_with(':') || rest.starts_with(' ') {
            return Some(*kind);
        }
    }
    // 注册态：只吞单字段形态（双字段带逗号的是查询应答）
    for p in REG_URC_PREFIXES {
        if let Some(rest) = body.strip_prefix(p) {
            let rest = rest.trim_start_matches(':').trim();
            if !rest.is_empty() && !rest.contains(',') {
                return Some(UrcKind::Reg);
            }
        }
    }
    // USSD 应答：同前缀但要与 `AT+CUSD=?` 的取值范围应答区分。
    // 放在最后判：`CUSD` 与 `CMGS` / `CMGL` 等短信指令只差一个字母，
    // 先按私有表走完再单独判，顺序上更稳。
    if let Some(rest) = body.strip_prefix("CUSD") {
        if rest.starts_with(':') && cusd_is_urc(rest) {
            return Some(UrcKind::Cusd);
        }
    }
    None
}

/// 字节层行分割 + URC 过滤。
///
/// 串口 `read` 不保证按行返回，URC 可能被切在任意字节边界。所有读取点先把
/// chunk 喂进来：完整行走 [`classify_urc`]（命中的不进 `out`），未闭合的
/// 残余留在内部缓冲等下一个 chunk 闭合。
///
/// 残余不主动放行：结果码总是 `\r\n` 结尾，闭合前的残余几乎只会是 URC 分片；
/// 把它留给下一次 `read` 处理比「到期放行半行进应答」更安全。
#[derive(Debug)]
pub struct LineGate {
    buf: Vec<u8>,
    urc: Vec<UrcLine>,
    /// 当前洪水窗口的起点。
    window_start: Instant,
    /// 当前窗口内已放行的 URC 行数。
    window_lines: u32,
    /// 被洪水阈值丢弃的 URC 行数（累计，供排障）。
    dropped: u64,
}

impl Default for LineGate {
    fn default() -> Self {
        Self {
            buf: Vec::new(),
            urc: Vec::new(),
            window_start: Instant::now(),
            window_lines: 0,
            dropped: 0,
        }
    }
}

impl LineGate {
    /// 喂入一段串口字节；**不属于 URC** 的完整行追加进 `out`。
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<u8>) {
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            // 剥行尾 \n（与可选的 \r）
            let mut end = line.len() - 1;
            if end > 0 && line[end - 1] == b'\r' {
                end -= 1;
            }
            let text = String::from_utf8_lossy(&line[..end]);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                // 空行保持原样穿过（normalize 依赖行结构）
                out.extend_from_slice(&line);
                continue;
            }
            match classify_urc(trimmed) {
                Some(kind) => self.push_urc(trimmed.to_string(), kind),
                None => out.extend_from_slice(&line),
            }
        }
    }

    /// 记录一条 URC，受洪水阈值约束。
    fn push_urc(&mut self, line: String, kind: UrcKind) {
        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.window_start = Instant::now();
            self.window_lines = 0;
        }
        self.window_lines += 1;
        if self.window_lines > FLOOD_LINES_PER_SEC {
            // 超限：直接丢弃，既不入队也不放行 —— 放行会让垃圾行污染应答
            self.dropped += 1;
            return;
        }
        if self.urc.len() >= URC_QUEUE_CAP {
            self.urc.remove(0);
        }
        self.urc.push(UrcLine { line, kind });
    }

    /// 取走并清空当前积压的 URC。
    pub fn take_urc(&mut self) -> Vec<UrcLine> {
        std::mem::take(&mut self.urc)
    }

    /// 只取走指定类别的 URC，其余原样留在队列里。
    ///
    /// 为什么需要「只取一类」：`Modem::send_ussd` 要在下发后顺手收下**同步到达**
    /// 的 `+CUSD`，但它绝不能把队列里的 `+GTSIM` / 注册态 URC 一起拿走 ——
    /// 那等于用一个功能的顺手优化，吃掉另一个功能的巡检加速信号。
    /// 取走的行不计入洪水丢弃（它们当时已经获准入队）。
    pub fn take_urc_of(&mut self, kinds: &[UrcKind]) -> Vec<UrcLine> {
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(self.urc.len());
        for u in std::mem::take(&mut self.urc) {
            if kinds.contains(&u.kind) {
                taken.push(u);
            } else {
                kept.push(u);
            }
        }
        self.urc = kept;
        taken
    }

    /// 被洪水阈值丢弃的 URC 行数（累计）。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 缓冲里是否还有未闭合的残余字节（排障用）。
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一整段文本喂进去，返回放行部分。
    fn gate_out(text: &str) -> String {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        g.feed(text.as_bytes(), &mut out);
        String::from_utf8_lossy(&out).to_string()
    }

    // ---------- 判据 ----------

    #[test]
    fn recognizes_private_urc_prefixes() {
        for (line, want) in [
            ("+GTSTATUS: 1,1", UrcKind::GtStatus),
            ("+GTRGC: 3", UrcKind::GtRgc),
            ("+GTUSBMODE: 32", UrcKind::GtUsbmode),
            ("+GTSIM: 1", UrcKind::GtSim),
            ("+GTSRV: 1,2", UrcKind::GtSrv),
            ("+GTPIN: 10", UrcKind::GtPin),
            ("+GTDSS: 1", UrcKind::GtDss),
            ("GTCELLINFO: 1", UrcKind::GtCellInfo),
            ("+GTCELLINFO: 1", UrcKind::GtCellInfo),
            ("VER: 81600.0000.00", UrcKind::Ver),
            ("+CMTI: \"SM\",3", UrcKind::SmsNew),
            ("+CMTI: \"ME\",12", UrcKind::SmsNew),
            ("CMTI: SM,3", UrcKind::SmsNew),
        ] {
            assert_eq!(classify_urc(line), Some(want), "{line}");
        }
    }

    #[test]
    fn recognizes_cusd_urc_by_shape() {
        // 主动上报：第一字段是数字状态码
        for line in [
            "+CUSD: 0,\"余额 12.5 元\",15",
            "+CUSD: 2",
            "+CUSD: 1,\"继续\"",
            "CUSD: 0,\"x\"",
        ] {
            assert_eq!(classify_urc(line), Some(UrcKind::Cusd), "{line}");
        }
    }

    #[test]
    fn cusd_query_response_passes_through() {
        // `AT+CUSD=?` 的应答也是 +CUSD: 开头 —— 同前缀，只能靠形态区分。
        // 注意冒号后直接跟 '('（无空格）也要放行：判据剥的是冒号不是空格。
        for line in [
            "+CUSD: (0-2)",
            "+CUSD:(0-2)",
            "+CUSD: (0-2,15)",
            "+CUSD:(0)",
        ] {
            assert_eq!(
                classify_urc(line),
                None,
                "USSD 支持情况查询应答被吞了：{line}"
            );
        }
    }

    #[test]
    fn recognizes_single_field_registration_urc() {
        for (line, _) in [("+CGREG: 5", ""), ("+CEREG: 5", ""), ("+CREG: 1", "")] {
            assert_eq!(classify_urc(line), Some(UrcKind::Reg), "{line}");
        }
    }

    #[test]
    fn never_swallows_query_responses() {
        // 注册态查询应答是双字段 —— 这是与 URC 的唯一区别，必须放行
        for line in [
            "+CEREG: 1,1",
            "+CGREG: 2,5,\"A1B2\",\"1D2E3F\",7",
            "+CREG: 1,1",
        ] {
            assert_eq!(classify_urc(line), None, "查询应答被吞了：{line}");
        }
        // `+CPIN` 的 URC 与 `AT+CPIN?` 应答同形，一律不吞
        assert_eq!(classify_urc("+CPIN: READY"), None);
        assert_eq!(classify_urc("+CPIN: NOT READY"), None);
        // 本程序查询过的广和通指令，响应行绝不能吞
        // （参考实现用 +GT 宽匹配吞掉 AT+GTDNS 响应的事故）
        for line in [
            "+GTDNS: 1,\"2001:db8::1\"",
            "+GTCCINFO: 460,11,1,2,1",
            "+GTSTATIS: 100,200",
            "+GTSENRDTEMP: 10,46066",
        ] {
            assert_eq!(classify_urc(line), None, "查询应答被吞了：{line}");
        }
        // 普通响应
        for line in ["OK", "ERROR", "+CSQ: 99,99", "+CME ERROR: 3", ""] {
            assert_eq!(classify_urc(line), None, "{line}");
        }
        // 短信读取链路的应答行：与 +CMTI 只差一个字母，绝不能吞
        for line in [
            "+CMGL: 1,\"REC READ\",\"+8613\",,\"26/10/18,12:00:00+32\"",
            "+CMGR: \"REC READ\",\"+8613\"",
            "+CPMS: \"SM\",1,20,\"SM\",1,20",
            "+CMGF: 1",
            "+CMGS: 32",
        ] {
            assert_eq!(classify_urc(line), None, "短信查询应答被吞了：{line}");
        }
    }

    #[test]
    fn single_field_reg_urc_is_safe_to_swallow() {
        // `AT+CEREG?` 在 n=0（URC 关闭）时也只回单字段 `+CEREG: 0`，
        // 与 stat=0 的 URC 完全同形。吞掉它不丢信息：解析层取不到
        // 第 2 字段时 stat 回落 0，与解出 `+CEREG: 0` 的结果一致。
        // 反过来不吞才有害 —— URC 抢先时会把 `+CEREG: 1,1` 挤掉，
        // 注册态误报成「未注册」（regstate::parse 取的是第 2 字段）。
        for line in ["+CEREG: 0", "+CGREG: 0", "+CREG: 0", "+CEREG: 1"] {
            assert_eq!(classify_urc(line), Some(UrcKind::Reg), "{line}");
        }
    }

    #[test]
    fn prefix_requires_separator() {
        // "VERSION" 不是 "VER" 的 URC
        assert_eq!(classify_urc("VERSION: 1.0"), None);
        assert_eq!(classify_urc("VERTEX: 1"), None);
        assert_eq!(classify_urc("VER"), None);
        // "CUSDX" / "CMTIX" 不是 CUSD / CMTI 的 URC
        assert_eq!(classify_urc("CUSDX: 1"), None);
        assert_eq!(classify_urc("CMTIX: 1"), None);
    }

    // ---------- 按类别取走 ----------

    #[test]
    fn take_urc_of_keeps_other_kinds() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        g.feed(
            b"+GTSIM: 1\r\n+CEREG: 5\r\n+CUSD: 0,\"hi\",15\r\n+CMTI: \"SM\",3\r\n",
            &mut out,
        );
        let taken = g.take_urc_of(&[UrcKind::Cusd]);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].line, "+CUSD: 0,\"hi\",15");
        // 其它类别必须原样留着：一个功能的顺手优化不能吃掉巡检加速的信号
        let rest = g.take_urc();
        assert_eq!(rest.len(), 3);
        assert_eq!(rest[0].kind, UrcKind::GtSim);
        assert_eq!(rest[1].kind, UrcKind::Reg);
        assert_eq!(rest[2].kind, UrcKind::SmsNew);
        // 洪水丢弃计数不变（那些行当时已经获准入队）
        assert_eq!(g.dropped(), 0);
    }

    #[test]
    fn take_urc_of_multiple_kinds() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        g.feed(b"+CUSD: 2\r\n+GTSIM: 1\r\n+CMTI: \"ME\",1\r\n", &mut out);
        let taken = g.take_urc_of(&[UrcKind::Cusd, UrcKind::SmsNew]);
        assert_eq!(taken.len(), 2);
        assert_eq!(g.take_urc().len(), 1);
    }

    // ---------- 行分割与洪水 ----------

    #[test]
    fn urc_lines_are_swallowed_and_queued() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        g.feed(b"AT+CSQ\r\r\n+CSQ: 31,99\r\n+CEREG: 5\r\nOK\r\n", &mut out);
        assert_eq!(
            String::from_utf8_lossy(&out),
            "AT+CSQ\r\r\n+CSQ: 31,99\r\nOK\r\n"
        );
        let urc = g.take_urc();
        assert_eq!(urc.len(), 1);
        assert_eq!(urc[0].kind, UrcKind::Reg);
        assert_eq!(urc[0].line, "+CEREG: 5");
        // 取走后队列清空
        assert!(g.take_urc().is_empty());
    }

    #[test]
    fn handles_urc_split_across_reads() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        // URC 行被切成两半，夹在正常响应里
        g.feed(b"+CSQ: 31,99\r\n+CER", &mut out);
        assert_eq!(String::from_utf8_lossy(&out), "+CSQ: 31,99\r\n");
        out.clear();
        g.feed(b"EG: 5\r\nOK\r\n", &mut out);
        assert_eq!(String::from_utf8_lossy(&out), "OK\r\n");
        assert_eq!(g.take_urc().len(), 1);
    }

    #[test]
    fn keeps_unterminated_residue() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        g.feed(b"AT+CGACT?\r", &mut out); // 无 \n 结尾：留在缓冲
        assert!(out.is_empty());
        assert_eq!(g.pending(), 10);
        g.feed(b"\n+CGACT: 1,1\r\nOK\r\n", &mut out);
        let s = String::from_utf8_lossy(&out).to_string();
        assert!(s.contains("+CGACT: 1,1"));
        assert!(s.contains("OK"));
        assert_eq!(g.pending(), 0);
    }

    #[test]
    fn empty_lines_pass_through() {
        // normalize 依赖行结构：空行必须原样穿过
        assert_eq!(
            gate_out("+CSQ: 31,99\r\n\r\nOK\r\n"),
            "+CSQ: 31,99\r\n\r\nOK\r\n"
        );
    }

    #[test]
    fn flood_drops_excess_urc_only() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        // 80 条 URC（超 60/秒阈值）+ 1 条正常应答
        let mut chunk = String::new();
        for _ in 0..80 {
            chunk.push_str("+GTSTATUS: 1\r\n");
        }
        chunk.push_str("+CSQ: 31,99\r\n");
        g.feed(chunk.as_bytes(), &mut out);
        // 应答行一条都不能少
        assert_eq!(String::from_utf8_lossy(&out), "+CSQ: 31,99\r\n");
        // 前 60 条入队，后 20 条被丢
        assert_eq!(g.take_urc().len(), FLOOD_LINES_PER_SEC as usize);
        assert_eq!(g.dropped(), 20);
    }

    #[test]
    fn flood_window_resets_each_second() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        // 第一秒 60 条入队
        for _ in 0..60 {
            g.feed(b"+GTSTATUS: 1\r\n", &mut out);
        }
        assert_eq!(g.take_urc().len(), 60);
        // 手动把窗口起点推到 1 秒以前，模拟下一秒
        g.window_start = Instant::now() - Duration::from_secs(2);
        g.feed(b"+GTSTATUS: 1\r\n", &mut out);
        assert_eq!(g.take_urc().len(), 1);
        assert_eq!(g.dropped(), 0);
    }

    #[test]
    fn queue_overflow_drops_oldest() {
        let mut g = LineGate::default();
        let mut out = Vec::new();
        for i in 0..=URC_QUEUE_CAP {
            // 队列容量 64 > 每秒洪水阈值 60，必须把窗口往前推，
            // 否则测的是洪水而不是队列溢出
            g.window_start = Instant::now() - Duration::from_secs(2);
            g.feed(format!("+GTSTATUS: {i}\r\n").as_bytes(), &mut out);
        }
        let urc = g.take_urc();
        assert_eq!(urc.len(), URC_QUEUE_CAP);
        // 最旧的那条（序号 0）被丢，剩下的是 1..=CAP
        assert_eq!(urc.first().map(|u| u.line.as_str()), Some("+GTSTATUS: 1"));
        assert_eq!(g.dropped(), 0);
    }
}
