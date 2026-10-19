//! AT 串口的打开与配置。

use std::time::Duration;

use crate::error::{msg, Result};

use super::pacing::Pacing;
use super::urc::{LineGate, UrcKind, UrcLine};

/// 一个已打开的 AT 串口。
pub struct AtPort {
    pub(super) inner: Box<dyn serialport::SerialPort>,
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
    pub fn open(path: &str, baud: u32, timeout: Duration) -> Result<Self> {
        let inner = serialport::new(path, baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(timeout)
            .open()
            .map_err(|e| msg(format!("打开串口 {path} 失败: {e}")))?;
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
}
