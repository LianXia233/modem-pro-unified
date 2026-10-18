//! 短信：PDU / 文本两种读法的解析、长短信拼接、按时间排序与存储区用量。
//!
//! 读短信有两条路径：
//!   · 文本模式（`AT+CMGF=1` + `AT+CMGL="ALL"`）：正文由模组给，但**不带 UDH**，
//!     拼接信息要另切一次 PDU 模式（`AT+CMGF=0` + `AT+CMGL=4`）才拿得到；
//!   · PDU 模式：正文/号码/时间/分条全在 PDU 里，由 [`decode_pdu`] 自行解出。
//!
//! 本模块只做纯解析（可单测），收发与缓存见 [`super::modem::Modem`]。
//!
//! 写侧（发 / 删 / 切存储区）也在本文件末尾，口径要点：
//!   · 发短信走文本模式 `AT+CMGS`：**含非 ASCII 就切 UCS2**（`AT+CSCS="UCS2"` +
//!     `AT+CSMP=17,167,0,8`），地址与正文都按 UCS2 十六进制下发；发完一律切回 IRA，
//!     否则后面读短信时正文会变成一串十六进制（字符集是模组全局状态）；
//!   · `AT+CMGS` 是**两步指令**：先等 `>` 提示符再发正文 + Ctrl-Z，不能用一问一答的写法；
//!   · 删除：`AT+CMGD=<index>` 单条，`AT+CMGD=1,4` 全部（`4` = 删所有类型）。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::Serialize;

use crate::error::{self, msg};

use super::super::at::is_ok;

/// 一条短信。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Sms {
    /// 存储位置（删除该条时用）
    pub index: String,
    pub status: String,
    pub number: String,
    pub timestamp: String,
    pub content: String,
    /// 拼接引用号（同一条长短信的各段相同）
    #[serde(skip_serializing_if = "is_zero")]
    pub part_ref: i32,
    /// 总段数
    #[serde(skip_serializing_if = "is_zero")]
    pub part_total: i32,
    /// 当前段序号（1 起）
    #[serde(skip_serializing_if = "is_zero")]
    pub part_seq: i32,
    /// 各段明细（仅成组的长短信有）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<SmsPart>,
}

/// 长短信中的一段。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SmsPart {
    /// 存储位置（删除该段时用）
    pub index: String,
    pub status: String,
    /// 段序号（1 起）
    pub seq: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timestamp: String,
    pub content: String,
}

/// 短信存储用量（模组会同时上报读/写/接收三个存储区，多数相同）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SmsStorage {
    pub mem1: String,
    pub mem1_used: i32,
    pub mem1_total: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mem2: String,
    /// 用量为 0 时字段也不能消失：前端据此算百分比。
    pub mem2_used: i32,
    pub mem2_total: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mem3: String,
    pub mem3_used: i32,
    pub mem3_total: i32,
    /// 当前存储区（= mem1），前端下拉框靠它选中当前项。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub current: String,
    /// 模组支持的存储区（取自 `AT+CPMS=?`）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supported: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub raw: String,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

/// 一段长短信的拼接信息（来自 UDH 的 IEI=0x00/0x08）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcatInfo {
    pub reference: i32,
    pub total: i32,
    pub seq: i32,
}

// ====================== 字节 / 字符集 ======================

/// 短信字符集（由 DCS 判定，TS 23.038 §4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alphabet {
    Gsm7,
    EightBit,
    Ucs2,
}

/// 从 DCS 判断字符集；压缩位与保留值按 7 位处理。
fn alphabet_of(dcs: u8) -> Alphabet {
    if dcs & 0xC0 == 0x00 || dcs & 0xF0 == 0xF0 {
        match (dcs >> 2) & 0x03 {
            0x01 => return Alphabet::EightBit,
            0x02 => return Alphabet::Ucs2,
            _ => {}
        }
    }
    Alphabet::Gsm7
}

/// 十六进制串 → 字节（长度必须为偶数）。
pub fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in 0..out.capacity() {
        let v = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        out.push(v);
    }
    Some(out)
}

/// 字节 → 大写十六进制。
pub fn hex_string(b: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(b.len() * 2);
    for c in b {
        s.push(DIGITS[(c >> 4) as usize] as char);
        s.push(DIGITS[(c & 0x0F) as usize] as char);
    }
    s
}

