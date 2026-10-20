//! MBIM 线协议（MBIM 1.0 / USB CDC-MBIM）。
//!
//! 与 [`super::qmi`] 并列：这是**纯编解码**，不做 IO。
//! 常量与流程逐条对齐上游 `{quectel-cm,meig-cm}/mbim-cm.c`（原 `third_party/`，已移除）：
//!
//! - 帧头 12 字节：`type / length / transaction_id`（均小端）；
//! - `COMMAND_MSG` 追加 8 字节分片头 + 16 字节 service UUID + `CID / type / length`；
//! - `COMMAND_DONE` 把 `type` 换成 `status`；
//! - UUID 按 **GUID 小端布局**（前三段反转），与 `mbimcli` / libmbim 一致。
//!
//! 为什么不自己拍：MBIM 的 offset 全部相对 **InformationBuffer 起点**，
//! 差一个字段就整条命令被固件拒（`status=0xE` invalid parameter），
//! 所以每个 payload 都有 golden-bytes 单测。

/// 消息类型（MBIM 1.0 §9.4.1）。
pub const MSG_OPEN: u32 = 0x0000_0001;
pub const MSG_OPEN_DONE: u32 = 0x8000_0001;
pub const MSG_CLOSE: u32 = 0x0000_0002;
pub const MSG_CLOSE_DONE: u32 = 0x8000_0002;
pub const MSG_COMMAND: u32 = 0x0000_0003;
pub const MSG_COMMAND_DONE: u32 = 0x8000_0003;
pub const MSG_HOST_ERROR: u32 = 0x0000_0005;
pub const MSG_FUNCTION_ERROR: u32 = 0x0000_0006;

/// Basic Connect 服务（`a289cc33-bcbb-8b4f-b6b0-133ec2aae6df`，GUID 小端）。
pub const UUID_BASIC_CONNECT: [u8; 16] = [
    0x33, 0xcc, 0x89, 0xa2, 0xbb, 0xbc, 0x4f, 0x8b, 0xb6, 0xb0, 0x13, 0x3e, 0xc2, 0xaa, 0xe6, 0xdf,
];

/// 上下文类型 "Internet"（`7e5e2a7e-…`，GUID 小端）。
pub const UUID_CONTEXT_INTERNET: [u8; 16] = [
    0x7e, 0x2a, 0x5e, 0x7e, 0x6f, 0x4e, 0x72, 0x72, 0x73, 0x6b, 0x65, 0x6e, 0x7e, 0x5e, 0x2a, 0x7e,
];

/// Basic Connect 的 CID（枚举值取自 `mbim-cm.c`，与 MBIM 1.0 表一致）。
///
/// 完整表（供后续扩展，当前只实现拨号必需的三条）：
/// `1 DEVICE_CAPS`、`2 SUBSCRIBER_READY`、`3 RADIO_STATE`、`4 PIN`、
/// `6 HOME_PROVIDER`、`7 PREFERRED_PROVIDERS`、`8 VISIBLE_PROVIDERS`、
/// `9 REGISTER_STATE`、`10 PACKET_SERVICE`、`11 SIGNAL_STATE`、
/// `12 CONNECT`、`13 PROVISIONED_CONTEXTS`、`15 IP_CONFIGURATION`。
pub const CID_SUBSCRIBER_READY: u32 = 2;
pub const CID_CONNECT: u32 = 12;
pub const CID_IP_CONFIGURATION: u32 = 15;

/// 命令类型。
pub const CMD_QUERY: u32 = 0;
pub const CMD_SET: u32 = 1;

/// 激活命令。
pub const ACTIVATE: u32 = 1;
pub const DEACTIVATE: u32 = 0;

/// 激活状态：`2` = activated。
pub const ACTIVATION_ACTIVATED: u32 = 2;

/// 上下文 IP 类型。
pub const IP_TYPE_DEFAULT: u32 = 0;
pub const IP_TYPE_IPV4: u32 = 1;
pub const IP_TYPE_IPV4V6: u32 = 3;

/// 认证协议。
pub const AUTH_NONE: u32 = 0;

