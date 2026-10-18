//! IPv4 / IPv6 地址工具（与厂商协议无关的纯函数）。
//!
//! ## 语义边界（改动这里的代价很高，务必看清）
//!
//! * 冒号写法**原样返回**（只做结构校验与 zone id / 方括号剥离），
//!   不做重新压缩 —— 保持与接口上已配置的值逐字节一致。
//! * [`normalize_ipv6`] **拒绝占位地址**：全零（`::`）与纯 `::1`。
//!   实测广和通 MTK 平台在 IPv6 未分配时会回
//!   `0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1`（16 段点分十进制的 `::1` 占位），
//!   必须判为无效，否则会配出一条永远不通的 v6 默认路由。
//! * 点分的 16 段十进制是该平台特有的 IPv6 表示，见
//!   [`decode_dotted_ipv6`] 的实机样例。
//! * **与参考实现的偏差（刻意）**：参考实现的点分 16 段只拦全零与 `::1`；
//!   本模块还拦「前 8 字节全 0」（只上报接口标识、没有前缀）的地址。
//!   实测 FM350-GL 就会这么回，按项目惯例不显示不可信的地址。
//!   两条通路（`AT+CGPADDR` 的 `split_addrs` 与 `AT+CGCONTRDP` 的
//!   `split_addr`）都走这里，判据必须一致 —— 否则同一份固件输出
//!   在拨号态和网络卡片上会显示成两个样。

