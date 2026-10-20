//! AT 命令发送与应答读取。

use std::time::{Duration, Instant};

use crate::error::{msg, Result};

use super::parser::has_terminator;
use super::port::AtPort;

impl AtPort {
    /// 写字节并 flush（两个写入口共用，错误信息统一带串口路径）。
    ///
    /// flush 失败刻意忽略：`serialport` 的 flush 在部分后端是 no-op，
    /// 且写已成功；真正该报的错由 `write_all` 负责。
    fn write_bytes(&mut self, data: &[u8]) -> Result<()> {
        self.inner
            .write_all(data)
            .map_err(|e| msg(format!("写串口 {} 失败: {e}", self.path)))?;
        let _ = self.inner.flush();
        Ok(())
    }

    /// 发一条 AT 命令（自动补 \r），读回应答；遇到终结符或超时返回。
    pub fn command(&mut self, cmd: &str, timeout: Duration) -> Result<String> {
        self.command_cancellable(cmd, timeout, &|| false)
            .map(|(resp, _)| resp)
    }

    /// 同 [`Self::command`]，但可被 `cancel` 叫停。
    ///
    /// 主动扫网（`AT+COPS=?`）会占住串口几分钟，页面需要「停止扫描」把它打断：
    /// 叫停后**已收到的部分应答照样返回**（里面的运营商往往已经够看），
    /// 第二个返回值 = 是否是被叫停的。
    pub fn command_cancellable(
        &mut self,
        cmd: &str,
        timeout: Duration,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(String, bool)> {
        self.drain();
        // ⚠️ 硬性节奏：距上一条不足 `Pacing::gap`（默认 25ms）就先睡够。
        //
        // 这里是**每一条**命令的落点 —— 一次逻辑操作里连发 3~5 条（读参数、
        // 读短信）时，每条之间都有间隔，而不是「一口气全打给模块」。
        // 早先只在「两次占用串口」之间限流，闭包内连发是无间隔的，实测会
        // 丢应答 / 偶发 ERROR / 严重时 AT 口假死。
        self.pacing.wait_gap();
        let mut out = String::with_capacity(cmd.len() + 2);
        out.push_str(cmd);
        out.push('\r');
        self.write_bytes(out.as_bytes())?;
        // 写完才记时刻：从「写完」算间隔，模组收到的两条命令间隔才真是 gap
        self.pacing.mark_sent();
        self.read_until_terminator(timeout, cancel)
    }

    /// 写原始数据（**不自动补 \r**）。
    ///
    /// `AT+CMGS` 这类指令要分两步：先发命令等 `>` 提示符，再发正文 + Ctrl-Z，
    /// 中间不能插入别的东西，所以调用方得自己控制换行符。
    pub fn write_raw(&mut self, data: &str) -> Result<()> {
        self.write_bytes(data.as_bytes())
    }

    /// 读到出现 `marker` 为止（等 `>` 这类提示符用），超时返回已收到的内容。
    pub fn read_until_marker(&mut self, marker: &str, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        let mut acc = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let _ = self.inner.set_timeout(left.min(Duration::from_millis(100)));
            match self.inner.read(&mut buf) {
                Ok(0) => continue,
                Ok(n) => {
                    // 过 URC 过滤层：主动上报不进应答，也不进 marker 匹配
                    self.gate.feed(&buf[..n], &mut acc);
                    let text = String::from_utf8_lossy(&acc);
                    if text.contains(marker) {
                        return Ok(text.to_string());
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(e) => return Err(msg(format!("读串口 {} 失败: {e}", self.path))),
            }
        }
        Ok(String::from_utf8_lossy(&acc).to_string())
    }

    /// 只读应答（不发命令）：等终结符或超时，用于 `AT+CMGS` 发完正文之后收尾。
    pub fn read_response(&mut self, timeout: Duration) -> Result<String> {
        self.read_until_terminator(timeout, &|| false)
            .map(|(resp, _)| resp)
    }

    /// 读走并丢弃当前缓冲区里的残留数据（避免上一条命令的尾巴串进下一条）。
    fn drain(&mut self) {
        let _ = self.inner.set_timeout(Duration::from_millis(30));
        let mut buf = [0u8; 512];
        for _ in 0..8 {
            match self.inner.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    }

    fn read_until_terminator(
        &mut self,
        timeout: Duration,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(String, bool)> {
        let deadline = Instant::now() + timeout;
        let mut acc = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            if cancel() {
                // 被叫停：手上有什么就交什么，让上层决定要不要用
                if acc.is_empty() {
                    return Ok((String::new(), true));
                }
                return Ok((String::from_utf8_lossy(&acc).to_string(), true));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let _ = self.inner.set_timeout(left.min(Duration::from_millis(250)));
            match self.inner.read(&mut buf) {
                Ok(0) => continue,
                Ok(n) => {
                    // 过 URC 过滤层：主动上报不进应答，也就不可能被
                    // 解析层当成查询结果（实测 +CEREG: 5 抢跑导致注册态误报）
                    self.gate.feed(&buf[..n], &mut acc);
                    let text = String::from_utf8_lossy(&acc);
                    if has_terminator(&text) {
                        return Ok((text.to_string(), false));
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(e) => return Err(msg(format!("读串口 {} 失败: {e}", self.path))),
            }
        }
        if acc.is_empty() {
            return Err(msg(format!("串口 {} 无应答（超时）", self.path)));
        }
        Ok((String::from_utf8_lossy(&acc).to_string(), false))
    }
}
