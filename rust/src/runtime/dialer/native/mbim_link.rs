//! MBIM 数据链路：`/dev/cdc-wdmN` 上的 OPEN / COMMAND / CLOSE + Basic Connect。
//!
//! 与 [`super::qmi_link`] 平级，覆盖「模组只给 MBIM 接口」的场景
//! （quectel-cm / meig-cm 的 `mbim-cm.c` 是同一套流程）。
//!
//! 流程（MBIM 1.0）：
//! 1. `MBIM_OPEN_MSG` → `OPEN_DONE`（status 0）；
//! 2. `SUBSCRIBER_READY_STATUS` QUERY —— 失败不致命（有些固件在上电早期拒）；
//! 3. `CONNECT` SET（activation=1，带 APN / IP 类型）→ 检查 `activation_state == 2`；
//! 4. `IP_CONFIGURATION` QUERY 读地址（**只是给页面看**：真正的地址仍由 netifd 取）；
//! 5. 退出 `CONNECT` SET（activation=0）→ `MBIM_CLOSE_MSG`。

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use crate::runtime::dialer::mbim::{
    self, IpConfig, CID_CONNECT, CID_IP_CONFIGURATION, CID_SUBSCRIBER_READY,
};

const O_NONBLOCK: i32 = 0o4000;
const OP_TIMEOUT: Duration = Duration::from_secs(15);
/// 激活在弱信号下慢；与 QMI 的 Start 同量级。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

/// 一条 MBIM 连接（请求-应答串行化）。
pub struct MbimLink {
    file: File,
    rx: Vec<u8>,
    tid: u32,
}

impl MbimLink {
    /// 打开设备并完成 `MBIM_OPEN` 握手。
    pub fn open(path: &str) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(path)
            .map_err(|e| format!("打开 {path} 失败: {e}"))?;
        let mut link = Self {
            file,
            rx: Vec::new(),
            tid: 0,
        };
        let tid = link.next_tid();
        let frame = mbim::open(tid);
        link.write_frame(&frame, Instant::now() + OP_TIMEOUT)?;
        let body = link.read_done(mbim::MSG_OPEN_DONE, tid, Instant::now() + OP_TIMEOUT)?;
        match mbim::done_status(&body) {
            Some(0) => Ok(link),
            Some(s) => Err(format!("MBIM_OPEN 被拒（status={s:#x}）")),
            None => Err("MBIM_OPEN 应答太短".into()),
        }
    }

    fn next_tid(&mut self) -> u32 {
        self.tid = self.tid.wrapping_add(1);
        if self.tid == 0 {
            self.tid = 1;
        }
        self.tid
    }

    fn write_frame(&mut self, frame: &[u8], deadline: Instant) -> Result<(), String> {
        let mut off = 0usize;
        while off < frame.len() {
            match self.file.write(&frame[off..]) {
                Ok(0) => return Err("cdc-wdm 写入返回 0".into()),
                Ok(n) => off += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err("写 cdc-wdm 超时".into());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("写 cdc-wdm 失败: {e}")),
            }
        }
        Ok(())
    }

    /// 等指定类型的应答（跳过 indication 与别的会话）。
    fn read_done(
        &mut self,
        kind: u32,
        transaction: u32,
        deadline: Instant,
    ) -> Result<Vec<u8>, String> {
        loop {
            while let Some(m) = mbim::take(&mut self.rx) {
                if m.kind == mbim::MSG_FUNCTION_ERROR || m.kind == mbim::MSG_HOST_ERROR {
                    let code = mbim::done_status(&m.body).unwrap_or(0);
                    return Err(format!(
                        "设备返回错误帧（type={:#x}, status={code:#x}）",
                        m.kind
                    ));
                }
                if m.kind == kind && m.transaction == transaction {
                    return Ok(m.body);
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "等 MBIM 应答超时（type={kind:#x} tid={transaction}）"
                ));
            }
            let mut buf = [0u8; 4096];
            match self.file.read(&mut buf) {
                Ok(0) => return Err("cdc-wdm 已关闭".into()),
                Ok(n) => self.rx.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("读 cdc-wdm 失败: {e}")),
            }
        }
    }

    /// 发一条 Basic Connect 命令，返回 info buffer。
    fn command(
        &mut self,
        cid: u32,
        kind: u32,
        info: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let tid = self.next_tid();
        let frame = mbim::command(tid, &mbim::UUID_BASIC_CONNECT, cid, kind, info);
        let deadline = Instant::now() + timeout;
        self.write_frame(&frame, deadline)?;
        let body = self.read_done(mbim::MSG_COMMAND_DONE, tid, deadline)?;
        match mbim::command_status(&body) {
            Some(0) => Ok(mbim::command_info(&body).to_vec()),
            Some(s) => Err(format!("CID {cid} 被拒（status={s:#x}）")),
            None => Err("COMMAND_DONE 应答太短".into()),
        }
    }

    /// 订阅者是否就绪（SIM 已解锁 / 已注册）。
    pub fn subscriber_ready(&mut self) -> bool {
        self.command(CID_SUBSCRIBER_READY, mbim::CMD_QUERY, &[], OP_TIMEOUT)
            .map(|info| mbim::subscriber_ready(&info))
            .unwrap_or(false)
    }

    /// 激活 / 去激活一个会话，返回 `NwError`（0 = 正常）。
    pub fn connect(
        &mut self,
        session: u32,
        activate: bool,
        apn: &str,
        ip_type: u32,
    ) -> Result<u32, String> {
        let info = mbim::connect_set(session, activate, apn, "", "", ip_type);
        let out = self.command(CID_CONNECT, mbim::CMD_SET, &info, CONNECT_TIMEOUT)?;
        let state = mbim::connect_state(&out).unwrap_or(0);
        let nw = mbim::connect_nw_error(&out).unwrap_or(0);
        if activate && state != mbim::ACTIVATION_ACTIVATED {
            return Err(format!(
                "会话没有激活（activation_state={state}, nw_error={nw:#x}）"
            ));
        }
        Ok(nw)
    }

    /// 查一次 IP 配置。
    pub fn ip_configuration(&mut self, session: u32) -> Result<IpConfig, String> {
        let info = self.command(
            CID_IP_CONFIGURATION,
            mbim::CMD_QUERY,
            &mbim::ip_configuration_query(session),
            OP_TIMEOUT,
        )?;
        Ok(mbim::parse_ip_configuration(&info))
    }

    /// 当前是否已激活（探活用）。
    pub fn activated(&mut self, session: u32) -> Result<bool, String> {
        let info = self.command(
            CID_CONNECT,
            mbim::CMD_QUERY,
            &session.to_le_bytes(),
            OP_TIMEOUT,
        )?;
        Ok(mbim::connect_state(&info) == Some(mbim::ACTIVATION_ACTIVATED))
    }

    /// 关连接（重复调用安全；失败只记日志）。
    pub fn close(&mut self) {
        let tid = self.next_tid();
        let frame = mbim::close(tid);
        let deadline = Instant::now() + OP_TIMEOUT;
        if self.write_frame(&frame, deadline).is_err() {
            return;
        }
        let _ = self.read_done(mbim::MSG_CLOSE_DONE, tid, deadline);
    }
}

