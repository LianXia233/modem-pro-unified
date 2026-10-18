//! QMI 线协议（QMUX over `/dev/cdc-wdm`）：分帧、TLV、WDS 数据呼叫。
//!
//! ## 口径来源
//!
//! 与 `files/third_party/quectel-cm` 的 `QCQMUX.h` / `QMIThread.c` 逐条对齐：
//! 帧头布局、TLV 编号、IP family 取值都照那份实现抄，避免「自己发明一套、
//! 到真机上全被拒」。关键编号：
//!
//! | 用途 | service | message |
//! | --- | --- | --- |
//! | CTL Sync | `0x00` | `0x0027` |
//! | CTL Get Client ID | `0x00` | `0x0022` |
//! | WDS Start Network Interface | `0x01`（IPv6 用 `0x11`） | `0x0020` |
//! | WDS Stop Network Interface | 同上 | `0x0021` |
//! | WDS Get Runtime Settings | 同上 | `0x002D` |
//!
//! ## 帧格式（全部小端）
//!
//! ```text
//! [0]      0x01                     IFType = QMUX
//! [1..3]   len = 整帧长度 - 1        （与 libqmi / quectel-CM 一致：长度从 IFType 之后算起）
//! [3]      ctl flags = 0x00
//! [4]      service
//! [5]      client id
//! [6]      qmux flags = 0x00（单条命令）
//! [7..9]   transaction id
//! [9..11]  message id
//! [11..13] payload 长度
//! [13..]   TLV 串（请求）/ 结果码 + TLV（应答）
//! ```
//!
//! ## 边界
//!
//! 本文件**不碰文件系统**（编解码 + 纯解析），设备 IO 在 [`super::native`]；
//! 所有解析都做长度检查，坏帧返回 `None` / 跳过，不 panic。

use std::net::{Ipv4Addr, Ipv6Addr};

/// QMUX 的 IFType。
pub const IF_TYPE_QMUX: u8 = 0x01;

/// 控制服务。
pub const SVC_CTL: u8 = 0x00;
/// WDS（IPv4 / 通用数据服务）。
pub const SVC_WDS: u8 = 0x01;
/// WDS IPv6（与 WDS 分开一个 client，和 quectel-CM 同口径）。
pub const SVC_WDS_IPV6: u8 = 0x11;

/// CTL Sync（拿回 client id 之前必须先同步）。
pub const CTL_SYNC: u16 = 0x0027;
/// CTL Get Client ID。
pub const CTL_GET_CLIENT_ID: u16 = 0x0022;
/// CTL Release Client ID（长驻进程必须释放，否则反复重拨会把 client id 用光）。
pub const CTL_RELEASE_CLIENT_ID: u16 = 0x0023;
/// WDS Start Network Interface。
pub const WDS_START_NETWORK: u16 = 0x0020;
/// `WDS Set Autoconnect`：TLV 0x01 = 1 字节（0 关 / 1 开）。
///
/// quectel-cm / fibocom-dial 的 `QmiWwanInit()` 都会先关掉它：
/// 不关的话固件可能在 CTL Sync 之后自己建一条默认承载，我们的 Start 会被拒
/// （`result=0x01, error=0x11` "call already present"）。
pub fn wds_set_autoconnect(transaction: u16, client: u8, enable: bool) -> Vec<u8> {
    let payload = tlv(0x01, &[u8::from(enable)]);
    encode(SVC_WDS, client, transaction, WDS_SET_AUTOCONNECT, &payload)
}

/// WDS Stop Network Interface。
pub const WDS_STOP_NETWORK: u16 = 0x0021;
/// WDS Get Runtime Settings。
pub const WDS_GET_RUNTIME_SETTINGS: u16 = 0x002D;
/// WDS Set Autoconnect（关掉固件的自动拨号，防它抢先建默认承载）。
pub const WDS_SET_AUTOCONNECT: u16 = 0x0026;
/// WDS Get Packet Service Status（探活）。
pub const WDS_GET_PKT_STATUS: u16 = 0x0022;

/// IP family 取值（`IpFamilyV4 = 0x04`、`IpFamilyV6 = 0x06`）。
pub const IP_FAMILY_V4: u8 = 0x04;
pub const IP_FAMILY_V6: u8 = 0x06;