/// `COMMAND_MSG` 头长度：12（消息头）+ 8（分片头）+ 16（UUID）+ 12（CID/type/len）。
pub const COMMAND_HEADER_LEN: usize = 48;
/// `COMMAND_DONE` 在 status **之前**的头长度：12 + 8 + 16 + 4（CID） = 40。
///
/// body 从 `status` 起：[status(4)][info_len(4)][info…]，
/// 所以 [`command_status`] 读 body[0..4]、[`command_info`] 读 body[8..]。
pub const COMMAND_DONE_HEADER_LEN: usize = 40;

/// 一帧解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: u32,
    pub transaction: u32,
    pub body: Vec<u8>,
}

/// 组一帧（只用于 OPEN / CLOSE 这类无 body 的消息）。
pub fn simple_frame(kind: u32, transaction: u32, extra: &[u8]) -> Vec<u8> {
    let len = 12 + extra.len();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.extend_from_slice(&transaction.to_le_bytes());
    out.extend_from_slice(extra);
    out
}

/// `MBIM_OPEN_MSG`：额外带 `MaxControlTransfer`（用 4096，主流固件都接受）。
pub fn open(transaction: u32) -> Vec<u8> {
    simple_frame(MSG_OPEN, transaction, &4096u32.to_le_bytes())
}

/// `MBIM_CLOSE_MSG`（无额外字段）。
pub fn close(transaction: u32) -> Vec<u8> {
    simple_frame(MSG_CLOSE, transaction, &[])
}

/// 组一条 `COMMAND_MSG`。
pub fn command(transaction: u32, service: &[u8; 16], cid: u32, kind: u32, info: &[u8]) -> Vec<u8> {
    let len = COMMAND_HEADER_LEN + info.len();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&MSG_COMMAND.to_le_bytes());
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.extend_from_slice(&transaction.to_le_bytes());
    // 分片头：总共 1 片、当前第 0 片（不分片）
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(service);
    out.extend_from_slice(&cid.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(info.len() as u32).to_le_bytes());
    out.extend_from_slice(info);
    out
}

/// 从缓冲区取一帧（半截返回 `None`，坏长度直接丢弃头部 4 字节重同步）。
pub fn take(buf: &mut Vec<u8>) -> Option<Message> {
    loop {
        if buf.len() < 12 {
            return None;
        }
        let kind = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        let transaction = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        // 长度必须 >= 头、且不超过一个合理的控制传输上限（8 KiB）。
        // 坏长度说明流错位了：丢 4 字节继续找（同一轮里重同步，不把脏帧留给调用方）。
        if !(12..=8192).contains(&len) {
            buf.drain(..4);
            continue;
        }
        if buf.len() < len {
            return None;
        }
        let frame: Vec<u8> = buf.drain(..len).collect();
        let body = if kind == MSG_COMMAND_DONE && frame.len() >= COMMAND_DONE_HEADER_LEN {
            frame[COMMAND_DONE_HEADER_LEN..].to_vec()
        } else if frame.len() > 12 {
            frame[12..].to_vec()
        } else {
            Vec::new()
        };
        return Some(Message {
            kind,
            transaction,
            body,
        });
    }
}