/// 是否为合法 IPv4 点分十进制。
///
/// 判定：四段、每段纯数字且 <= 255、且不是 `0.0.0.0`。
/// 含 `:` 一律拒绝 —— 上面的 IPv6 会写成 16 段点分十进制，段数不同本就
/// 会被拒绝，但显式排除更直观。
pub fn is_valid_ipv4(s: &str) -> bool {
    if s.is_empty() || s.contains(':') {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let numeric = parts.iter().all(|p| {
        !p.is_empty()
            && p.chars().all(|c| c.is_ascii_digit())
            && p.parse::<u16>().is_ok_and(|n| n <= 255)
    });
    numeric && s != "0.0.0.0"
}

/// 是否为链路本地 IPv6（`fe80::/10`，首组落在 `fe80`~`febf`）。
///
/// 任何 UP 的网卡都会自带一个 link-local 地址，把它计入「有 IPv6」
/// 会制造长期假阳性。
pub fn is_link_local_v6(ip: &str) -> bool {
    match u16::from_str_radix(ip.split(':').next().unwrap_or(""), 16) {
        Ok(v) => v & 0xffc0 == 0xfe80,
        Err(_) => false,
    }
}

/// 统计一段冒号分隔文本中的合法组数（空串 = 0 组；任一组非法返回 `None`）。
///
/// 尾段若还含 `::`，`split(':')` 会产生空组，同样判非法 —— 这正是
/// 「两个 `::`」被拒绝的机制。
///
/// 内嵌 IPv4 已由 [`split_ipv4_tail`] 在调用前拆掉，这里只管纯十六进制组。
fn count_v6_groups(part: &str) -> Option<usize> {
    if part.is_empty() {
        return Some(0);
    }
    let mut n = 0usize;
    for g in part.split(':') {
        if g.is_empty() || g.len() > 4 || !g.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        n += 1;
    }
    Some(n)
}

/// 把段串**末尾**的内嵌 IPv4 拆出来（`::ffff:1.2.3.4` 的 `ffff:1.2.3.4`）。
///
/// 返回 `(去掉 IPv4 后的段串, 该 IPv4 占的 16 位组数)`。
/// 末尾一组不是 IPv4 形式 → 原样返回、附加组数 0；
/// 是 IPv4 形式但本身不合法（`1.2.3.256` / `1.2.3`）→ `None`。
///
/// 之所以要单独拆：内嵌 IPv4 占 **2 个组**，混在 `split(':')` 里数会算成 1 个，
/// 而「`1.2.3.4::5`」这种 IPv4 不在整串末尾的必须拒掉。
fn split_ipv4_tail(seg: &str) -> Option<(&str, usize)> {
    let (head, last) = match seg.rsplit_once(':') {
        Some((h, l)) => (h, l),
        // 整段就一个组（例如 `::1.2.3.4` 的 tail）
        None => ("", seg),
    };
    if !last.contains('.') {
        return Some((seg, 0));
    }
    if !is_valid_ipv4(last) {
        return None;
    }
    Some((head, 2))
}

/// 冒号形式的结构校验：至多一个 `::`，每组 1~4 位十六进制；
/// 无 `::` 时必须恰好 8 组（内嵌 IPv4 算 2 组），有 `::` 时显式组数必须 < 8。
fn well_formed_colon_v6(core: &str) -> bool {
    let mut split = core.splitn(2, "::");
    let head = split.next().unwrap_or("");
    let tail = split.next();

    let Some((head_body, head_extra)) = split_ipv4_tail(head) else {
        return false;
    };
    // IPv4 出现在 head 末尾、而后面还有 tail → 它不在整串末尾，拒
    if head_extra > 0 && tail.is_some() {
        return false;
    }
    let Some((tail_body, tail_extra)) = (match tail {
        Some(t) => split_ipv4_tail(t),
        None => Some(("", 0)),
    }) else {
        return false;
    };

    let Some(h) = count_v6_groups(head_body) else {
        return false;
    };
    match tail {
        None => h + head_extra == 8,
        Some(_) => match count_v6_groups(tail_body) {
            Some(t) => h + t + head_extra + tail_extra < 8,
            None => false,
        },
    }
}

/// 16 段点分十进制形式的 IPv6 解码。
///
/// 实机原始响应：
///
/// ```text
/// AT+CGPADDR=1    -> +CGPADDR: 1,"10.42.4.240","0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1"
/// AT+CGCONTRDP=1  -> +CGCONTRDP: 1,,"cmiot5g","","","32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8",...
/// ```
///
/// 解码示例：`32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8` → `2001:db8:1:2::8`
/// （`36.9` 得 `0x2409`，`128.87` 得 `0x8057`，`32.0` 得 `0x2000`，其余为零）。
///
/// 占位与不可信地址一律返回 `None`，三种都要拦：
///   * 全零；
///   * 仅最低字节为 1（即 `::1`）—— 广和通 MTK 平台 IPv6 未分配时就回这个；
///   * **前 8 字节全 0** —— 实测 FM350-GL 只上报接口标识（没有前缀），
///     按项目惯例不显示不可信的地址。参考实现不拦这条，见模块文档的偏差说明。
fn decode_dotted_ipv6(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 16 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || p.len() > 3 || !p.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let n: u16 = p.parse().ok()?;
        if n > 255 {
            return None;
        }
        bytes[i] = n as u8;
    }
    if bytes.iter().all(|b| *b == 0) {
        return None;
    }
    if bytes[..15].iter().all(|b| *b == 0) && bytes[15] == 1 {
        return None;
    }
    if bytes[..8].iter().all(|b| *b == 0) {
        return None;
    }
    Some(format_ipv6_bytes(&bytes))
}

/// 把 16 字节格式化为标准 IPv6 文本（RFC 5952：小写、去前导零、最长连续零组用 `::` 压缩）。
///
/// 字节数不是 16 时返回空串 —— 调用方按「没有可用地址」处理，
/// 而不是把畸形数据写进接口配置。
pub(crate) fn format_ipv6_bytes(b: &[u8]) -> String {
    if b.len() != 16 {
        return String::new();
    }
    let groups: Vec<u16> = (0..8)
        .map(|i| u16::from_be_bytes([b[i * 2], b[i * 2 + 1]]))
        .collect();
    format_ipv6_groups(&groups)
}