/// 一帧解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub service: u8,
    pub client: u8,
    pub transaction: u16,
    pub message: u16,
    pub payload: Vec<u8>,
}

/// 组一帧 QMUX 命令。
pub fn encode(service: u8, client: u8, transaction: u16, message: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(13 + payload.len());
    out.push(IF_TYPE_QMUX);
    let len = 12 + payload.len() as u16; // 整帧长度 - 1
    out.extend_from_slice(&len.to_le_bytes());
    out.push(0x00); // QMI ctl flags（保留）
    out.push(service);
    out.push(client);
    out.push(0x00); // QMUX flags：单条命令
    out.extend_from_slice(&transaction.to_le_bytes());
    out.extend_from_slice(&message.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// 从缓冲区头部取一帧（不够一帧就返回 `None`，不改动缓冲区）。
pub fn take(buf: &mut Vec<u8>) -> Option<Message> {
    if buf.len() < 3 || buf[0] != IF_TYPE_QMUX {
        // 不是 QMUX（或半截头）：没法恢复，丢掉 1 字节让调用方继续找
        if !buf.is_empty() {
            buf.remove(0);
        }
        return None;
    }
    let len = u16::from_le_bytes([buf[1], buf[2]]) as usize;
    let total = len + 1;
    if total < 13 || buf.len() < total {
        return None;
    }
    let service = buf[4];
    let client = buf[5];
    let transaction = u16::from_le_bytes([buf[7], buf[8]]);
    let message = u16::from_le_bytes([buf[9], buf[10]]);
    let plen = u16::from_le_bytes([buf[11], buf[12]]) as usize;
    if 13 + plen > total {
        buf.drain(..total);
        return None; // 长度自相矛盾，丢整帧
    }
    let payload = buf[13..13 + plen].to_vec();
    buf.drain(..total);
    Some(Message {
        service,
        client,
        transaction,
        message,
        payload,
    })
}

/// 组一个 TLV。
pub fn tlv(kind: u8, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + value.len());
    out.push(kind);
    out.extend_from_slice(&(value.len() as u16).to_le_bytes());
    out.extend_from_slice(value);
    out
}

/// 遍历 TLV（`skip_result = true` 时跳过开头的 0x02 结果码）。
pub fn tlvs(payload: &[u8], skip_result: bool) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut first = true;
    while i + 3 <= payload.len() {
        let kind = payload[i];
        let len = u16::from_le_bytes([payload[i + 1], payload[i + 2]]) as usize;
        let start = i + 3;
        let end = start + len;
        if end > payload.len() {
            break; // 半截 TLV：丢尾巴
        }
        if !(first && skip_result && kind == 0x02) {
            out.push((kind, &payload[start..end]));
        }
        first = false;
        i = end;
    }
    out
}

/// 取一个 TLV 的值（`skip_result = true` 时跳过结果码）。
pub fn find_tlv(payload: &[u8], kind: u8, skip_result: bool) -> Option<&[u8]> {
    tlvs(payload, skip_result)
        .into_iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, v)| v)
}

/// 读结果码：`(result, error)`；没有结果码 TLV 视为成功（有些固件省略）。
pub fn result(payload: &[u8]) -> Result<(), (u16, u16)> {
    let Some(v) = find_tlv(payload, 0x02, false) else {
        return Ok(());
    };
    if v.len() < 4 {
        return Ok(());
    }
    let r = u16::from_le_bytes([v[0], v[1]]);
    let e = u16::from_le_bytes([v[2], v[3]]);
    if r == 0 && e == 0 {
        Ok(())
    } else {
        Err((r, e))
    }
}

// ---------------------------------------------------------------- 请求构造

/// CTL Sync（无 TLV）。
pub fn ctl_sync(transaction: u16) -> Vec<u8> {
    encode(SVC_CTL, 0, transaction, CTL_SYNC, &[])
}

/// CTL Get Client ID（TLV 0x01 = service）。
pub fn ctl_get_client_id(transaction: u16, service: u8) -> Vec<u8> {
    encode(
        SVC_CTL,
        0,
        transaction,
        CTL_GET_CLIENT_ID,
        &tlv(0x01, &[service]),
    )
}