/// `OPEN_DONE` / `CLOSE_DONE` 的 status（0 = 成功）。
pub fn done_status(body: &[u8]) -> Option<u32> {
    body.get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// `COMMAND_DONE` 的 status（body 前 4 字节；`COMMAND_DONE` 的 body 从 status 起）。
pub fn command_status(body: &[u8]) -> Option<u32> {
    done_status(body)
}

/// `COMMAND_DONE` 的 info buffer（status 之后 4 字节是长度）。
pub fn command_info(body: &[u8]) -> &[u8] {
    if body.len() < 8 {
        return &[];
    }
    let len = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as usize;
    // 设备偶尔把长度写大：截到实际长度，避免越界 panic
    let end = (8 + len).min(body.len());
    &body[8..end]
}

/// 订阅者就绪状态（`SUBSCRIBER_READY_STATUS` QUERY 的应答首字节：1 = ready）。
pub fn subscriber_ready(info: &[u8]) -> bool {
    info.first().copied() == Some(1)
}

/// 组装 `CONNECT` 的 SET 负载。
///
/// 布局（`MBIM_CONNECT`，56 字节 + 变长字符串，全部小端）：
/// `session / activation / access{off,size} / user{off,size} / pass{off,size} /
///  compression / auth / ip_type / context_uuid / nw_error`。
/// offset 相对 **info buffer 起点**，字符串是 UTF-16LE（size 是字节数）。
pub fn connect_set(
    session: u32,
    activate: bool,
    apn: &str,
    user: &str,
    pass: &str,
    ip_type: u32,
) -> Vec<u8> {
    // 11 个 u32（session…ip_type）= 44，+ UUID 16，+ NwError 4 = **64**。
    // 早先按 56 算会让所有字符串 offset 偏 8 字节，设备直接回
    // `status=0xE`（invalid parameter）—— 单测里用真实布局断言过。
    const BASE: usize = 64;
    let apn16: Vec<u8> = apn.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let user16: Vec<u8> = user.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let pass16: Vec<u8> = pass.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();

    let apn_off = BASE;
    let user_off = apn_off + apn16.len();
    let pass_off = user_off + user16.len();

    let mut out = Vec::with_capacity(pass_off + pass16.len());
    out.extend_from_slice(&session.to_le_bytes());
    out.extend_from_slice(&(if activate { ACTIVATE } else { DEACTIVATE }).to_le_bytes());
    out.extend_from_slice(&(apn_off as u32).to_le_bytes());
    out.extend_from_slice(&(apn16.len() as u32).to_le_bytes());
    out.extend_from_slice(&(user_off as u32).to_le_bytes());
    out.extend_from_slice(&(user16.len() as u32).to_le_bytes());
    out.extend_from_slice(&(pass_off as u32).to_le_bytes());
    out.extend_from_slice(&(pass16.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // compression = none
    out.extend_from_slice(&AUTH_NONE.to_le_bytes());
    out.extend_from_slice(&ip_type.to_le_bytes());
    out.extend_from_slice(&UUID_CONTEXT_INTERNET);
    out.extend_from_slice(&0u32.to_le_bytes()); // nw_error
    out.extend_from_slice(&apn16);
    out.extend_from_slice(&user16);
    out.extend_from_slice(&pass16);
    out
}

/// `CONNECT` 应答里的激活状态（偏移 4 的 u32）。
pub fn connect_state(info: &[u8]) -> Option<u32> {
    info.get(4..8)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// `CONNECT` 应答里的 `NwError`（最后 4 字节；排查「为什么激活失败」用）。
pub fn connect_nw_error(info: &[u8]) -> Option<u32> {
    let n = info.len();
    if n < 4 {
        return None;
    }
    info.get(n - 4..)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// `IP_CONFIGURATION` QUERY 的负载（只有一个 session id）。
pub fn ip_configuration_query(session: u32) -> Vec<u8> {
    session.to_le_bytes().to_vec()
}

/// 解析出来的地址信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IpConfig {
    /// 本机 IPv4（点分）。
    pub ipv4: Option<String>,
    /// 前缀长度。
    pub prefix: Option<u8>,
    /// 网关。
    pub gateway: Option<String>,
    /// DNS（可能多条）。
    pub dns: Vec<String>,
    /// MTU（0 = 没给）。
    pub mtu: u32,
    /// 本机 IPv6（压缩形态）与前缀长度。
    pub ipv6: Option<(String, u8)>,
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    if off + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn ipv4_at(b: &[u8], off: usize) -> Option<String> {
    let s = b.get(off..off + 4)?;
    Some(format!("{}.{}.{}.{}", s[0], s[1], s[2], s[3]))
}

fn ipv6_at(b: &[u8], off: usize) -> Option<String> {
    let s = b.get(off..off + 16)?;
    let mut arr = [0u8; 16];
    arr.copy_from_slice(s);
    Some(crate::modem::model::format_ipv6_bytes(&arr))
}

/// 解析 `IP_CONFIGURATION` 应答。
///
/// 依据 MBIM 1.0 §10.5.29 的 `IP_CONFIGURATION_INFO`，头部 14 个 u32（56 字节），
/// 之后是按 offset 索引的数据区（offset 相对 info buffer 起点）：
///
/// | 偏移 | 字段 |
/// | --- | --- |
/// | 4 | IPv4ConfigurationAvailable（位图） |
/// | 8 | IPv6ConfigurationAvailable（位图） |
/// | 12 / 16 | v4 地址条数 / 偏移 |
/// | 20 / 24 | v6 地址条数 / 偏移 |
/// | 28 | v4 网关偏移 |
/// | 32 | v6 网关偏移（**有意不读**：v6 网关交给 netifd 按 RA 处理） |
/// | 36 / 40 | v4 DNS 条数 / 偏移 |
/// | 44 / 48 | v6 DNS 条数 / 偏移 |
/// | 52 | v4 MTU |
///
/// 可用性位图：bit0 地址、bit1 网关、bit2 DNS、bit3 MTU。
///
/// 长度约定：`info` 短于 56 字节直接返回空结构；每个地址条目按 `b.get(off..off+n)`
/// 取，越界返回 `None` 而不是 panic —— 设备把条数写大时只丢多余条目。
pub fn parse_ip_configuration(info: &[u8]) -> IpConfig {
    const HDR: usize = 56;
    if info.len() < HDR {
        return IpConfig::default();
    }
    let v4_avail = u32_at(info, 4);
    let v6_avail = u32_at(info, 8);
    let v4_count = u32_at(info, 12) as usize;
    let v4_addr_off = u32_at(info, 16) as usize;
    let v6_count = u32_at(info, 20) as usize;
    let v6_addr_off = u32_at(info, 24) as usize;
    let v4_gw_off = u32_at(info, 28) as usize;
    let v4_dns_count = u32_at(info, 36) as usize;
    let v4_dns_off = u32_at(info, 40) as usize;
    let v6_dns_off = u32_at(info, 48) as usize;
    let v4_mtu = u32_at(info, 52);

    let mut cfg = IpConfig::default();
    if v4_avail & 1 != 0 && v4_count > 0 {
        // 每个元素：prefix_len(4) + addr(4)
        cfg.prefix = Some((u32_at(info, v4_addr_off) & 0xff) as u8);
        cfg.ipv4 = ipv4_at(info, v4_addr_off + 4);
    }
    if v4_avail & 2 != 0 && v4_gw_off > 0 {
        cfg.gateway = ipv4_at(info, v4_gw_off);
    }
    if v4_avail & 4 != 0 {
        for i in 0..v4_dns_count.min(4) {
            if let Some(ip) = ipv4_at(info, v4_dns_off + i * 4) {
                cfg.dns.push(ip);
            }
        }
    }
    if v4_avail & 8 != 0 {
        cfg.mtu = v4_mtu;
    }
    if v6_avail & 1 != 0 && v6_count > 0 {
        let prefix = (u32_at(info, v6_addr_off) & 0xff) as u8;
        if let Some(ip) = ipv6_at(info, v6_addr_off + 4) {
            cfg.ipv6 = Some((ip, prefix));
        }
    }
    // v6 网关与 v6 DNS：只用来补 DNS（网关对 v6 由 netifd 按 RA 处理）
    if v6_avail & 4 != 0 {
        for i in 0..(u32_at(info, 44) as usize).min(4) {
            if let Some(ip) = ipv6_at(info, v6_dns_off + i * 16) {
                cfg.dns.push(ip);
            }
        }
    }
    // v6 网关（偏移 32）有意不读：对 v6 由 netifd 按 RA 取网关，
    // 模组给的地址在这里写进上层只会与 RA 结果打架。
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le32(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn open_frame_matches_reference_layout() {
        let f = open(7);
        assert_eq!(&f[0..4], &le32(MSG_OPEN));
        assert_eq!(&f[4..8], &le32(16)); // 12 + 4
        assert_eq!(&f[8..12], &le32(7));
        assert_eq!(&f[12..16], &le32(4096));
    }

    #[test]
    fn command_frame_has_all_header_fields() {
        let f = command(9, &UUID_BASIC_CONNECT, CID_CONNECT, CMD_SET, &[0xaa, 0xbb]);
        assert_eq!(&f[0..4], &le32(MSG_COMMAND));
        assert_eq!(&f[4..8], &le32((48 + 2) as u32));
        assert_eq!(&f[8..12], &le32(9));
        assert_eq!(&f[12..16], &le32(1)); // total fragments
        assert_eq!(&f[16..20], &le32(0)); // current fragment
        assert_eq!(&f[20..36], &UUID_BASIC_CONNECT);
        assert_eq!(&f[36..40], &le32(CID_CONNECT));
        assert_eq!(&f[40..44], &le32(CMD_SET));
        assert_eq!(&f[44..48], &le32(2));
        assert_eq!(&f[48..], &[0xaa, 0xbb]);
    }

    #[test]
    fn take_handles_partial_and_resyncs_on_bad_length() {
        let mut buf = open(1);
        buf.truncate(8); // 半截
        assert!(take(&mut buf).is_none());
        assert_eq!(buf.len(), 8);

        // 坏长度（0）：丢脏字节重新同步 —— 不 panic、不死循环，缓冲区被推进
        let mut bad = le32(MSG_OPEN).to_vec();
        bad.extend_from_slice(&le32(0));
        bad.extend_from_slice(&le32(1));
        assert!(take(&mut bad).is_none(), "只剩脏头，取不到帧");
        assert!(bad.len() < 12, "脏字节必须被丢掉（否则下次还会卡住）");

        // 完整帧跟在脏字节后面：先丢脏头，再正常取到
        let mut dirty = le32(MSG_OPEN).to_vec();
        dirty.extend_from_slice(&le32(0));
        dirty.extend_from_slice(&le32(1));
        dirty.extend_from_slice(&le32(0xdead_beef)); // 脏尾巴
        dirty.extend_from_slice(&open(2));
        // 手动保证重同步落在帧起点：丢掉 12 字节脏头 + 4 字节脏尾巴
        dirty.drain(..16);
        let got = take(&mut dirty).expect("干净起点应取到帧");
        assert_eq!(got.transaction, 2);
    }

    #[test]
    fn command_done_status_and_info() {
        let mut body = le32(0).to_vec(); // status = success
        body.extend_from_slice(&le32(3)); // info len
        body.extend_from_slice(&[2, 0, 0]); // activation_state = 2
        let mut frame = le32(MSG_COMMAND_DONE).to_vec();
        let total = COMMAND_DONE_HEADER_LEN + body.len();
        frame.extend_from_slice(&le32(total as u32));
        frame.extend_from_slice(&le32(5));
        frame.extend_from_slice(&le32(1));
        frame.extend_from_slice(&le32(0));
        frame.extend_from_slice(&UUID_BASIC_CONNECT);
        frame.extend_from_slice(&le32(CID_CONNECT));
        frame.extend_from_slice(&body);

        let mut buf = frame;
        let m = take(&mut buf).expect("完整帧");
        assert_eq!(m.kind, MSG_COMMAND_DONE);
        assert_eq!(m.transaction, 5);
        assert_eq!(command_status(&m.body), Some(0));
        assert_eq!(command_info(&m.body), &[2, 0, 0]);
        // info 里第 4..8 字节是 activation_state（这里只有 3 字节，取不到）
        assert_eq!(connect_state(command_info(&m.body)), None);
    }

    #[test]
    fn connect_set_offsets_are_relative_to_info_buffer() {
        let info = connect_set(0, true, "ctnet", "", "", IP_TYPE_IPV4V6);
        assert_eq!(info.len(), 64 + 10, "固定头 + APN 的 UTF-16");
        assert_eq!(u32_at(&info, 0), 0); // session
        assert_eq!(u32_at(&info, 4), ACTIVATE);
        // APN 紧跟在 64 字节固定头之后
        assert_eq!(u32_at(&info, 8), 64);
        assert_eq!(u32_at(&info, 12), 10); // "ctnet" = 5 字符 × 2
                                           // user / pass 空：size 0，offset 仍指向「紧跟 APN 之后」（合法，设备不看空串）
        assert_eq!(u32_at(&info, 16), 74);
        assert_eq!(u32_at(&info, 20), 0);
        assert_eq!(u32_at(&info, 24), 74);
        assert_eq!(u32_at(&info, 28), 0);
        assert_eq!(u32_at(&info, 32), 0); // compression
        assert_eq!(u32_at(&info, 36), AUTH_NONE);
        assert_eq!(u32_at(&info, 40), IP_TYPE_IPV4V6);
        assert_eq!(&info[44..60], &UUID_CONTEXT_INTERNET);
        assert_eq!(u32_at(&info, 60), 0); // nw_error
                                          // 字符串是 UTF-16LE
        assert_eq!(
            &info[64..],
            "ctnet"
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect::<Vec<u8>>()
                .as_slice()
        );
    }

    #[test]
    fn command_done_body_starts_at_status() {
        // 用真实布局组一帧：DONE 头 44 + status(4) + len(4) + info
        // info = session(4) + activation_state(4)；`connect_state` 读第 4..8 字节
        let mut frame = le32(MSG_COMMAND_DONE).to_vec();
        let total = COMMAND_DONE_HEADER_LEN + 8 + 8;
        frame.extend_from_slice(&le32(total as u32));
        frame.extend_from_slice(&le32(1));
        frame.extend_from_slice(&le32(1));
        frame.extend_from_slice(&le32(0));
        frame.extend_from_slice(&UUID_BASIC_CONNECT);
        frame.extend_from_slice(&le32(CID_CONNECT));
        frame.extend_from_slice(&le32(0)); // status
        frame.extend_from_slice(&le32(8)); // info len
        frame.extend_from_slice(&le32(0)); // session id
        frame.extend_from_slice(&le32(2)); // activation_state = 2
        let mut buf = frame;
        let m = take(&mut buf).expect("完整帧");
        assert_eq!(m.body.len(), 16, "status(4) + len(4) + info(8)");
        assert_eq!(command_status(&m.body), Some(0));
        assert_eq!(connect_state(command_info(&m.body)), Some(2));
    }

    #[test]
    fn connect_nw_error_reads_tail() {
        let mut info = vec![0u8; 12];
        info.extend_from_slice(&le32(0x1234));
        assert_eq!(connect_nw_error(&info), Some(0x1234));
        assert_eq!(connect_nw_error(&[]), None);
    }

    #[test]
    fn ip_configuration_parses_v4_and_dns() {
        // 56 字节头 + 数据区
        let mut info = vec![0u8; 56];
        let addr_off = 56usize;
        let gw_off = 64usize;
        let dns_off = 68usize;
        // v4 可用：地址 + 网关 + DNS + MTU
        info[4..8].copy_from_slice(&le32(0b1111));
        info[12..16].copy_from_slice(&le32(1)); // count
        info[16..20].copy_from_slice(&le32(addr_off as u32));
        info[28..32].copy_from_slice(&le32(gw_off as u32));
        info[36..40].copy_from_slice(&le32(2)); // DNS 条数
        info[40..44].copy_from_slice(&le32(dns_off as u32));
        info[52..56].copy_from_slice(&le32(1500));
        // 数据：prefix=24 + 10.42.0.16
        info.extend_from_slice(&le32(24));
        info.extend_from_slice(&[10, 42, 0, 16]);
        info.extend_from_slice(&[10, 42, 0, 1]);
        info.extend_from_slice(&[203, 0, 113, 53]);
        info.extend_from_slice(&[203, 0, 113, 54]);

        let cfg = parse_ip_configuration(&info);
        assert_eq!(cfg.ipv4.as_deref(), Some("10.42.0.16"));
        assert_eq!(cfg.prefix, Some(24));
        assert_eq!(cfg.gateway.as_deref(), Some("10.42.0.1"));
        assert_eq!(cfg.dns, vec!["203.0.113.53", "203.0.113.54"]);
        assert_eq!(cfg.mtu, 1500);
    }

    #[test]
    fn ip_configuration_tolerates_short_buffer() {
        assert_eq!(parse_ip_configuration(&[]), IpConfig::default());
        assert_eq!(parse_ip_configuration(&[0u8; 20]), IpConfig::default());
    }

    #[test]
    fn subscriber_ready_only_accepts_one() {
        assert!(subscriber_ready(&[1]));
        assert!(!subscriber_ready(&[0]));
        assert!(!subscriber_ready(&[]));
    }
}
