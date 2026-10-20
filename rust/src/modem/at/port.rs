//! AT 串口的打开与配置。

use std::time::Duration;

use crate::error::{msg, Result};

use super::pacing::Pacing;
use super::urc::{LineGate, UrcKind, UrcLine};

/// AT 口的读写后端。
///
/// 分成两种实现：
///
/// - **tty**（`/dev/ttyUSB0`、`/dev/ttyACM0`、pty）：走 `serialport`，有波特率/流控；
/// - **裸字符设备**（`/dev/mhi_DUN0`、`/dev/wwan0at0`、`/dev/mhi_uci_q*`）：
///   PCIe/MHI 模组的 AT 口不是 tty，`tcgetattr` 会回 `ENOTTY`，
///   `serialport` 直接打不开 —— 上游 Go 版为此专门写了裸 open
///   （`module/netdev/atdev.go:90-143`，注释里记着「PCIe 模组曾因此一个都认不出」）。
pub(super) trait AtIo: Send {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
    fn set_timeout(&mut self, timeout: Duration) -> std::io::Result<()>;
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl AtIo for Box<dyn serialport::SerialPort> {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        (**self).write_all(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        (**self).flush()
    }
    fn set_timeout(&mut self, timeout: Duration) -> std::io::Result<()> {
        // serialport 的 set_timeout 回自己的 Error；调用点只用来限读超时，
        // 失败也不致命（读侧本来就有 deadline），这里转成 io::Error 保持接口统一。
        (**self)
            .set_timeout(timeout)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (**self).read(buf)
    }
}

/// 裸字符设备（MHI / wwan AT 口）。
#[cfg(target_os = "linux")]
struct RawChar {
    file: std::fs::File,
    timeout: Duration,
}

#[cfg(target_os = "linux")]
impl RawChar {
    /// Linux 的 `O_NOCTTY` / `O_NONBLOCK`（内核 ABI 固定值）。
    ///
    /// 不引 `libc` 依赖：这两个常量在 Linux 上稳定，且本实现的读写本身要
    /// 自己做超时轮询，留着 `nix`/`libc` 只为两个 flag 不值得。
    const O_NOCTTY: i32 = 0o400;
    const O_NONBLOCK: i32 = 0o4000;

    fn open(path: &str, timeout: Duration) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(Self::O_NOCTTY | Self::O_NONBLOCK)
            .open(path)?;
        Ok(Self { file, timeout })
    }
}