/// CTL Release Client ID（TLV 0x01 = service、0x02 = client id）。
pub fn ctl_release_client_id(transaction: u16, service: u8, client: u8) -> Vec<u8> {
    let mut payload = tlv(0x01, &[service]);
    payload.extend(tlv(0x02, &[client]));
    encode(SVC_CTL, 0, transaction, CTL_RELEASE_CLIENT_ID, &payload)
}

/// WDS Start Network Interface。
///
/// TLV：`0x30` 技术偏好（1 = 3GPP）、`0x14` APN、`0x17/0x18/0x16` 认证、
/// `0x19` IP family、`0x31` profile index（`pdn > 0` 时）。
#[allow(clippy::too_many_arguments)] // QMI 请求本来就是这些 TLV，拆开反而看不出顺序
pub fn wds_start(
    transaction: u16,
    service: u8,
    client: u8,
    apn: &str,
    user: &str,
    pass: &str,
    auth: u8,
    ip_family: u8,
    pdn: i64,
) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(tlv(0x30, &[0x01]));
    if !apn.is_empty() {
        p.extend(tlv(0x14, apn.as_bytes()));
    }
    if !user.is_empty() {
        p.extend(tlv(0x17, user.as_bytes()));
    }
    if !pass.is_empty() {
        p.extend(tlv(0x18, pass.as_bytes()));
    }
    if !user.is_empty() && !pass.is_empty() {
        p.extend(tlv(0x16, &[auth]));
    }
    p.extend(tlv(0x19, &[ip_family]));
    if pdn > 0 && pdn <= u8::MAX as i64 {
        p.extend(tlv(0x31, &[pdn as u8]));
    }
    encode(service, client, transaction, WDS_START_NETWORK, &p)
}

/// WDS Stop Network Interface（TLV 0x01 = handle）。
pub fn wds_stop(transaction: u16, service: u8, client: u8, handle: u32) -> Vec<u8> {
    encode(
        service,
        client,
        transaction,
        WDS_STOP_NETWORK,
        &tlv(0x01, &handle.to_le_bytes()),
    )
}

/// WDS Get Runtime Settings：mask 与 quectel-CM 相同（v4 地址/网关/掩码/DNS + MTU）。
pub fn wds_runtime(transaction: u16, service: u8, client: u8) -> Vec<u8> {
    let mask: u32 = (1 << 4) | (1 << 8) | (1 << 9) | (1 << 13);
    encode(
        service,
        client,
        transaction,
        WDS_GET_RUNTIME_SETTINGS,
        &tlv(0x10, &mask.to_le_bytes()),
    )
}

// ---------------------------------------------------------------- 应答解析

/// Start 应答里的连接句柄（TLV 0x01，跳过结果码）。
pub fn parse_start_handle(payload: &[u8]) -> Option<u32> {
    let v = find_tlv(payload, 0x01, true)?;
    if v.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}

/// Get Client ID 应答里的 client id（TLV 0x02，**跳过开头的同号结果码**）。
pub fn parse_client_id(payload: &[u8]) -> Option<u8> {
    let v = find_tlv(payload, 0x02, true)?;
    v.first().copied()
}

/// 数据呼叫状态：`2` = connected，`1` = disconnected。
pub fn parse_pkt_status(payload: &[u8]) -> Option<u8> {
    find_tlv(payload, 0x01, true).and_then(|v| v.first().copied())
}

/// 运行时设置（地址/网关/DNS/MTU）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub ipv4: Option<Ipv4Addr>,
    pub gateway: Option<Ipv4Addr>,
    pub subnet: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
    pub mtu: Option<u32>,
    pub ipv6: Option<(Ipv6Addr, u8)>,
    pub ipv6_gateway: Option<(Ipv6Addr, u8)>,
}

fn ipv4(v: &[u8]) -> Option<Ipv4Addr> {
    (v.len() >= 4).then(|| Ipv4Addr::new(v[0], v[1], v[2], v[3]))
}

fn ipv6(v: &[u8]) -> Option<(Ipv6Addr, u8)> {
    if v.len() < 16 {
        return None;
    }
    let mut a = [0u8; 16];
    a.copy_from_slice(&v[..16]);
    Some((Ipv6Addr::from(a), v.get(16).copied().unwrap_or(64)))
}