/// 把 8 个 16 位组格式化为标准 IPv6 文本（RFC 5952：压缩最长的连续零段）。
fn format_ipv6_groups(g: &[u16]) -> String {
    let (mut best_start, mut best_len, mut cur_start, mut cur_len) =
        (0usize, 0usize, 0usize, 0usize);
    for (i, v) in g.iter().enumerate() {
        if *v == 0 {
            if cur_len == 0 {
                cur_start = i;
            }
            cur_len += 1;
            if cur_len > best_len {
                best_len = cur_len;
                best_start = cur_start;
            }
        } else {
            cur_len = 0;
        }
    }
    let hex = |s: &[u16]| {
        s.iter()
            .map(|v| format!("{v:x}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    if best_len < 2 {
        return hex(g);
    }
    let (head, tail) = (hex(&g[..best_start]), hex(&g[best_start + best_len..]));
    match (head.is_empty(), tail.is_empty()) {
        (true, true) => "::".to_string(),
        (true, false) => format!("::{tail}"),
        (false, true) => format!("{head}::"),
        (false, false) => format!("{head}::{tail}"),
    }
}

/// 把上报的 IPv6 归一化为可写进接口配置 / 可展示的标准写法；非法或占位返回 `None`。
///
/// 两条通路：
///   * 标准冒号形式 —— 先做**结构校验**，再排除全零占位。宽松实现只要含
///     十六进制字符就采信，`gg::1`、`2001::8::1`（两个 `::`）这类脏值会被
///     原样写进配置；现由 [`well_formed_colon_v6`] 拦下。
///   * 16 段点分十进制形式 —— 走 [`decode_dotted_ipv6`]。
///
/// zone id（`%eth2`）与方括号展示形态（`[2001::1]`）在此剥掉。
pub fn normalize_ipv6(s: &str) -> Option<String> {
    if s.contains(':') {
        let core = s
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split('%')
            .next()
            .unwrap_or("");
        if core.is_empty() || !well_formed_colon_v6(core) {
            return None;
        }
        // 占位地址：全零（`::`）与纯 `::1`（回环）。
        // 模组不会把回环地址当 PDP 地址报上来，见到它就是「没分配到」。
        // 判据用「去掉冒号后去前导零」：`::1` / `::0001` / `0:0:0:0:0:0:0:1`
        // 三种写法都会归一成 `1`。
        let compact: String = core.chars().filter(|c| *c != ':').collect();
        let trimmed = compact.trim_start_matches('0');
        if trimmed.is_empty() || (trimmed == "1" && compact.len() <= 16) {
            return None;
        }
        Some(core.to_string())
    } else {
        decode_dotted_ipv6(s)
    }
}

/// 字符串是否「看起来像」 IPv6（含冒号，或为 16 段点分十进制）。
///
/// 用于 `AT+CGPADDR` 的**地址族判别**：v6 字段位一旦出现过
/// （哪怕是 `::1` 占位），就说明网络侧对 IPv6 表过态 —— 此时绝不能
/// 再用上一轮的残留值兜底。
pub fn looks_v6(s: &str) -> bool {
    let s = s.trim().trim_matches('"');
    if s.contains(':') {
        return true;
    }
    s.split('.').count() == 16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_valid_ipv6(s: &str) -> bool {
        normalize_ipv6(s).is_some()
    }

    #[test]
    fn ipv4_accepts_real_addresses() {
        assert!(is_valid_ipv4("10.42.3.212"));
        assert!(is_valid_ipv4("203.0.113.7"));
    }

    #[test]
    fn ipv4_rejects_dummy_v6_address() {
        // 16 段点分十进制的 IPv6 表示不是 IPv4
        let dummy = "0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1";
        assert!(!is_valid_ipv4(dummy));
        assert!(!is_valid_ipv4("32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8"));
    }

    #[test]
    fn ipv4_rejects_edge_cases() {
        assert!(!is_valid_ipv4(""));
        assert!(!is_valid_ipv4("0.0.0.0"));
        assert!(!is_valid_ipv4("10.5.23"));
        assert!(!is_valid_ipv4("10.42.3.256"));
        assert!(!is_valid_ipv4("10.42.3.a"));
        assert!(!is_valid_ipv4("::1"));
    }

    #[test]
    fn ipv6_accepts_proper_notation() {
        assert!(is_valid_ipv6("2001:db8:1:2::1"));
        assert!(is_valid_ipv6("fe80::1"));
    }

    #[test]
    fn ipv6_rejects_dummy_and_empty() {
        assert!(!is_valid_ipv6(""));
        assert!(!is_valid_ipv6("10.42.3.212"));
        assert!(!is_valid_ipv6("0:0:0:0:0:0:0:0"));
        assert!(!is_valid_ipv6("0000:0000:0000:0000:0000:0000:0000:0000"));
    }

    #[test]
    fn ipv6_decodes_dotted_notation() {
        // 广和通 MTK 平台把 IPv6 按字节展开成 16 段十进制
        assert_eq!(
            normalize_ipv6("32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8").as_deref(),
            Some("2001:db8:1:2::8")
        );
        // 16 段十进制里混进非数字 / 超范围 / 段数不对都要拒
        assert_eq!(
            normalize_ipv6("36.9.128.87.32.0.0.0.0.0.0.0.0.0.0.256"),
            None
        );
        assert_eq!(normalize_ipv6("36.9.128.87.32.0.0.0.0.0.0.0.0.0.0.x"), None);
        assert_eq!(normalize_ipv6("36.9.128.87.32.0.0.0.0.0.0.0.0.0.0"), None);
    }

    #[test]
    fn format_ipv6_bytes_compresses_longest_zero_run() {
        // 2001:db8:1:2::8
        let full: [u8; 16] = [
            0x20, 0x01, 0x0d, 0xb8, 0x00, 0x01, 0x00, 0x02, 0, 0, 0, 0, 0, 0, 0x00, 0x08,
        ];
        assert_eq!(format_ipv6_bytes(&full), "2001:db8:1:2::8");
        // 中段 6 组连续零 → 压成 ::
        let comp: [u8; 16] = [
            0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0x8a, 0x2e, 0x37, 0x0e,
        ];
        assert_eq!(format_ipv6_bytes(&comp), "2001:db8::8a2e:370e");
        // 连续零只有 1 组时不压缩（RFC 5952）
        let one: [u8; 16] = [
            0x20, 0x01, 0, 0, 0x00, 0x01, 0, 2, 0, 0, 0, 0, 0, 0, 0x00, 0x08,
        ];
        assert_eq!(format_ipv6_bytes(&one), "2001:0:1:2::8");
        assert_eq!(format_ipv6_bytes(&[0u8; 16]), "::");
        // 字节数不对 → 空串（调用方按「没有可用地址」处理）
        assert_eq!(format_ipv6_bytes(&[0u8; 8]), "");
    }

    #[test]
    fn ipv6_dotted_strictly_rejects_untrustworthy() {
        // 三种「不能当地址用」的点分 16 段写法，全部要拦
        // ① 全零
        assert_eq!(normalize_ipv6("0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0"), None);
        // ② 仅最低字节为 1 —— 广和通 MTK 平台 IPv6 未分配时就回这个
        assert_eq!(normalize_ipv6("0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1"), None);
        // ③ 只上报接口标识（前 8 字节全 0、没有前缀）—— FM350-GL 实机行为
        assert_eq!(
            normalize_ipv6("0.0.0.0.0.0.0.0.24.217.217.24.12.29.50.199"),
            None
        );
        // 对照：有真实前缀的同一形态要能解出
        assert_eq!(
            normalize_ipv6("32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8").as_deref(),
            Some("2001:db8:1:2::8")
        );
    }

    #[test]
    fn ipv6_dotted_decodes_address_and_gateway() {
        // 实机样例：本机地址与网关都按字节展开
        let addr = "32.1.13.184.0.1.0.2.0.0.0.0.0.0.0.8";
        let gw = "32.1.13.184.0.1.0.2.0.4.0.0.0.0.0.8";
        assert!(is_valid_ipv6(addr));
        assert_eq!(normalize_ipv6(addr).as_deref(), Some("2001:db8:1:2::8"));
        assert_eq!(normalize_ipv6(gw).as_deref(), Some("2001:db8:1:2:4::8"));
    }

    #[test]
    fn ipv6_rejects_dotted_placeholder() {
        let dummy = "0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1";
        assert!(!is_valid_ipv6(dummy));
        assert_eq!(normalize_ipv6(dummy), None);
        assert!(!is_valid_ipv6("0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0"));
        assert!(!is_valid_ipv6("1.2.3.4"));
        assert!(!is_valid_ipv6("36.9.128.87.32.0.0.0.0.0.0.0.0.0.0.256"));
    }

    #[test]
    fn ipv6_colon_structure_enforced() {
        assert_eq!(normalize_ipv6("gg::1"), None);
        assert_eq!(normalize_ipv6("2001::8::1"), None); // 两个 ::
        assert_eq!(normalize_ipv6("2001:db8:1:2:4:0:0:0:8:1"), None); // 9 组
        assert_eq!(normalize_ipv6("fe80:1:2:3:4:5:6"), None); // 7 组缺 ::
        assert_eq!(normalize_ipv6(":2001::1"), None); // 头部空组
        assert_eq!(normalize_ipv6("2001:db8:1:2:4:0:0:0:80000"), None); // 组过长
                                                                        // zone id 与方括号展示形态
        assert_eq!(normalize_ipv6("fe80::1%eth2").as_deref(), Some("fe80::1"));
        assert_eq!(
            normalize_ipv6("[2001:db8::8]").as_deref(),
            Some("2001:db8::8")
        );
        // 恰好 8 组、无 :: —— 原样返回，不重新压缩
        assert_eq!(
            normalize_ipv6("2001:db8:1:2:0:0:0:8").as_deref(),
            Some("2001:db8:1:2:0:0:0:8")
        );
    }

    #[test]
    fn ipv6_accepts_embedded_ipv4() {
        // 内嵌 IPv4 是合法形态（占 2 个 16 位组），不能当成脏值拒掉
        assert_eq!(
            normalize_ipv6("::ffff:1.2.3.4").as_deref(),
            Some("::ffff:1.2.3.4")
        );
        assert_eq!(
            normalize_ipv6("2001:db8::10.0.0.1").as_deref(),
            Some("2001:db8::10.0.0.1")
        );
        // 但内嵌的 IPv4 本身不合法时仍要拒
        assert_eq!(normalize_ipv6("::ffff:1.2.3.256"), None);
        assert_eq!(normalize_ipv6("::ffff:1.2.3"), None);
        assert_eq!(normalize_ipv6("1.2.3.4::5"), None); // IPv4 不在末组
    }

    #[test]
    fn link_local_detection() {
        assert!(is_link_local_v6("fe80::1"));
        assert!(is_link_local_v6("FE80::200:11ff:fe12:1314"));
        assert!(is_link_local_v6("febf::1"));
        assert!(!is_link_local_v6("2001:db8:1:2::1"));
        assert!(!is_link_local_v6("::1"));
        assert!(!is_link_local_v6(""));
    }

    #[test]
    fn looks_v6_discriminates_address_family() {
        assert!(looks_v6("2001:db8::8"));
        // 占位也算 v6 位出现过
        assert!(looks_v6("0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1"));
        assert!(!looks_v6("10.42.3.212"));
        assert!(!looks_v6(""));
    }
}