#[cfg(target_os = "linux")]
impl AtIo for RawChar {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let mut left = data;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !left.is_empty() {
            match self.file.write(left) {
                Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                Ok(n) => left = &left[n..],
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return Err(std::io::ErrorKind::TimedOut.into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        self.file.flush()
    }

    fn set_timeout(&mut self, timeout: Duration) -> std::io::Result<()> {
        self.timeout = timeout;
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::Read;
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            match self.file.read(buf) {
                Ok(0) => {
                    // 非阻塞字符设备没有数据时回 0；按「等到超时」处理，
                    // 否则调用方会把「暂时没数据」当成 EOF 立刻返回空应答。
                    if std::time::Instant::now() >= deadline {
                        return Err(std::io::ErrorKind::TimedOut.into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(n) => return Ok(n),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return Err(std::io::ErrorKind::TimedOut.into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

/// 路径是不是字符设备（MHI / wwan 的 AT 口就是这类，但不是 tty）。
#[cfg(target_os = "linux")]
fn is_char_device(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|m| m.file_type().is_char_device())
        .unwrap_or(false)
}

/// 一个已打开的 AT 串口。
pub struct AtPort {
    pub(super) inner: Box<dyn AtIo>,
    pub(super) path: String,
    /// 行级 URC 过滤器：每个串口一份，缓冲与队列随端口生命周期。
    pub(super) gate: LineGate,
    /// 命令节奏：**每条** AT 之间硬性延时（见 `at/pacing.rs`）。
    ///
    /// 放在端口上而不是全局：换口（`repoint`）之后节拍重新起算，
    /// 不会因为「上个口刚发过」让新口的第一条命令白等。
    pub(super) pacing: Pacing,
}

impl AtPort {
    /// 打开并配置串口（8N1，无流控）。
    /// tty 走 `serialport`；打不开且是字符设备时退到裸读写（MHI/wwan AT 口）。
    pub fn open(path: &str, baud: u32, timeout: Duration) -> Result<Self> {
        let inner: Box<dyn AtIo> = match serialport::new(path, baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(timeout)
            .open()
        {
            Ok(p) => Box::new(p),
            Err(e) => {
                #[cfg(target_os = "linux")]
                {
                    if is_char_device(path) {
                        if let Ok(raw) = RawChar::open(path, timeout) {
                            log::debug!("{path} 不是 tty，改用裸字符设备读写");
                            return Ok(Self {
                                inner: Box::new(raw),
                                path: path.to_string(),
                                gate: LineGate::default(),
                                pacing: Pacing::default(),
                            });
                        }
                    }
                }
                return Err(msg(format!("打开串口 {path} 失败: {e}")));
            }
        };
        Ok(Self {
            inner,
            path: path.to_string(),
            gate: LineGate::default(),
            pacing: Pacing::default(),
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// 取走并清空本口积压的 URC（主动上报）。
    ///
    /// 单独放在 `&mut self` 上：过滤器内部有跨 read 的行缓冲，需要写权限。
    pub fn take_urc(&mut self) -> Vec<UrcLine> {
        self.gate.take_urc()
    }

    /// 只取走指定类别的 URC，其余留在队列里（见 [`LineGate::take_urc_of`]）。
    pub fn take_urc_of(&mut self, kinds: &[UrcKind]) -> Vec<UrcLine> {
        self.gate.take_urc_of(kinds)
    }

    /// 被洪水阈值丢弃的 URC 行数（累计，排障用）。
    pub fn urc_dropped(&self) -> u64 {
        self.gate.dropped()
    }

    /// 命令节奏统计（间隔、被节流的次数与累计等待毫秒）。
    pub(crate) fn pacing_stats(&self) -> (u64, u64, u64) {
        (
            self.pacing.gap().as_millis() as u64,
            self.pacing.throttled(),
            self.pacing.waited_ms(),
        )
    }

    /// 快速探活：发 "AT"，能在超时内收到 "OK" 就认为这是 AT 口。
    pub fn probe(path: &str, baud: u32, timeout: Duration) -> bool {
        match AtPort::open(path, baud, timeout) {
            Ok(mut p) => p
                .command("AT", timeout)
                .map(|r| r.contains("OK"))
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// 身份级探活：回 `OK` **还不够**，必须再拿到一行有内容的身份应答。
    ///
    /// ## 为什么 `AT` 回 OK 不能当命令口
    ///
    /// 实测（上游 Go 版 `module/netdev/ports.go:165-252` 的记录）：
    ///
    /// - MBIM 模式下的 `ttyUSB1` 对 `AT` 回 `OK`，但 `AT+CGMM` / `ATI` 全部超时
    ///   —— 那是数据口/诊断口，真发指令只会把拨号流程卡死；
    /// - 部分固件把 `AT` 当成「任意命令」都回 OK；
    /// - 语音 PCM 口打开后也能收到杂散字节。
    ///
    /// 所以这里依次试 `AT+CGMM` → `AT+CGMI` → `ATI`，**至少一条**要给出
    /// 非 `OK`/`ERROR`/回显的内容行；拿不到就判「不是命令口」。
    ///
    /// 返回拿到的身份行（调用方可以直接拿去识别，省一轮 AT）。
    pub fn probe_identity(path: &str, baud: u32, timeout: Duration) -> Option<Vec<String>> {
        let mut p = AtPort::open(path, baud, timeout).ok()?;
        let alive = p.command("AT", timeout).ok()?;
        if !alive.to_ascii_uppercase().contains("OK") {
            return None;
        }
        let mut lines: Vec<String> = Vec::new();
        for cmd in ["AT+CGMM", "AT+CGMI", "ATI"] {
            let Ok(resp) = p.command(cmd, timeout) else {
                continue;
            };
            for line in resp.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                let u = t.to_ascii_uppercase();
                // 回显、终结符、错误行都不算「身份内容」
                if u == "OK"
                    || u == "ERROR"
                    || u.starts_with("+CME ERROR")
                    || u.starts_with("+CMS ERROR")
                {
                    continue;
                }
                if u == cmd.to_ascii_uppercase() || u == "AT" {
                    continue;
                }
                if t.chars().any(|c| c.is_ascii_alphanumeric()) && t.len() >= 2 {
                    lines.push(t.to_string());
                }
            }
            if !lines.is_empty() {
                break;
            }
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines)
        }
    }
}