/// 解析 Get Runtime Settings 应答。
pub fn parse_runtime(payload: &[u8]) -> RuntimeSettings {
    let mut out = RuntimeSettings::default();
    for (kind, v) in tlvs(payload, true) {
        match kind {
            0x15 => out.dns.extend(ipv4(v)),
            0x16 => out.dns.extend(ipv4(v)),
            0x1E => out.ipv4 = ipv4(v),
            0x20 => out.gateway = ipv4(v),
            0x21 => out.subnet = ipv4(v),
            0x25 => out.ipv6 = ipv6(v),
            0x26 => out.ipv6_gateway = ipv6(v),
            0x29 if v.len() >= 4 => {
                out.mtu = Some(u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctl_sync_golden_bytes() {
        // 13 字节整帧：IFType + len(=12) + 2 个 flags + service/client + tid + msg + 空 payload
        assert_eq!(
            ctl_sync(1),
            vec![0x01, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x27, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn encode_then_decode_roundtrip() {
        let frame = wds_start(0x1234, SVC_WDS, 7, "ctnet", "", "", 0, IP_FAMILY_V4, 0);
        let mut buf = frame.clone();
        let m = take(&mut buf).expect("完整帧");
        assert!(buf.is_empty(), "一帧正好吃完");
        assert_eq!(m.service, SVC_WDS);
        assert_eq!(m.client, 7);
        assert_eq!(m.transaction, 0x1234);
        assert_eq!(m.message, WDS_START_NETWORK);
        // APN TLV 0x14 在 payload 里
        assert_eq!(find_tlv(&m.payload, 0x14, false), Some(&b"ctnet"[..]));
        // 整帧长度字段 = 帧长 - 1
        assert_eq!(
            u16::from_le_bytes([frame[1], frame[2]]) as usize + 1,
            frame.len()
        );
    }

    #[test]
    fn set_autoconnect_carries_one_byte_tlv() {
        let mut buf = wds_set_autoconnect(3, 5, false);
        let m = take(&mut buf).expect("完整帧");
        assert_eq!(m.service, SVC_WDS);
        assert_eq!(m.client, 5);
        assert_eq!(m.message, WDS_SET_AUTOCONNECT);
        assert_eq!(find_tlv(&m.payload, 0x01, false), Some(&[0u8][..]));
        let mut buf = wds_set_autoconnect(3, 5, true);
        let m = take(&mut buf).expect("完整帧");
        assert_eq!(find_tlv(&m.payload, 0x01, false), Some(&[1u8][..]));
    }

    #[test]
    fn release_client_id_carries_service_and_client() {
        let mut buf = ctl_release_client_id(7, SVC_WDS, 3);
        let m = take(&mut buf).expect("完整帧");
        assert_eq!(m.service, SVC_CTL);
        assert_eq!(m.message, CTL_RELEASE_CLIENT_ID);
        assert_eq!(find_tlv(&m.payload, 0x01, false), Some(&[SVC_WDS][..]));
        assert_eq!(find_tlv(&m.payload, 0x02, false), Some(&[3][..]));
    }

    #[test]
    fn take_handles_partial_and_garbage() {
        let mut buf = ctl_sync(9);
        buf.truncate(6); // 半截
        assert!(take(&mut buf).is_none());
        assert_eq!(buf.len(), 6, "半截帧不能丢，等下一次 read 补齐");
        // 垃圾字节：丢 1 字节继续找
        let mut junk = vec![0xAA, 0xBB];
        assert!(take(&mut junk).is_none());
        assert_eq!(junk, vec![0xBB]);
    }

    #[test]
    fn take_rejects_inconsistent_length() {
        // 帧头说 payload 100 字节，实际只有 1 字节 TLV → 整帧丢弃，不 panic
        let mut frame = encode(SVC_WDS, 1, 1, WDS_START_NETWORK, &[]);
        frame[11] = 100;
        frame[12] = 0;
        let mut buf = frame;
        assert!(take(&mut buf).is_none());
        assert!(buf.is_empty());
    }

    #[test]
    fn tlv_iteration_and_result() {
        let mut p = tlv(0x02, &[0, 0, 0, 0]);
        p.extend(tlv(0x01, &[1, 2, 3]));
        assert!(result(&p).is_ok());
        // 跳过结果码后只剩 0x01
        assert_eq!(find_tlv(&p, 0x01, true), Some(&[1u8, 2, 3][..]));
        // 半截 TLV 不 panic
        assert!(tlvs(&[0x02, 0xFF, 0x00, 0x01], true).is_empty());
        // 失败结果码
        let bad = tlv(0x02, &[1, 0, 0x2D, 0]);
        assert_eq!(result(&bad), Err((1, 45)));
    }

    #[test]
    fn client_id_is_not_confused_with_result_code() {
        // 应答：结果码 0x02(4B) + 服务 0x01 + client id 0x02(1B)
        let mut p = tlv(0x02, &[0, 0, 0, 0]);
        p.extend(tlv(0x01, &[SVC_WDS]));
        p.extend(tlv(0x02, &[9]));
        assert_eq!(parse_client_id(&p), Some(9));
        assert_eq!(parse_client_id(&tlv(0x02, &[0, 0, 0, 0])), None);
    }

    #[test]
    fn start_handle_parsing() {
        let mut p = tlv(0x02, &[0, 0, 0, 0]);
        p.extend(tlv(0x01, &0xDEAD_BEEFu32.to_le_bytes()));
        assert_eq!(parse_start_handle(&p), Some(0xDEAD_BEEF));
        // 句柄 TLV 太短 ⇒ None（不 panic）
        let mut short = tlv(0x02, &[0, 0, 0, 0]);
        short.extend(tlv(0x01, &[1]));
        assert_eq!(parse_start_handle(&short), None);
    }

    #[test]
    fn packet_status_parsing() {
        let mut p = tlv(0x02, &[0, 0, 0, 0]);
        p.extend(tlv(0x01, &[2, 1, 0, 0]));
        assert_eq!(parse_pkt_status(&p), Some(2));
        let mut p = tlv(0x02, &[0, 0, 0, 0]);
        p.extend(tlv(0x01, &[1, 1, 0, 0]));
        assert_eq!(parse_pkt_status(&p), Some(1));
        assert_eq!(parse_pkt_status(&[]), None);
    }

    #[test]
    fn runtime_settings_parse() {
        let mut p = tlv(0x02, &[0, 0, 0, 0]);
        p.extend(tlv(0x1E, &[10, 4, 83, 16]));
        p.extend(tlv(0x20, &[10, 4, 83, 1]));
        p.extend(tlv(0x21, &[255, 255, 255, 0]));
        p.extend(tlv(0x15, &[202, 96, 128, 86]));
        p.extend(tlv(0x16, &[202, 96, 134, 133]));
        p.extend(tlv(0x29, &1500u32.to_le_bytes()));
        let s = parse_runtime(&p);
        assert_eq!(s.ipv4, Some(Ipv4Addr::new(10, 4, 83, 16)));
        assert_eq!(s.gateway, Some(Ipv4Addr::new(10, 4, 83, 1)));
        assert_eq!(s.subnet, Some(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(
            s.dns,
            vec![
                Ipv4Addr::new(202, 96, 128, 86),
                Ipv4Addr::new(202, 96, 134, 133)
            ]
        );
        assert_eq!(s.mtu, Some(1500));

        // 空 payload / 只有结果码：全空，不 panic
        assert_eq!(parse_runtime(&[]), RuntimeSettings::default());
        assert_eq!(
            parse_runtime(&tlv(0x02, &[0, 0, 0, 0])),
            RuntimeSettings::default()
        );
    }

    #[test]
    fn ipv6_address_carries_prefix_length() {
        let mut addr = [0u8; 16];
        addr[0] = 0x24;
        addr[1] = 0x0e;
        let mut v = addr.to_vec();
        v.push(64);
        let mut p = tlv(0x02, &[0, 0, 0, 0]);
        p.extend(tlv(0x25, &v));
        let s = parse_runtime(&p);
        assert_eq!(s.ipv6, Some((Ipv6Addr::from(addr), 64)));
    }
}
