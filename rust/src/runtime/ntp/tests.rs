//! NTP 模块单测：协议编解码、时区表、假服务器端到端。
//!
//! 不打真实 NTP 服务器（CI 无外网 / 会引入不确定性）；用本地 UDP 假服务器
//! 回一个构造好的报文，验证「校验 + 偏移计算」这条链。

use std::net::UdpSocket;
use std::time::Duration;

use super::*;

fn ts(unix: f64) -> [u8; 8] {
    let mut out = [0u8; 8];
    secs_to_ts(unix, &mut out);
    out
}

#[test]
fn timestamp_roundtrip() {
    for v in [0.0, 1_700_000_000.5, 1_900_000_000.25] {
        let b = ts(v);
        let back = ts_to_secs(&b);
        assert!((back - v).abs() < 0.001, "{v} → {back}");
    }
}

#[test]
fn iana_timezone_table() {
    assert_eq!(iana_offset("Asia/Shanghai"), Some(8 * 3600));
    assert_eq!(iana_offset("asia/shanghai"), Some(8 * 3600), "大小写无关");
    assert_eq!(iana_offset("Asia/Hong_Kong"), Some(8 * 3600));
    assert_eq!(iana_offset("Asia/Tokyo"), Some(9 * 3600));
    assert_eq!(iana_offset("UTC"), Some(0));
    assert_eq!(iana_offset("Europe/London"), Some(0));
    assert_eq!(iana_offset("America/New_York"), Some(-5 * 3600));
    // 半小时 / 45 分钟时区
    assert_eq!(iana_offset("Asia/Kolkata"), Some(5 * 3600 + 1800));
    assert_eq!(iana_offset("Asia/Kathmandu"), Some(5 * 3600 + 2700));
    assert_eq!(iana_offset("Asia/Nowhere"), None);
}

#[test]
fn timezone_accepts_posix_and_iana() {
    // POSIX 写法：西为正、符号翻转
    assert_eq!(parse_timezone("CST-8"), Some(8 * 3600));
    assert_eq!(parse_timezone("UTC+3"), Some(-3 * 3600));
    // IANA（含 `:Asia/Shanghai` 的 TZif 引用写法）
    assert_eq!(parse_timezone("Asia/Shanghai"), Some(8 * 3600));
    assert_eq!(parse_timezone(":Asia/Shanghai"), Some(8 * 3600));
    // 空 / 认不出
    assert_eq!(parse_timezone(""), None);
    assert_eq!(parse_timezone("Nowhere/Land"), None);
}

#[test]
fn query_rejects_short_packet() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let t = std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let (_, from) = sock.recv_from(&mut buf).unwrap();
        sock.send_to(&[0u8; 10], from).unwrap();
    });
    let e = query_addr(addr, "short.test", Duration::from_millis(500)).unwrap_err();
    assert!(e.contains("短包"), "错误要说清是短包: {e}");
    t.join().unwrap();
}

#[test]
fn query_success_returns_offset() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let server_now = 1_700_000_123.0f64;
    let t = std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let (n, from) = sock.recv_from(&mut buf).unwrap();
        assert_eq!(n, 48, "客户端应发 48 字节");
        assert_eq!(buf[0] & 0x07, 3, "mode 应为 3（client）");
        let mut resp = [0u8; 48];
        resp[0] = 0x24; // LI=0, VN=4, Mode=4（server）
        resp[1] = 1; // stratum 1
        resp[24..32].copy_from_slice(&buf[40..48]); // originate = 客户端 T1
        resp[32..40].copy_from_slice(&ts(server_now)); // receive
        resp[40..48].copy_from_slice(&ts(server_now + 0.001)); // transmit
        sock.send_to(&resp, from).unwrap();
    });
    let got = query_addr(addr, "fake.test", Duration::from_secs(2)).expect("应当成功");
    assert_eq!(got.server, "fake.test");
    let expect = server_now as i64 - super::system_now();
    assert!(
        (got.offset_secs - expect).abs() <= 2,
        "offset 应接近 {expect}，实际 {}",
        got.offset_secs
    );
    t.join().unwrap();
}

#[test]
fn query_rejects_mismatched_originate() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let t = std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let (_, from) = sock.recv_from(&mut buf).unwrap();
        let mut resp = [0u8; 48];
        resp[0] = 0x24;
        resp[1] = 1;
        // originate 故意留 0（对不上客户端 T1）
        resp[32..40].copy_from_slice(&ts(1_700_000_000.0));
        resp[40..48].copy_from_slice(&ts(1_700_000_001.0));
        sock.send_to(&resp, from).unwrap();
    });
    let e = query_addr(addr, "evil.test", Duration::from_millis(500)).unwrap_err();
    assert!(e.contains("originate"), "应识别串包/伪造: {e}");
    t.join().unwrap();
}

#[test]
fn query_rejects_kiss_of_death() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let t = std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let (_, from) = sock.recv_from(&mut buf).unwrap();
        let mut resp = [0u8; 48];
        resp[0] = 0x24;
        resp[1] = 0; // stratum 0 = kiss-o'-death
        resp[12..16].copy_from_slice(b"RATE");
        resp[24..32].copy_from_slice(&buf[40..48]);
        sock.send_to(&resp, from).unwrap();
    });
    let e = query_addr(addr, "kod.test", Duration::from_millis(500)).unwrap_err();
    assert!(e.contains("kiss"), "应识别 kiss-o'-death: {e}");
    t.join().unwrap();
}

#[test]
fn query_times_out_with_clear_message() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    let e = query_addr(addr, "silent.test", Duration::from_millis(120)).unwrap_err();
    assert!(e.contains("超时"), "超时要写清: {e}");
}

#[test]
fn query_any_handles_empty_and_unreachable() {
    let e = query_any(&[], Duration::from_millis(50)).unwrap_err();
    assert!(e.contains("没有配置"));
    let servers = vec!["127.0.0.1".to_string(), "127.0.0.2".to_string()];
    if let Err(e) = query_any(&servers, Duration::from_millis(80)) {
        assert!(e.contains("127.0.0"), "错误里要带上服务器名: {e}");
    }
}

#[test]
fn status_json_has_time_fields() {
    let st = Status::new(true, vec!["a.test".into()], "Asia/Shanghai");
    let v = st.to_json();
    assert_eq!(v["enabled"], true);
    assert_eq!(v["source"], "system", "未同步时回退设备时间");
    assert_eq!(v["phase"], "fast");
    assert_eq!(v["servers"].as_array().unwrap().len(), 1);
    assert_eq!(v["timezone"], "Asia/Shanghai");
    assert!(v["now"].as_str().unwrap().contains('T'));
    assert!(v["now_local"].as_str().unwrap().contains(':'));
}
