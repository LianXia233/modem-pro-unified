//! 日志输出：优先写 busybox syslog（`/dev/log`），不可用时退到 stderr。
//!
//! 旧 shell 用 `logger -t modem-pro "<msg>"`，本实现保持同样的 tag 与文案，
//! `logread` 里的观感不变（busybox syslogd 会对没有时间戳的报文自行补时间）。

use std::io::Write;

/// 打一行日志（tag 固定 `modem-pro`）。
pub fn line(msg: &str) {
    let tagged = format!("modem-pro: {msg}");
    if syslog(&tagged).is_ok() {
        return;
    }
    let _ = writeln!(std::io::stderr(), "{tagged}");
}

#[cfg(unix)]
fn syslog(msg: &str) -> std::io::Result<()> {
    use std::os::unix::net::UnixDatagram;
    use std::sync::OnceLock;

    // `<14>` = facility user(1) * 8 + severity info(6)，与 logger 默认一致。
    static SOCK: OnceLock<Option<UnixDatagram>> = OnceLock::new();
    let sock = SOCK.get_or_init(|| {
        let s = UnixDatagram::unbound().ok()?;
        match s.connect("/dev/log") {
            Ok(()) => Some(s),
            Err(_) => None,
        }
    });

    match sock {
        Some(s) => s.send(format!("<14>{msg}").as_bytes()).map(|_| ()),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "/dev/log 不可用",
        )),
    }
}

#[cfg(not(unix))]
fn syslog(_msg: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "本平台无 syslog",
    ))
}