/// 是否只有十六进制字符。
fn is_hex_only(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// UTF-16BE 解码（含代理对）；末尾畸形半格丢掉。
pub fn decode_ucs2(b: &[u8]) -> String {
    let b = if b.len() % 2 != 0 {
        &b[..b.len() - 1]
    } else {
        b
    };
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// GSM 03.38 默认字母表（0x1B 是转义前缀，占位）。
const GSM7_DEFAULT: [char; 128] = [
    '@', '£', '$', '¥', 'è', 'é', 'ù', 'ì', 'ò', 'Ç', '\n', 'Ø', 'ø', '\r', 'Å', 'å', 'Δ', '_',
    'Φ', 'Γ', 'Λ', 'Ω', 'Π', 'Ψ', 'Σ', 'Θ', 'Ξ', '\u{1B}', 'Æ', 'æ', 'ß', 'É', ' ', '!', '"', '#',
    '¤', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/', '0', '1', '2', '3', '4', '5', '6',
    '7', '8', '9', ':', ';', '<', '=', '>', '?', '¡', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I',
    'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'Ä', 'Ö',
    'Ñ', 'Ü', '§', '¿', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'ä', 'ö', 'ñ', 'ü', 'à',
];

/// GSM 03.38 扩展字母表（0x1B 之后的码位 → 字符）。
///
/// 编解码共用这一份表：两处各写一份必然漂移，而漂移的后果是
/// 「自己能解出来的短信自己发不出去」。
const GSM7_EXT: [(u8, char); 9] = [
    (0x0A, '\u{0C}'),
    (0x14, '^'),
    (0x28, '{'),
    (0x29, '}'),
    (0x2F, '\\'),
    (0x3C, '['),
    (0x3E, ']'),
    (0x40, '|'),
    (0x65, '€'),
];

/// 转义（0x1B）之后的扩展字符。
fn gsm7_ext(c: u8) -> Option<char> {
    GSM7_EXT
        .iter()
        .find(|(code, _)| *code == c)
        .map(|(_, ch)| *ch)
}

/// 文本 → septet 序列（0x1B 转义字符展开成两个 septet）。
///
/// 有一个字符不在默认字母表 / 扩展表里就返回 `None`，调用方据此改走 UCS2。
/// **扩展表必须先查**：默认表第 27 位的占位符正是 `0x1B` 本身，
/// 顺序反了会把 `€` 编码成裸转义符。
pub fn string_to_gsm7(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for c in text.chars() {
        if let Some((code, _)) = GSM7_EXT.iter().find(|(_, ch)| *ch == c) {
            out.push(0x1B);
            out.push(*code);
            continue;
        }
        out.push(GSM7_DEFAULT.iter().position(|x| *x == c)? as u8);
    }
    Some(out)
}

/// septet 序列 → LSB 在前的 7 位压缩字节（`unpack7bit` 的逆运算）。
///
/// 末字节不足 7 位的地方补 0，与模组行为一致。
pub fn pack7(seps: &[u8]) -> Vec<u8> {
    let n = seps.len();
    let mut out = vec![0u8; (n * 7).div_ceil(8)];
    for (i, &s) in seps.iter().enumerate() {
        let v = (s & 0x7F) as u16;
        let bit = i * 7;
        let (idx, off) = (bit / 8, bit % 8);
        out[idx] |= (v << off) as u8;
        // off >= 2 时高位会溢出当前字节，需要落到下一字节；
        // off 为 0 / 1 时 7 位数据刚好还在本字节内。
        if off > 1 && idx + 1 < out.len() {
            out[idx + 1] |= (v >> (8 - off)) as u8;
        }
    }
    out
}

/// LSB 在前的 7 位压缩数据 → n 个 septet。
fn unpack7bit(b: &[u8], n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| {
            let bit = i * 7;
            let (idx, off) = (bit / 8, (bit % 8) as u32);
            let mut v = 0u16;
            if idx < b.len() {
                v = (b[idx] as u16) >> off;
            }
            if idx + 1 < b.len() {
                v |= (b[idx + 1] as u16) << (8 - off);
            }
            (v & 0x7F) as u8
        })
        .collect()
}

/// UDH 占用 n 字节时，7 位流里被顶掉的 septet 数（向上取整）。
fn septet_pad(n_bytes: usize) -> usize {
    let bits = n_bytes * 8;
    if bits % 7 == 0 {
        bits / 7
    } else {
        bits / 7 + 1
    }
}

/// septet 序列 → 文本（0x1B 走扩展表）。
fn decode_gsm7(seps: &[u8]) -> String {
    let mut s = String::new();
    let mut i = 0;
    while i < seps.len() {
        let c = seps[i];
        if c == 0x1B {
            if let Some(&next) = seps.get(i + 1) {
                if let Some(r) = gsm7_ext(next) {
                    s.push(r);
                }
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        s.push(GSM7_DEFAULT[(c & 0x7F) as usize]);
        i += 1;
    }
    // 7 位数据末尾常见补位 CR，不是正文
    s.trim_end_matches('\r').to_string()
}

// ====================== PDU ======================

/// 半字节 → 号码字符；0x0F（补位）与越界值不产出。
fn bcd_digit(v: u8) -> Option<char> {
    match v {
        0..=9 => Some((b'0' + v) as char),
        0x0A => Some('*'),
        0x0B => Some('#'),
        _ => None,
    }
}

/// 一个字节 → 两位数字符串（低半字节在前）；含非法半字节返回 None。
fn bcd_pair(x: u8) -> Option<String> {
    let (lo, hi) = (x & 0x0F, x >> 4);
    if lo > 9 || hi > 9 {
        return None;
    }
    Some(format!("{}{}", (b'0' + lo) as char, (b'0' + hi) as char))
}

/// 解码 PDU 里的地址字段：1 字节"数字个数" + 1 字节类型 + BCD 半字节。
///
/// 号码前**不**无脑补 `+`：只有 TON = 001 才算国际号。
fn decode_pdu_address(b: &[u8], p: usize) -> Result<(String, usize), String> {
    if p + 1 >= b.len() {
        return Err("pdu 在地址字段处被截断".into());
    }
    let digits = b[p] as usize;
    let toa = b[p + 1];
    let p = p + 2;
    if digits == 0 {
        return Ok((String::new(), p)); // 状态报告没有号码
    }
    if digits > 20 {
        return Err(format!("pdu 地址长度异常: {digits}"));
    }
    let n_bytes = digits.div_ceil(2);
    if p + n_bytes > b.len() {
        return Err("pdu 在地址数字处被截断".into());
    }
    let mut num = String::new();
    for i in 0..n_bytes {
        let x = b[p + i];
        if let Some(c) = bcd_digit(x & 0x0F) {
            num.push(c);
        }
        if let Some(c) = bcd_digit((x >> 4) & 0x0F) {
            num.push(c);
        }
    }
    if (toa >> 4) & 0x07 == 0x01 {
        num.insert(0, '+');
    }
    Ok((num, p + n_bytes))
}

/// 7 字节 BCD 的 TP-SCTS → 与文本模式一致的 `YY/MM/DD,HH:MM:SS±ZZ`。
///
/// 时区单位是 15 分钟，bit3 是符号位；BCD 值异常时返回空串（上层按"时间未知"排在最后）。
pub fn pdu_time(b: &[u8]) -> String {
    if b.len() < 7 {
        return String::new();
    }
    let mut parts = Vec::with_capacity(6);
    for x in &b[..6] {
        match bcd_pair(*x) {
            Some(s) => parts.push(s),
            None => return String::new(),
        }
    }
    let mag = (b[6] & 0x07) as i32 * 10 + (b[6] >> 4) as i32;
    let tz = if mag <= 56 {
        let sign = if b[6] & 0x08 != 0 { '-' } else { '+' };
        format!("{sign}{mag:02}")
    } else {
        String::new()
    };
    format!(
        "{}/{}/{},{}:{}:{}{}",
        parts[0], parts[1], parts[2], parts[3], parts[4], parts[5], tz
    )
}

/// 从一条 `SMS-DELIVER` 的 PDU 里提取拼接 IE（只解到 UDH）。
pub fn parse_concat_udh(b: &[u8]) -> Option<ConcatInfo> {
    let p = 1 + *b.first()? as usize;
    let fo = *b.get(p)?;
    let p = p + 1;
    if fo & 0x03 != 0 || fo & 0x40 == 0 {
        return None;
    }
    // 发件人地址
    let len = *b.get(p)? as usize;
    let mut p = p + 2 + len.div_ceil(2);
    // PID + DCS + SCTS(7) + UDL
    p += 10;
    let udhl = *b.get(p)? as usize;
    let p = p + 1;
    let end = p + udhl;
    if udhl == 0 || end > b.len() {
        return None;
    }
    let mut i = p;
    while i + 1 < end {
        let (iei, iel) = (b[i], b[i + 1] as usize);
        let body = i + 2;
        if body + iel > end {
            break; // 残缺 IE，放弃
        }
        match iei {
            0x00 if iel >= 3 => {
                return Some(ConcatInfo {
                    reference: b[body] as i32,
                    total: b[body + 1] as i32,
                    seq: b[body + 2] as i32,
                })
            }
            0x08 if iel >= 4 => {
                return Some(ConcatInfo {
                    reference: ((b[body] as i32) << 8) | b[body + 1] as i32,
                    total: b[body + 2] as i32,
                    seq: b[body + 3] as i32,
                })
            }
            _ => {}
        }
        i = body + iel;
    }
    None
}

/// 解码一条 PDU（SMS-DELIVER / SMS-SUBMIT）。
///
/// 只填号码/时间/正文与拼接信息；存储位置与状态由调用方从 `+CMGL:` 头里取。
pub fn decode_pdu(pdu: &str) -> Result<(Sms, ConcatInfo), String> {
    let up = pdu.trim().to_uppercase();
    let b = hex_to_bytes(&up).ok_or("pdu 不是合法的十六进制串")?;
    if b.len() < 2 {
        return Err("pdu 不是合法的十六进制串".into());
    }
    let p = 1 + b[0] as usize;
    let fo = *b.get(p).ok_or("pdu 在短信中心地址处被截断")?;
    let mut p = p + 1;
    let (mti, udhi) = (fo & 0x03, fo & 0x40 != 0);
    // SMS-SUBMIT 比 DELIVER 多一个 TP-MR（在地址**之前**），有效期长度也由 FO 决定
    let mut vp_len = 0usize;
    match mti {
        0x00 => {}
        0x01 => {
            if p >= b.len() {
                return Err("pdu 在 TP-MR 处被截断".into());
            }
            p += 1;
            match (fo >> 3) & 0x03 {
                0x01 | 0x03 => vp_len = 7,
                0x02 => vp_len = 1,
                _ => {}
            }
        }
        _ => return Err("unsupported PDU type".into()),
    }
    let (num, np) = decode_pdu_address(&b, p)?;
    p = np;
    let mut out = Sms {
        number: num,
        ..Default::default()
    };
    if p + 1 >= b.len() {
        return Err("pdu 在 PID/DCS 处被截断".into());
    }
    p += 1; // TP-PID 用不上
    let dcs = b[p];
    p += 1;
    if mti == 0x00 {
        if p + 7 > b.len() {
            return Err("pdu 在 TP-SCTS 处被截断".into());
        }
        out.timestamp = pdu_time(&b[p..p + 7]);
        p += 7;
    }
    p += vp_len;
    let udl = *b.get(p).ok_or("pdu 在 UDL 处被截断")? as usize;
    let p = p + 1;
    let ud = &b[p..];

    let mut ci = ConcatInfo {
        reference: 0,
        total: 0,
        seq: 0,
    };
    let mut has_ci = false;
    if mti == 0x00 {
        if let Some(c) = parse_concat_udh(&b) {
            ci = c;
            has_ci = true;
        }
    }

    let mut data = ud;
    let mut udh_bytes = 0usize;
    if udhi && !ud.is_empty() {
        udh_bytes = 1 + ud[0] as usize;
        if udh_bytes > ud.len() {
            return Err("pdu 的 UDH 长度越界".into());
        }
        data = &ud[udh_bytes..];
    }
    out.content = match alphabet_of(dcs) {
        Alphabet::Ucs2 => decode_ucs2(data),
        Alphabet::EightBit => hex_string(data),
        Alphabet::Gsm7 => {
            let mut seps = unpack7bit(ud, udl);
            let skip = septet_pad(udh_bytes);
            if skip > 0 {
                seps = if skip < seps.len() {
                    seps[skip..].to_vec()
                } else {
                    Vec::new()
                };
            }
            decode_gsm7(&seps)
        }
    };
    if !has_ci {
        ci = ConcatInfo {
            reference: 0,
            total: 0,
            seq: 0,
        };
    }
    Ok((out, ci))
}

// ====================== +CMGL 应答解析 ======================

/// 按逗号切分 `+CMGL` 行字段：引号内的逗号不分割，空字段保留。
pub fn split_cmgl_fields(s: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for r in s.chars() {
        match r {
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                fields.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(r),
        }
    }
    fields.push(cur.trim().to_string());
    fields
}

/// 解析一条 `+CMGL` 头：`<index>,<stat>,<oa/da>,[<alpha>],[<scts>]`。
fn parse_cmgl_header(line: &str) -> Option<Sms> {
    let up = line.to_uppercase();
    let idx = up.find("+CMGL:")?;
    let f = split_cmgl_fields(&line[idx + "+CMGL:".len()..]);
    if f.len() < 2 || f[0].is_empty() {
        return None;
    }
    Some(Sms {
        index: f[0].clone(),
        status: f[1].clone(),
        number: f.get(2).cloned().unwrap_or_default(),
        timestamp: f.get(4).cloned().unwrap_or_default(),
        ..Default::default()
    })
}

/// 解析文本模式（`AT+CMGL="ALL"`）的完整应答。
///
/// 正文可能跨多行，一直累加到下一条 `+CMGL` 或应答结束。
pub fn parse_sms_list(resp: &str) -> Vec<Sms> {
    let mut list = Vec::new();
    let mut cur: Option<Sms> = None;
    for line in resp.lines() {
        if let Some(mut s) = parse_cmgl_header(line) {
            if let Some(prev) = cur.take() {
                list.push(finish_sms(prev));
            }
            s.content = String::new();
            cur = Some(s);
            continue;
        }
        let t = line.trim();
        if t.is_empty()
            || t == "OK"
            || t == "ERROR"
            || t.starts_with("AT+")
            || t.starts_with("+CME")
            || t.starts_with("+CMS")
        {
            continue;
        }
        if let Some(s) = cur.as_mut() {
            if s.content.is_empty() {
                s.content = t.to_string();
            } else {
                s.content.push('\n');
                s.content.push_str(t);
            }
        }
    }
    if let Some(prev) = cur.take() {
        list.push(finish_sms(prev));
    }
    list
}

/// 收尾：文本模式下的正文可能是 UTF-16BE 十六进制，尝试还原。
fn finish_sms(mut s: Sms) -> Sms {
    s.content = decode_sms_text(&s.content);
    s
}

/// 解析 PDU 模式（`AT+CMGF=0` + `AT+CMGL=4`）的应答。
///
/// 返回短信列表与「存储位置 → 拼接信息」。夹杂在应答里的 URC 行不是十六进制，
/// 会被跳过；只有消费掉一条 PDU 之后才认下一段，因此不会错配。
pub fn parse_pdu_list(resp: &str) -> (Vec<Sms>, HashMap<String, ConcatInfo>) {
    let mut list: Vec<Sms> = Vec::new();
    let mut concat = HashMap::new();
    let mut cur: Option<usize> = None;
    for line in resp.lines() {
        let up = line.to_uppercase();
        if let Some(idx) = up.find("+CMGL:") {
            let f = split_cmgl_fields(&line[idx + "+CMGL:".len()..]);
            cur = None;
            if f.is_empty() || f[0].is_empty() {
                continue;
            }
            let mut s = Sms {
                index: f[0].clone(),
                ..Default::default()
            };
            if let Some(st) = f.get(1) {
                s.status = st.clone();
            }
            list.push(s);
            cur = Some(list.len() - 1);
            continue;
        }
        let Some(i) = cur else { continue };
        let t = line.trim();
        if t.is_empty() || !is_hex_only(t) || t.len() % 2 != 0 {
            continue;
        }
        if let Ok((mut s, ci)) = decode_pdu(t) {
            s.index = list[i].index.clone();
            s.status = list[i].status.clone();
            list[i] = s;
            if ci.total >= 2 && ci.seq >= 1 && ci.seq <= ci.total {
                concat.insert(list[i].index.clone(), ci);
            }
        }
        cur = None;
    }
    (list, concat)
}

/// 解析 PDU 模式读全部短信的应答，只返回「存储位置 → 拼接信息」。
pub fn parse_pdu_concat_list(resp: &str) -> HashMap<String, ConcatInfo> {
    let mut out = HashMap::new();
    let mut index = String::new();
    for line in resp.lines() {
        let up = line.to_uppercase();
        if let Some(i) = up.find("+CMGL:") {
            let f = split_cmgl_fields(&line[i + "+CMGL:".len()..]);
            if let Some(first) = f.first() {
                if !first.is_empty() {
                    index = first.clone();
                }
            }
            continue;
        }
        let t = line.trim();
        if index.is_empty()
            || t.is_empty()
            || t == "OK"
            || t == "ERROR"
            || t.starts_with("AT+")
            || t.starts_with("+CME")
            || t.starts_with("+CMS")
        {
            continue;
        }
        if let Some(b) = hex_to_bytes(t) {
            if let Some(ci) = parse_concat_udh(&b) {
                out.insert(index.clone(), ci);
            }
        }
        index.clear(); // 一行 PDU 对应一条记录
    }
    out
}

// ====================== 长短信拼接 ======================

/// 把同一条长短信的各段合并为「父条目 + parts 子表」。
///
/// `concat` 为空时原样返回（退化为平铺列表）。
pub fn group_sms_concat(list: Vec<Sms>, concat: &HashMap<String, ConcatInfo>) -> Vec<Sms> {
    if concat.is_empty() {
        return list;
    }
    struct Group {
        out_idx: usize,
        seqs: HashSet<i32>,
    }
    let mut pending: HashMap<String, Vec<Group>> = HashMap::new();
    let mut out: Vec<Sms> = Vec::with_capacity(list.len());

    for mut s in list {
        let Some(ci) = concat.get(&s.index).copied() else {
            out.push(s);
            continue;
        };
        if ci.total < 2 || ci.seq < 1 || ci.seq > ci.total {
            out.push(s);
            continue;
        }
        s.part_ref = ci.reference;
        s.part_total = ci.total;
        s.part_seq = ci.seq;

        // 同一 key 下可能有多组（引用号是 8 位，会撞号），段号重复时另起一组
        let key = format!("{}|{}|{}", s.number, ci.reference, ci.total);
        let groups = pending.entry(key).or_default();
        let target = groups
            .iter_mut()
            .find(|g| !g.seqs.contains(&ci.seq))
            .map(|g| g.out_idx);
        let out_idx = match target {
            Some(idx) => idx,
            None => {
                out.push(s.clone()); // 先以本段作为父条目占位
                let idx = out.len() - 1;
                groups.push(Group {
                    out_idx: idx,
                    seqs: HashSet::new(),
                });
                idx
            }
        };
        if let Some(g) = groups.iter_mut().find(|g| g.out_idx == out_idx) {
            g.seqs.insert(ci.seq);
        }
        out[out_idx].parts.push(SmsPart {
            index: s.index.clone(),
            status: s.status.clone(),
            seq: ci.seq,
            timestamp: s.timestamp.clone(),
            content: s.content.clone(),
        });
    }

    for item in out.iter_mut() {
        if item.parts.len() < 2 {
            // 只有 1 段（其余段缺失/已删）不算分条：清掉子表，保留 part_total/part_seq
            item.parts.clear();
            continue;
        }
        item.parts.sort_by_key(|p| p.seq);
        item.content = item.parts.iter().map(|p| p.content.as_str()).collect();
        let first = item.parts[0].clone();
        item.index = first.index;
        item.status = first.status;
        item.timestamp = first.timestamp;
        item.part_seq = first.seq;
    }
    out
}

// ====================== 排序 / 未读统计 ======================

/// 3GPP 时间戳 → 可比较整数（只取日期与时间，时区不参与比较）。
pub fn time_key(ts: &str) -> Option<i64> {
    let ts = ts.trim();
    let bytes = ts.as_bytes();
    let num = |i: usize| -> Option<i64> {
        let s = ts.get(i..i + 2)?;
        if s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    if bytes.len() < 17
        || bytes.get(2) != Some(&b'/')
        || bytes.get(5) != Some(&b'/')
        || bytes.get(8) != Some(&b',')
        || bytes.get(11) != Some(&b':')
        || bytes.get(14) != Some(&b':')
    {
        return None;
    }
    let (y, mo, d) = (num(0)?, num(3)?, num(6)?);
    let (h, mi, s) = (num(9)?, num(12)?, num(15)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return None; // 模组偶尔会吐全 0 或越界的时间
    }
    Some((2000 + y) * 10_000_000_000 + mo * 100_000_000 + d * 1_000_000 + h * 10_000 + mi * 100 + s)
}

/// 按时间倒序排列（最新的在最前）；时间无效的条目排在最后并保持原相对顺序。
pub fn sort_newest_first(list: Vec<Sms>) -> Vec<Sms> {
    let mut indexed: Vec<(usize, Sms)> = list.into_iter().enumerate().collect();
    let keys: Vec<Option<i64>> = indexed
        .iter()
        .map(|(_, s)| time_key(&s.timestamp))
        .collect();
    indexed.sort_by(|(ia, _), (ib, _)| match (keys[*ia], keys[*ib]) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    indexed.into_iter().map(|(_, s)| s).collect()
}

/// 单条短信的状态是否为「未读」。
///
/// 文本模式是 `REC UNREAD` 字样；PDU 模式的 `<stat>` 是数字，0 = 未读。
/// 空状态判未读 = false：拿不到状态时宁可少算。
pub fn sms_status_unread(status: &str) -> bool {
    let st = status.trim().trim_matches('"').trim();
    if st.is_empty() {
        return false;
    }
    if st.to_uppercase().contains("UNREAD") {
        return true;
    }
    st.parse::<i64>().map(|n| n == 0).unwrap_or(false)
}

/// 统计未读条数。
pub fn count_unread(list: &[Sms]) -> usize {
    list.iter().filter(|s| sms_status_unread(&s.status)).count()
}

// ====================== 存储用量 ======================

/// 存储区名称校验（会拼进 `AT+CPMS="…","…","…"`，必须卡死）。
pub fn valid_mem(mem: &str) -> bool {
    matches!(mem.trim().to_uppercase().as_str(), "SM" | "ME" | "MT")
}

// ====================== 写操作：发送 / 删除 / 切存储区 ======================

/// 单条短信的容量（UCS2 70 个字符、7bit 160 个），超了要分段。
const UCS2_LIMIT: usize = 70;
const ASCII_LIMIT: usize = 160;
/// 发一条要比普通命令耐心：模组要等到网络回执才吐 OK。
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// 一次发送的结果。
#[derive(Debug, Clone, Serialize)]
pub struct SmsSendResult {
    pub modem_id: u32,
    pub phone: String,
    /// 实际分了几条发出去（超长自动分段）
    pub parts: usize,
    /// 是否走了 UCS2（中文 / emoji 会走）
    pub ucs2: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl super::Modem {
    /// 发一条短信（超长自动分段）。
    pub fn send_sms(&self, phone: &str, content: &str) -> error::Result<SmsSendResult> {
        let phone = phone.trim();
        let content = content.trim();
        if phone.is_empty() {
            return Err(msg("收件号码不能为空"));
        }
        if content.is_empty() {
            return Err(msg("短信内容不能为空"));
        }
        // 广和通 MTK 平台（FM350 系列）走 PDU 模式：文本模式在它上面
        // 中文与长短信都不可靠（实测），所以整条发送序列换成 PDU。
        if self.is_fibocom_mtk() {
            return self.send_sms_pdu(phone, content);
        }
        // 号码里的 + 在 UCS2 下也要一起编码，所以只看有没有非 ASCII 字符
        let ucs2 = !content.is_ascii() || !phone.is_ascii();
        let parts = split_content(content, ucs2);
        let mut note = None;
        if parts.len() > 1 {
            note = Some(format!("内容超长，已分成 {} 条发出", parts.len()));
        }

        self.with_at(|p| {
            let t = self.cmd_timeout;
            p.command("AT+CMGF=1", t)?;
            if ucs2 {
                p.command("AT+CSCS=\"UCS2\"", t)?;
                p.command("AT+CSMP=17,167,0,8", t)?;
            } else {
                p.command("AT+CSCS=\"IRA\"", t)?;
            }
            for part in &parts {
                let addr = if ucs2 {
                    utf16_hex(phone)
                } else {
                    phone.to_string()
                };
                let body = if ucs2 { utf16_hex(part) } else { part.clone() };
                p.write_raw(&format!("AT+CMGS=\"{addr}\"\r"))?;
                let prompt = p.read_until_marker(">", t)?;
                if !prompt.contains('>') {
                    return Err(msg("模组没给出发送提示符（>），短信未发出"));
                }
                p.write_raw(&format!("{body}\u{1a}"))?;
                let resp = p.read_response(SEND_TIMEOUT)?;
                if !is_ok(&resp) {
                    return Err(msg(format!("模组拒绝发送：{}", resp.trim())));
                }
            }
            Ok(())
        })?;
        // 字符集是全局状态：不切回来的话，之后读到的正文会是一串十六进制
        let _ = self.at_cmd("AT+CSCS=\"IRA\"");

        Ok(SmsSendResult {
            modem_id: self.id,
            phone: phone.to_string(),
            parts: parts.len(),
            ucs2,
            note,
        })
    }

    /// PDU 模式发短信（广和通 MTK 平台专用路径）。
    ///
    /// 序列：`AT+CMGF=0` → `AT+CMGS=<TPDU 字节数>` → 等 `>` 提示符 →
    /// 写十六进制 PDU + Ctrl-Z → 等 OK。
    ///
    /// 与文本模式的差别不只是编码：文本模式在该平台上中文与长短信都不可靠，
    /// 所以这里是整条序列替换，不是在原序列上打补丁。
    fn send_sms_pdu(&self, phone: &str, content: &str) -> error::Result<SmsSendResult> {
        let parts = split_content_for_pdu(content);
        let mut note = None;
        if parts.len() > 1 {
            note = Some(format!("内容超长，已分成 {} 条发出", parts.len()));
        }

        let mut ucs2 = false;
        let total = parts.len();
        self.with_at(|p| {
            let t = self.cmd_timeout;
            p.command("AT+CMGF=0", t)?;
            // 分段发送中途失败时，要说清「已经成功几条、一共几条」——
            // 只报一句「发送失败」的话，用户不知道该不该重发整条
            for (i, part) in parts.iter().enumerate() {
                let (pdu, len) = build_submit_pdu(phone, part).map_err(msg)?;
                ucs2 = string_to_gsm7(part).is_none();
                p.write_raw(&format!("AT+CMGS={len}\r"))?;
                let prompt = p.read_until_marker(">", t)?;
                if !prompt.contains('>') {
                    return Err(msg(format!(
                        "第 {} 条没给出发送提示符（>），已成功 {}/{} 条",
                        i + 1,
                        i,
                        total
                    )));
                }
                p.write_raw(&format!("{pdu}\u{1a}"))?;
                let resp = p.read_response(SEND_TIMEOUT)?;
                if !is_ok(&resp) {
                    return Err(msg(format!(
                        "第 {} 条被模组拒绝（已成功 {}/{} 条）：{}",
                        i + 1,
                        i,
                        total,
                        resp.trim()
                    )));
                }
            }
            Ok(())
        })?;
        // 短信模式是全局状态：不切回文本模式的话，后续按文本模式读的操作会失效
        let _ = self.at_cmd("AT+CMGF=1");

        Ok(SmsSendResult {
            modem_id: self.id,
            phone: phone.to_string(),
            parts: parts.len(),
            ucs2,
            note,
        })
    }

    /// 删一条短信（按存储位置）。
    pub fn delete_sms(&self, index: &str) -> error::Result<()> {
        if index.trim().is_empty() || !index.trim().chars().all(|c| c.is_ascii_digit()) {
            return Err(msg("存储位置必须是数字"));
        }
        let resp = self.with_at(|p| {
            p.command(
                &format!("AT+CMGD={}", index.trim()),
                Duration::from_secs(10),
            )
        })?;
        if is_ok(&resp) {
            Ok(())
        } else {
            Err(msg(format!("删除第 {index} 条失败：{}", resp.trim())))
        }
    }

    /// 把一条短信标成已读（按存储位置）。
    ///
    /// 3GPP 的口径就是「读一次即已读」：`AT+CMGR=<index>` 成功返回后，模组的未读
    /// 状态就清掉了（没有单独的"标已读"指令）。列表/角标由调用方重读后推送。
    pub fn mark_sms_read(&self, index: &str) -> error::Result<()> {
        if index.trim().is_empty() || !index.trim().chars().all(|c| c.is_ascii_digit()) {
            return Err(msg("存储位置必须是数字"));
        }
        let resp =
            self.with_at(|p| p.command(&format!("AT+CMGR={}", index.trim()), self.cmd_timeout))?;
        if is_ok(&resp) {
            Ok(())
        } else {
            Err(msg(format!("读取第 {index} 条失败：{}", resp.trim())))
        }
    }

    /// 删全部短信（`AT+CMGD=1,4`）。
    pub fn delete_all_sms(&self) -> error::Result<()> {
        let resp = self.with_at(|p| p.command("AT+CMGD=1,4", Duration::from_secs(30)))?;
        if is_ok(&resp) {
            Ok(())
        } else {
            Err(msg(format!("删除全部短信失败：{}", resp.trim())))
        }
    }

    /// 切换短信存储区（读/写/接收三处一起切），并回读确认。
    ///
    /// 回读是必须的：实测有些模组（EM05）刚下发时会短暂报成 MT，得让界面看到真值。
    /// 把配置里的存储区偏好应用到模组（每个会话最多成功一次；失败只记日志）。
    pub(crate) fn apply_sms_memory_pref(&self) {
        let pref = self.sms_mem_pref.lock().unwrap().clone();
        if pref.is_empty() {
            return;
        }
        if self.sms_mem_active.lock().unwrap().as_deref() == Some(pref.as_str()) {
            return;
        }
        match self.set_sms_memory(&pref) {
            Ok(st) => {
                log::info!("短信存储区已按配置切到 {}（[sms] sms_memory）", st.current);
            }
            Err(e) => {
                // 不把「偏好在固件上不支持」升级成读短信失败：沿用当前存储区即可
                log::warn!("应用 [sms] sms_memory={pref} 失败，沿用当前存储区: {e}");
                *self.sms_mem_active.lock().unwrap() = Some(String::new());
            }
        }
    }

    pub fn set_sms_memory(&self, mem: &str) -> error::Result<SmsStorage> {
        let m = mem.trim().to_uppercase();
        if !valid_mem(&m) {
            return Err(msg(format!("不支持的存储区 {mem}（只认 SM / ME / MT）")));
        }
        let resp = self.with_at(|p| {
            p.command(
                &format!("AT+CPMS=\"{m}\",\"{m}\",\"{m}\""),
                Duration::from_secs(5),
            )
        })?;
        if !is_ok(&resp) {
            return Err(msg(format!("切换存储区失败：{}", resp.trim())));
        }
        let back = self.with_at(|p| p.command("AT+CPMS?", self.cmd_timeout))?;
        let st = parse_storage(&back).ok_or_else(|| msg("切换后回读存储区失败"))?;
        *self.sms_mem_active.lock().unwrap() = Some(st.current.to_uppercase());
        Ok(st)
    }
}

/// 按单条容量切分正文（按**字符**切，超宽的 emoji 在 UCS2 里占两个单元也算两个字符）。
pub fn split_content(content: &str, ucs2: bool) -> Vec<String> {
    let limit = if ucs2 { UCS2_LIMIT } else { ASCII_LIMIT };
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + limit).min(chars.len());
        out.push(chars[start..end].iter().collect());
        start = end;
    }
    out
}

/// 字符串 → UTF-16BE 十六进制（UCS2 模式下的地址与正文都要这么写）。
pub fn utf16_hex(s: &str) -> String {
    s.encode_utf16()
        .map(|u| format!("{u:04X}"))
        .collect::<Vec<_>>()
        .join("")
}

/// 字符串 → UTF-16BE 字节（PDU 里 UCS2 正文的编码形式）。
pub fn utf16_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

/// GSM7 单条上限（septet 数）；UCS2 单条上限 70 个 UTF-16 单元（140 字节）。
const GSM7_PDU_LIMIT: usize = 160;
const UCS2_PDU_UNITS: usize = 70;

/// 单个字符在 GSM7 里占几个 septet（扩展字符是 2）。
fn gsm7_cost(c: char) -> usize {
    if GSM7_EXT.iter().any(|(_, ch)| *ch == c) {
        2
    } else {
        1
    }
}

/// 按单条容量切分正文（PDU 模式，按**真实占用的 septet / UTF-16 单元**切）。
///
/// 与文本模式的 `split_content` 的区别：这里按实际容量算 ——
/// GSM7 的扩展字符占 2 个 septet、emoji 占 2 个 UTF-16 单元，
/// 按字符数切会超出单条上限，模组要么拒发要么静默截断。
pub fn split_content_for_pdu(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let gsm7 = string_to_gsm7(content).is_some();
    let (limit, cost): (usize, fn(char) -> usize) = if gsm7 {
        (GSM7_PDU_LIMIT, gsm7_cost)
    } else {
        (UCS2_PDU_UNITS, |c| c.len_utf16())
    };

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut used = 0usize;
    for c in chars {
        let cost = cost(c);
        // 单个字符就超限时也要能发出去：先落地当前段，再单独起一段
        if used + cost > limit && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(c);
        used += cost;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 构造 SMS-SUBMIT PDU（SMSC 长度置 0 → 使用模组内默认短信中心）。
///
/// 返回 `(十六进制 PDU, TPDU 字节数)`。注意 `AT+CMGS=<n>` 要的是 **TPDU
/// 字节数**，不含前导的 SMSC 长度字节 —— 多算一个字节模组会一直等数据。
///
/// 编码自适应：全文能塞进 GSM 7-bit 表就走 7-bit（更省、单条 160 字符），
/// 否则回落 UCS2（中文必然走这条）。
pub fn build_submit_pdu(number: &str, text: &str) -> Result<(String, usize), String> {
    // `+` 开头或 `86` 开头按国际格式（0x91），否则国内格式（0x81）
    let (addr_type, digits) = if let Some(d) = number.strip_prefix('+') {
        (0x91u8, d.to_string())
    } else if number.starts_with("86") {
        (0x91u8, number.to_string())
    } else {
        (0x81u8, number.to_string())
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err("收件人号码格式无效".to_string());
    }

    // 地址按数字成对交换，奇数位末字节高半字节填 0xF
    let chars: Vec<char> = digits.chars().collect();
    let mut addr_bytes = Vec::new();
    let mut k = 0usize;
    while k < chars.len() {
        let hi = chars[k].to_digit(10).unwrap_or(0) as u8;
        let lo = chars.get(k + 1).and_then(|c| c.to_digit(10)).unwrap_or(0xF) as u8;
        addr_bytes.push((lo << 4) | hi);
        k += 2;
    }

    let mut tpdu: Vec<u8> = vec![
        0x01,               // SMS-SUBMIT，无有效期、无 UDH
        0x00,               // TP-MR
        digits.len() as u8, // 地址长度（数字个数）
        addr_type,
    ];
    tpdu.extend_from_slice(&addr_bytes);
    tpdu.push(0x00); // TP-PID

    if let Some(septets) = string_to_gsm7(text) {
        tpdu.push(0x00); // TP-DCS: 7-bit
        tpdu.push(septets.len() as u8); // UDL = septet 数
        tpdu.extend_from_slice(&pack7(&septets));
    } else {
        let ucs2 = utf16_bytes(text);
        if ucs2.len() > 255 {
            return Err(format!("单条内容过长（{} 字节，上限 255）", ucs2.len()));
        }
        tpdu.push(0x08); // TP-DCS: UCS2
        tpdu.push(ucs2.len() as u8); // UDL = 字节数
        tpdu.extend_from_slice(&ucs2);
    }

    // 最终 PDU = 00（SMSC 长度 0） + TPDU
    let mut full = vec![0x00u8];
    full.extend_from_slice(&tpdu);
    Ok((hex_string(&full), tpdu.len()))
}

/// 解析 `AT+CPMS?` 应答：`+CPMS: "SM",3,30,"SM",3,30,"SM",3,30`。
pub fn parse_storage(resp: &str) -> Option<SmsStorage> {
    let line = resp.lines().find(|l| l.to_uppercase().contains("+CPMS:"))?;
    let up = line.to_uppercase();
    let idx = up.find("+CPMS:")?;
    let f = crate::modem::at::csv(&line[idx + "+CPMS:".len()..]);
    if f.len() < 6 {
        return None;
    }
    let at = |i: usize| f.get(i).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
    let mut st = SmsStorage {
        mem1: f[0].clone(),
        mem1_used: at(1),
        mem1_total: at(2),
        mem2: f[3].clone(),
        mem2_used: at(4),
        mem2_total: at(5),
        current: f[0].to_uppercase(),
        raw: line.trim().to_string(),
        ..Default::default()
    };
    if f.len() >= 9 {
        st.mem3 = f[6].clone();
        st.mem3_used = at(7);
        st.mem3_total = at(8);
    }
    Some(st)
}

/// 解析 `AT+CPMS=?`：`+CPMS: ("ME","MT","SM"),(...)` → 去重后的存储区名。
pub fn parse_cpms_supported(resp: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in resp.lines() {
        let up = line.to_uppercase();
        let Some(i) = up.find("CPMS:") else { continue };
        for tok in line[i..].split('"').skip(1).step_by(2) {
            let m = tok.trim().to_uppercase();
            if m.is_empty() || out.contains(&m) {
                continue;
            }
            out.push(m);
        }
    }
    out
}

// ====================== 文本模式正文还原 ======================

/// 尝试把 UCS2 十六进制内容还原为可读文本。
///
/// 纯数字短信本身就是合法十六进制串，因此三重判据全满足才还原：
/// 长度是 4 的倍数且 ≥ 8、每个码元高字节落在真实文本范围、解出的文本含 CJK。
pub fn decode_sms_text(s: &str) -> String {
    let s = s.trim();
    if s.len() < 8 || s.len() % 4 != 0 || !is_hex_only(s) {
        return s.to_string();
    }
    let mut units = Vec::with_capacity(s.len() / 4);
    for i in (0..s.len()).step_by(4) {
        let Ok(v) = u16::from_str_radix(&s[i..i + 4], 16) else {
            return s.to_string();
        };
        if !ucs2_high_byte_plausible(v) {
            return s.to_string();
        }
        units.push(v);
    }
    let text = String::from_utf16_lossy(&units);
    if !contains_cjk(&text) {
        return s.to_string();
    }
    text
}

/// 码元高字节是否像真实文本（白名单越窄，"纯数字被误当十六进制"的概率越低）。
fn ucs2_high_byte_plausible(sym: u16) -> bool {
    match (sym >> 8) as u8 {
        0x00 | 0x20 | 0xFF => true,
        0x30..=0x33 => true, // CJK 标点 / 假名
        0x4E..=0x9F => true, // 中日韩统一表意文字
        _ => false,
    }
}

/// 是否含中日韩字符。
fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        let r = c as u32;
        matches!(r,
            0x2E80..=0x2EFF
            | 0x3000..=0x303F
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFFEF)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一条 SMS-DELIVER：发件人 13800138000，UCS2 正文「你好」，时间 25/04/29,10:56:44+32。
    const DELIVER_UCS2: &str = "00000B813108108300F0000852409201654423044F60597D";

    #[test]
    fn decode_deliver_ucs2() {
        let (s, ci) = decode_pdu(DELIVER_UCS2).unwrap();
        assert_eq!(s.number, "13800138000");
        assert_eq!(s.timestamp, "25/04/29,10:56:44+32");
        assert_eq!(s.content, "你好");
        assert_eq!(ci.total, 0);
    }

    #[test]
    fn decode_deliver_international_number() {
        // 同一条，改 TON=91（国际号）→ 号码带 +
        let pdu = DELIVER_UCS2.replace("0B8131", "0B9131");
        let (s, _) = decode_pdu(&pdu).unwrap();
        assert_eq!(s.number, "+13800138000");
    }

    #[test]
    fn decode_intl_submit_pdu() {
        // 参考向量：+8613800138000 + UCS2「测试」
        let (s, _) = decode_pdu("0011000D91683108108300F00008AA046D4B8BD5").unwrap();
        assert_eq!(s.number, "+8613800138000");
        assert_eq!(s.content, "测试");
    }

    #[test]
    fn parse_pdu_list_pairs_header_and_body() {
        let resp = format!(
            "+CMGL: 1,0,,159\r\n{DELIVER_UCS2}\r\n+CMGL: 3,1,,159\r\n{DELIVER_UCS2}\r\n\r\nOK\r\n"
        );
        let (list, concat) = parse_pdu_list(&resp);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].index, "1");
        assert_eq!(list[0].status, "0");
        assert_eq!(list[0].content, "你好");
        assert_eq!(list[1].status, "1");
        assert!(concat.is_empty());
    }

    #[test]
    fn pdu_list_skips_urc_lines() {
        let resp = format!("^RSSI: 20\r\n+CMGL: 2,0,,159\r\n{DELIVER_UCS2}\r\n+OK\r\n");
        let (list, _) = parse_pdu_list(&resp);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].index, "2");
        assert_eq!(list[0].content, "你好");
    }

    #[test]
    fn text_mode_list_and_ucs2_content() {
        let resp = "+CMGL: 1,\"REC UNREAD\",\"+8613800138000\",,\"25/04/29,10:56:44+32\"\r\n4F60597D\r\n+CMGL: 2,\"REC READ\",\"10086\",,\"25/04/29,10:57:00+32\"\r\n余额 10 元\r\n\r\nOK\r\n";
        let list = parse_sms_list(resp);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].index, "1");
        assert_eq!(list[0].status, "REC UNREAD");
        assert_eq!(list[0].number, "+8613800138000");
        assert_eq!(list[0].timestamp, "25/04/29,10:56:44+32");
        assert_eq!(list[0].content, "你好");
        assert_eq!(list[1].content, "余额 10 元");
    }

    #[test]
    fn pure_digit_content_is_not_decoded_as_hex() {
        assert_eq!(decode_sms_text("12345678"), "12345678");
        assert_eq!(decode_sms_text("4F60597D"), "你好");
        assert_eq!(decode_sms_text("hi"), "hi");
    }

    #[test]
    fn gsm7_encode_roundtrips_with_decode() {
        // 编解码必须互为逆运算，否则「自己能解的短信自己发不出去」
        for s in ["Hi", "hello world", "A€B", "x[y]z", "£100 @ 50%"] {
            let seps = string_to_gsm7(s).unwrap_or_else(|| panic!("{s} 应可 GSM7 编码"));
            assert_eq!(decode_gsm7(&seps), s, "{s} 往返失败");
        }
    }

    #[test]
    fn gsm7_encode_rejects_non_gsm7_text() {
        // 中文与 emoji 不在默认/扩展表里，必须返回 None 让调用方改走 UCS2
        assert!(string_to_gsm7("你好").is_none());
        assert!(string_to_gsm7("ok😀").is_none());
    }

    #[test]
    fn pack7_is_inverse_of_unpack7bit() {
        let seps: Vec<u8> = (0..160).map(|i| (i % 128) as u8).collect();
        let packed = pack7(&seps);
        assert_eq!(unpack7bit(&packed, seps.len()), seps);
        // 短序列同样要自洽
        let short = vec![0x48u8, 0x49, 0x4A];
        assert_eq!(unpack7bit(&pack7(&short), 3), short);
    }

    #[test]
    fn build_submit_pdu_shape() {
        let (pdu, len) = build_submit_pdu("+8613800138000", "Hi").unwrap();
        // SMSC 长度字节必须是 00
        assert!(pdu.starts_with("00"), "{pdu}");
        // AT+CMGS 要的字节数不含 SMSC 那一个字节
        assert_eq!(pdu.len() / 2, len + 1, "{pdu}");
        // TPDU 头部：01(SMS-SUBMIT) 00(TP-MR) 0D(13 位) 91(国际)
        assert!(pdu.starts_with("0001000D91"), "{pdu}");
    }

    #[test]
    fn build_submit_pdu_uses_ucs2_for_chinese() {
        let (pdu, len) = build_submit_pdu("8613800138000", "测试").unwrap();
        // 国内号码（无 +、非 86 开头时才 0x81；这里 86 开头仍是国际格式）
        let b = hex_to_bytes(&pdu).unwrap();
        // TP-DCS = 0x08
        assert!(b.contains(&0x08), "{pdu}");
        assert_eq!(pdu.len() / 2, len + 1);
        // 中文 4 字节 UCS2 负载
        assert_eq!(b[b.len() - 4..], [0x6D, 0x4B, 0x8B, 0xD5]);
    }

    #[test]
    fn build_submit_pdu_rejects_bad_number() {
        assert!(build_submit_pdu("", "x").is_err());
        assert!(build_submit_pdu("+8613800abc", "x").is_err());
    }

    #[test]
    fn split_for_pdu_respects_real_capacity() {
        // 纯 ASCII：单条 160 字符
        let long: String = "a".repeat(400);
        let parts = split_content_for_pdu(&long);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.iter().map(|p| p.len()).sum::<usize>(), 400);

        // 扩展字符占 2 个 septet：81 个 '€' 就该分成两条
        let euros: String = "€".repeat(81);
        let parts = split_content_for_pdu(&euros);
        assert_eq!(parts.len(), 2);
        assert!(string_to_gsm7(&parts[0]).unwrap().len() <= 160);

        // 中文按 UTF-16 单元切：单条 70 单元
        let zh: String = "中".repeat(200);
        let parts = split_content_for_pdu(&zh);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].chars().count(), 70);

        // emoji 占 2 个单元，不得按 1 个算
        let emo: String = "😀".repeat(40);
        let parts = split_content_for_pdu(&emo);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].encode_utf16().count(), 70);
    }

    #[test]
    fn split_cmgl_fields_keeps_quoted_commas() {
        let f = split_cmgl_fields("\"REC UNREAD\",\"+86,138\",,159");
        assert_eq!(f[0], "REC UNREAD");
        assert_eq!(f[1], "+86,138");
        assert_eq!(f[2], "");
        assert_eq!(f[3], "159");
    }

    #[test]
    fn status_unread_and_counts() {
        for (st, want) in [
            ("REC UNREAD", true),
            ("\"REC UNREAD\"", true),
            ("REC READ", false),
            ("STO SENT", false),
            ("0", true),
            ("\"0\"", true),
            ("1", false),
            ("", false),
            ("   ", false),
            ("?", false),
        ] {
            assert_eq!(sms_status_unread(st), want, "status={st:?}");
        }
        let list = vec![
            Sms {
                status: "0".into(),
                ..Default::default()
            },
            Sms {
                status: "1".into(),
                ..Default::default()
            },
            Sms {
                status: "REC UNREAD".into(),
                ..Default::default()
            },
            Sms::default(),
        ];
        assert_eq!(count_unread(&list), 2);
        assert_eq!(count_unread(&[]), 0);
    }

    #[test]
    fn storage_parse_and_zero_used_kept() {
        let st = parse_storage("+CPMS: \"SM\",3,30,\"SM\",3,30,\"SM\",3,30\r\nOK\r\n").unwrap();
        assert_eq!(
            (st.mem1.as_str(), st.mem1_used, st.mem1_total),
            ("SM", 3, 30)
        );
        assert_eq!(
            (st.mem2.as_str(), st.mem2_used, st.mem2_total),
            ("SM", 3, 30)
        );
        assert_eq!(
            (st.mem3.as_str(), st.mem3_used, st.mem3_total),
            ("SM", 3, 30)
        );
        assert_eq!(st.current, "SM");

        let st2 = parse_storage("+CPMS: \"ME\",1,100,\"ME\",1,100\r\nOK\r\n").unwrap();
        assert_eq!(st2.mem3, "");
        assert_eq!(st2.mem3_total, 0);
        assert!(parse_storage("OK").is_none());

        // 空收件箱：0 值不能被序列化时丢掉
        let st3 = parse_storage("+CPMS: \"SM\",0,40,\"SM\",0,40,\"SM\",0,40\r\nOK\r\n").unwrap();
        let js = serde_json::to_string(&st3).unwrap();
        for want in [
            "\"mem1_used\":0",
            "\"mem2_used\":0",
            "\"mem3_used\":0",
            "\"mem2_total\":40",
            "\"mem3_total\":40",
        ] {
            assert!(js.contains(want), "缺少 {want}: {js}");
        }

        // current 统一大写
        let st4 = parse_storage("+CPMS: \"me\",1,99,\"me\",1,99\r\n").unwrap();
        assert_eq!(st4.current, "ME");
    }

    #[test]
    fn mem_name_validation_and_supported_list() {
        for m in ["SM", "sm", "ME", "me", "MT", " MT "] {
            assert!(valid_mem(m), "{m} 应通过");
        }
        for m in ["", "SIM", "SM1", "SM\",\"ME", "ME MT", "所有"] {
            assert!(!valid_mem(m), "{m} 应被拒绝");
        }
        let sup = parse_cpms_supported(
            "+CPMS: (\"ME\",\"MT\",\"SM\"),(\"ME\",\"MT\",\"SM\"),(\"ME\",\"MT\",\"SM\")\r\nOK\r\n",
        );
        assert_eq!(sup, vec!["ME", "MT", "SM"]);
        assert!(parse_cpms_supported("ERROR\r\nOK\r\n").is_empty());
    }

    #[test]
    fn sort_newest_first_orders_and_keeps_unknown_last() {
        let mk = |ts: &str, idx: &str| Sms {
            timestamp: ts.into(),
            index: idx.into(),
            ..Default::default()
        };
        let list = vec![
            mk("25/04/29,10:56:44+32", "1"),
            mk("", "2"),
            mk("25/04/30,08:00:00+32", "3"),
            mk("25/13/01,00:00:00+32", "4"),
        ];
        let sorted = sort_newest_first(list);
        let idx: Vec<&str> = sorted.iter().map(|s| s.index.as_str()).collect();
        assert_eq!(idx, vec!["3", "1", "2", "4"]);
        assert_eq!(
            time_key("25/04/29,10:56:44+32"),
            Some(
                2025 * 10_000_000_000
                    + 4 * 100_000_000
                    + 29 * 1_000_000
                    + 10 * 10_000
                    + 56 * 100
                    + 44
            )
        );
        assert_eq!(time_key("nope"), None);
    }

    #[test]
    fn group_long_sms_by_reference() {
        let mk = |index: &str, seq: i32, content: &str| Sms {
            index: index.into(),
            status: "0".into(),
            number: "10086".into(),
            timestamp: "25/04/29,10:56:44+32".into(),
            content: content.into(),
            part_seq: seq,
            ..Default::default()
        };
        let list = vec![mk("1", 1, "abc"), mk("2", 2, "def"), mk("9", 9, "solo")];
        let mut concat = HashMap::new();
        concat.insert(
            "1".to_string(),
            ConcatInfo {
                reference: 7,
                total: 2,
                seq: 1,
            },
        );
        concat.insert(
            "2".to_string(),
            ConcatInfo {
                reference: 7,
                total: 2,
                seq: 2,
            },
        );
        let out = group_sms_concat(list, &concat);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "abcdef");
        assert_eq!(out[0].parts.len(), 2);
        assert_eq!(out[0].part_ref, 7);
        assert_eq!(out[0].part_total, 2);
        assert_eq!(out[0].index, "1");
        assert_eq!(out[1].content, "solo");
        assert!(out[1].parts.is_empty());

        // 只有一段不算分条：清子表但保留 part_total/part_seq
        let mut only = HashMap::new();
        only.insert(
            "1".to_string(),
            ConcatInfo {
                reference: 7,
                total: 2,
                seq: 1,
            },
        );
        let out2 = group_sms_concat(vec![mk("1", 1, "abc")], &only);
        assert_eq!(out2.len(), 1);
        assert!(out2[0].parts.is_empty());
        assert_eq!((out2[0].part_total, out2[0].part_seq), (2, 1));
    }

    #[test]
    fn pdu_time_parsing() {
        assert_eq!(
            pdu_time(&[0x52, 0x40, 0x92, 0x01, 0x65, 0x44, 0x23]),
            "25/04/29,10:56:44+32"
        );
        // 负时区（bit3 为符号位）
        assert_eq!(
            pdu_time(&[0x52, 0x40, 0x92, 0x01, 0x65, 0x44, 0x2B]),
            "25/04/29,10:56:44-32"
        );
        // 非 BCD → 空串（按"时间未知"处理）
        assert_eq!(pdu_time(&[0xFF, 0x40, 0x92, 0x01, 0x65, 0x44, 0x23]), "");
        assert_eq!(pdu_time(&[0x00]), "");
    }
}