impl Drop for MbimLink {
    fn drop(&mut self) {
        self.close();
    }
}

/// 一个在跑的 MBIM 数据会话。
pub struct MbimSession {
    link: MbimLink,
    session: u32,
    pub settings: IpConfig,
    /// 激活时设备给的网络错误码（0 = 正常；非 0 时页面可提示）。
    pub nw_error: u32,
}

impl MbimSession {
    pub fn start(wdm: &str, apn: &str, ipv6: bool, session: u32) -> Result<(Self, String), String> {
        let mut link = MbimLink::open(wdm)?;
        let mut note = String::new();
        if !link.subscriber_ready() {
            // 不致命：有些固件在刚上电时对这条回失败，但 CONNECT 仍能成
            note = "订阅者未就绪（继续尝试激活）".to_string();
        }
        let ip_type = if ipv6 {
            mbim::IP_TYPE_IPV4V6
        } else {
            mbim::IP_TYPE_IPV4
        };
        let nw_error = link.connect(session, true, apn, ip_type)?;
        if nw_error != 0 {
            note = format!("激活带网络错误 nw_error={nw_error:#x}");
        }
        let settings = link.ip_configuration(session).unwrap_or_default();
        if settings.ipv4.is_none() {
            note.push_str("；暂未拿到 IPv4（等 netifd 取）");
        }
        Ok((
            Self {
                link,
                session,
                settings,
                nw_error,
            },
            note,
        ))
    }

    pub fn alive(&mut self) -> Option<bool> {
        self.link.activated(self.session).ok()
    }

    pub fn stop(&mut self) {
        if let Err(e) = self
            .link
            .connect(self.session, false, "", mbim::IP_TYPE_DEFAULT)
        {
            log::debug!("原生 MBIM：去激活失败: {e}");
        }
    }

    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "session": self.session,
            "mtu": self.settings.mtu,
            "modem_ipv4": self.settings.ipv4,
            "prefix": self.settings.prefix,
            "gateway": self.settings.gateway,
            "dns": self.settings.dns,
            "modem_ipv6": self.settings.ipv6.as_ref().map(|(ip, p)| format!("{ip}/{p}")),
            "nw_error": self.nw_error,
        })
    }
}

impl Drop for MbimSession {
    fn drop(&mut self) {
        self.stop();
    }
}
