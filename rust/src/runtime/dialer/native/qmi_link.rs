//! QMI 数据链路：`/dev/cdc-wdmN` 上的请求-应答串行化 + WDS 数据呼叫。
//!
//! 这是四家拨号器**共用**的底层（上游 quectel-cm / fibocom-dial / simcom-cm 的
//! `QMIThread.c` 是同一份代码的不同分支；meig-cm 的 QMI 走代理但协议一致）。
//! 差异只在「用哪条指令、要不要先关自动连接」这类策略，放在 `native/vendors/`。

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use crate::runtime::dialer::qmi::{self, RuntimeSettings};

/// `O_NONBLOCK`（Linux 各架构一致）；用常量而不引 libc，保持依赖精简。
const O_NONBLOCK: i32 = 0o4000;

/// 一轮控制请求的默认超时。
pub const CTL_TIMEOUT: Duration = Duration::from_secs(8);
/// Start Network Interface 在弱信号下可能很慢（quectel-CM 给 120 秒）。
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Stop / Runtime Settings / 状态查询。
pub const OP_TIMEOUT: Duration = Duration::from_secs(15);

/// 与 `/dev/cdc-wdmN` 的一条连接（请求-应答串行化）。
pub struct QmiLink {
    file: File,
    rx: Vec<u8>,
    tid: u16,
}

impl QmiLink {
    pub fn open(path: &str) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(path)
            .map_err(|e| format!("打开 {path} 失败: {e}"))?;
        Ok(Self {
            file,
            rx: Vec::new(),
            tid: 0,
        })
    }

    fn next_tid(&mut self) -> u16 {
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

    /// 等一帧应答（跳过 indication 与别的会话的帧）。
    fn read_response(
        &mut self,
        service: u8,
        message: u16,
        transaction: u16,
        deadline: Instant,
    ) -> Result<Vec<u8>, String> {
        loop {
            while let Some(m) = qmi::take(&mut self.rx) {
                if m.service == service && m.message == message && m.transaction == transaction {
                    return Ok(m.payload);
                }
                // 别的会话 / 主动上报：忽略（indication 的 transaction 通常为 0）
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "等 QMI 应答超时（service {service:#04x} message {message:#06x} tid {transaction}）"
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

    /// 写一帧、等对应应答，并检查结果码。
    fn transact(
        &mut self,
        frame: &[u8],
        service: u8,
        message: u16,
        tid: u16,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + timeout;
        self.write_frame(frame, deadline)?;
        let payload = self.read_response(service, message, tid, deadline)?;
        qmi::result(&payload).map_err(|(r, e)| format!("QMI 返回失败 result={r} error={e}"))?;
        Ok(payload)
    }

    /// 释放一个 client id（长驻进程反复重拨时必做，否则 QMI 的 16 个槽位会用光）。
    pub fn release_client_id(&mut self, service: u8, client: u8) -> Result<(), String> {
        let tid = self.next_tid();
        let frame = qmi::ctl_release_client_id(tid, service, client);
        self.transact(
            &frame,
            qmi::SVC_CTL,
            qmi::CTL_RELEASE_CLIENT_ID,
            tid,
            CTL_TIMEOUT,
        )?;
        Ok(())
    }

    /// CTL Sync（拿 client id 之前必须先做）。
    pub fn sync(&mut self) -> Result<(), String> {
        let tid = self.next_tid();
        let frame = qmi::ctl_sync(tid);
        self.transact(&frame, qmi::SVC_CTL, qmi::CTL_SYNC, tid, CTL_TIMEOUT)?;
        Ok(())
    }

    /// 申请一个服务的 client id。
    pub fn client_id(&mut self, service: u8) -> Result<u8, String> {
        let tid = self.next_tid();
        let frame = qmi::ctl_get_client_id(tid, service);
        let payload = self.transact(
            &frame,
            qmi::SVC_CTL,
            qmi::CTL_GET_CLIENT_ID,
            tid,
            CTL_TIMEOUT,
        )?;
        qmi::parse_client_id(&payload)
            .ok_or_else(|| format!("Get Client ID 应答里没有 client id（service {service:#04x}）"))
    }

    /// 关掉固件的自动拨号（失败不致命：有的固件本来就不支持）。
    pub fn set_autoconnect(&mut self, client: u8, enable: bool) -> Result<(), String> {
        let tid = self.next_tid();
        let frame = qmi::wds_set_autoconnect(tid, client, enable);
        let deadline = Instant::now() + CTL_TIMEOUT;
        self.write_frame(&frame, deadline)?;
        let payload = self.read_response(qmi::SVC_WDS, qmi::WDS_SET_AUTOCONNECT, tid, deadline)?;
        qmi::result(&payload)
            .map_err(|(r, e)| format!("Set Autoconnect 失败 result={r} error={e}"))?;
        Ok(())
    }

    /// 下发 QMAP 数据格式（WDA `Set Data Format`）。
    ///
    /// 这一条决定 `wwan0_1` 上跑的是 QMAP 聚合还是裸 IP：不发的话驱动不会
    /// `add_mux`，QMAP 模组的数据口永远是 NO-CARRIER（现象见配置注释里
    /// 「FM160-CN + qmi_wwan_f 不传 `-n` 就 add_mux 不起来」那一段）。
    pub fn set_data_format(
        &mut self,
        client: u8,
        settings: &qmi::QmapSettings,
    ) -> Result<(), String> {
        let tid = self.next_tid();
        let frame = qmi::wda_set_data_format(tid, client, settings);
        let deadline = Instant::now() + CTL_TIMEOUT;
        self.write_frame(&frame, deadline)?;
        let payload = self.read_response(
            qmi::SVC_WDA,
            qmi::WDA_SET_DATA_FORMAT,
            tid,
            deadline,
        )?;
        qmi::result(&payload)
            .map_err(|(r, e)| format!("Set Data Format 失败 result={r} error={e}"))?;
        Ok(())
    }

    /// 把 WDS client 绑到 QMAP 的 mux 上（`Bind Mux Data Port`）。
    ///
    /// 必须**每个 WDS client 各绑一次**（v4 走 service 1、v6 走 0x11），
    /// 与 quectel-CM 在 `requestSetEthMode()` 里的做法一致。
    pub fn bind_mux(
        &mut self,
        service: u8,
        client: u8,
        settings: &qmi::QmapSettings,
    ) -> Result<(), String> {
        let tid = self.next_tid();
        let frame = qmi::wds_bind_mux(tid, service, client, settings);
        let deadline = Instant::now() + CTL_TIMEOUT;
        self.write_frame(&frame, deadline)?;
        let payload = self.read_response(service, qmi::WDS_BIND_MUX_DATA_PORT, tid, deadline)?;
        qmi::result(&payload)
            .map_err(|(r, e)| format!("Bind Mux Data Port 失败 result={r} error={e}"))?;
        Ok(())
    }

    /// 建立一个数据呼叫，返回 handle。
    ///
    /// `pdn` 作为 profile index 下发；`auth` 由厂商策略给出（目前都是 none）。
    pub fn start(
        &mut self,
        service: u8,
        client: u8,
        apn: &str,
        family: u8,
        pdn: i64,
    ) -> Result<u32, String> {
        let tid = self.next_tid();
        let frame = qmi::wds_start(tid, service, client, apn, "", "", 0, family, pdn);
        let deadline = Instant::now() + START_TIMEOUT;
        self.write_frame(&frame, deadline)?;
        let payload = self.read_response(service, qmi::WDS_START_NETWORK, tid, deadline)?;
        qmi::result(&payload)
            .map_err(|(r, e)| format!("Start Network 失败 result={r} error={e}"))?;
        qmi::parse_start_handle(&payload).ok_or_else(|| "Start Network 应答里没有 handle".into())
    }

    /// 拆掉一个数据呼叫。
    pub fn stop(&mut self, service: u8, client: u8, handle: u32) -> Result<(), String> {
        let tid = self.next_tid();
        let frame = qmi::wds_stop(tid, service, client, handle);
        let deadline = Instant::now() + OP_TIMEOUT;
        self.write_frame(&frame, deadline)?;
        let payload = self.read_response(service, qmi::WDS_STOP_NETWORK, tid, deadline)?;
        qmi::result(&payload)
            .map_err(|(r, e)| format!("Stop Network 失败 result={r} error={e}"))?;
        Ok(())
    }

    /// 读运行时设置（地址 / 网关 / DNS / MTU）。
    pub fn runtime(&mut self, service: u8, client: u8) -> Result<RuntimeSettings, String> {
        let tid = self.next_tid();
        let frame = qmi::wds_runtime(tid, service, client);
        let deadline = Instant::now() + OP_TIMEOUT;
        self.write_frame(&frame, deadline)?;
        let payload = self.read_response(service, qmi::WDS_GET_RUNTIME_SETTINGS, tid, deadline)?;
        qmi::result(&payload).map_err(|(r, e)| format!("Get Runtime 失败 result={r} error={e}"))?;
        Ok(qmi::parse_runtime(&payload))
    }

    /// 数据呼叫当前状态：`true` = 已连接。
    pub fn connected(&mut self, service: u8, client: u8) -> Result<bool, String> {
        let tid = self.next_tid();
        let frame = qmi::encode(service, client, tid, qmi::WDS_GET_PKT_STATUS, &[]);
        let deadline = Instant::now() + OP_TIMEOUT;
        self.write_frame(&frame, deadline)?;
        let payload = self.read_response(service, qmi::WDS_GET_PKT_STATUS, tid, deadline)?;
        qmi::result(&payload)
            .map_err(|(r, e)| format!("Get Pkt Status 失败 result={r} error={e}"))?;
        Ok(qmi::parse_pkt_status(&payload) == Some(2))
    }
}

/// 一个在跑的 QMI 数据呼叫（v4 必有，v6 可选）。
pub struct QmiSession {
    link: QmiLink,
    client4: u8,
    handle4: u32,
    client6: Option<u8>,
    handle6: Option<u32>,
    /// QMAP 的 WDA client（非 QMAP 模组为 `None`），退出时要还回去。
    wda_client: Option<u8>,
    /// 这一轮用的 QMAP 参数（给状态接口看）。
    qmap: Option<qmi::QmapSettings>,
    pub settings: RuntimeSettings,
}

impl QmiSession {
    /// 建立呼叫：v4 必须成功；v6 失败只记 note（很多网络本来就不给 v6）。
    ///
    /// `qmap` 非空时按 quectel-CM `requestSetEthMode()` 的顺序先做
    /// `WDA Set Data Format`，再把 v4/v6 两个 WDS client 绑到同一个 mux 上。
    ///
    /// 返回 `(会话, 备注)`；备注是给页面看的一句话（非致命情况）。
    pub fn start(
        wdm: &str,
        iface: &str,
        apn: &str,
        ipv6: bool,
        pdn: i64,
        disable_autoconnect: bool,
        qmap: Option<qmi::QmapSettings>,
    ) -> Result<(Self, String), String> {
        if !super::ensure_raw_ip(iface) {
            return Err(format!("{iface} 的 qmi/raw_ip 设置失败"));
        }
        let mut link = QmiLink::open(wdm)?;
        link.sync()?;
        let client4 = link.client_id(qmi::SVC_WDS)?;

        let mut note = String::new();
        // v6 的 client 提前申请：QMAP 要在 Start 之前把两个 client 都绑到 mux 上
        let mut client6 = None;
        if ipv6 {
            match link.client_id(qmi::SVC_WDS_IPV6) {
                Ok(c) => client6 = Some(c),
                Err(e) => note = format!("IPv6 client id 申请失败：{e}"),
            }
        }

        // QMAP：数据格式 + 绑 mux（缺了这两步，驱动不会 add_mux，数据口永远 NO-CARRIER）
        let mut wda_client = None;
        if let Some(q) = qmap {
            let wda = match link.client_id(qmi::SVC_WDA) {
                Ok(w) => w,
                Err(e) => {
                    let _ = link.release_client_id(qmi::SVC_WDS, client4);
                    if let Some(c6) = client6 {
                        let _ = link.release_client_id(qmi::SVC_WDS_IPV6, c6);
                    }
                    return Err(format!("QMAP 需要 WDA client，申请失败：{e}"));
                }
            };
            let setup = link
                .set_data_format(wda, &q)
                .and_then(|_| link.bind_mux(qmi::SVC_WDS, client4, &q));
            if let Err(e) = setup {
                let _ = link.release_client_id(qmi::SVC_WDA, wda);
                let _ = link.release_client_id(qmi::SVC_WDS, client4);
                if let Some(c6) = client6 {
                    let _ = link.release_client_id(qmi::SVC_WDS_IPV6, c6);
                }
                return Err(format!("QMAP 初始化失败：{e}"));
            }
            if let Some(c6) = client6 {
                if let Err(e) = link.bind_mux(qmi::SVC_WDS_IPV6, c6, &q) {
                    // v6 绑不上就只走 v4，别让整条呼叫失败
                    note = format!("IPv6 绑 mux 失败：{e}");
                    let _ = link.release_client_id(qmi::SVC_WDS_IPV6, c6);
                    client6 = None;
                }
            }
            wda_client = Some(wda);
        }

        // 先关自动连接再 Start：顺序与 quectel-cm 的 QmiWwanInit() 一致
        if disable_autoconnect {
            if let Err(e) = link.set_autoconnect(client4, false) {
                // 不致命：个别固件不认这条，Start 仍可能成功
                log::debug!("原生 QMI：关自动连接失败（继续）: {e}");
            }
        }
        let handle4 = match link.start(qmi::SVC_WDS, client4, apn, qmi::IP_FAMILY_V4, pdn) {
            Ok(h) => h,
            Err(e) => {
                // 起不来就把 client id 还回去，别占着槽位
                let _ = link.release_client_id(qmi::SVC_WDS, client4);
                if let Some(w) = wda_client {
                    let _ = link.release_client_id(qmi::SVC_WDA, w);
                }
                if let Some(c6) = client6 {
                    let _ = link.release_client_id(qmi::SVC_WDS_IPV6, c6);
                }
                return Err(e);
            }
        };
        let mut handle6 = None;
        if let Some(c6) = client6 {
            match link.start(qmi::SVC_WDS_IPV6, c6, apn, qmi::IP_FAMILY_V6, pdn) {
                Ok(h) => handle6 = Some(h),
                Err(e) => {
                    let _ = link.release_client_id(qmi::SVC_WDS_IPV6, c6);
                    client6 = None;
                    note = format!("IPv6 数据呼叫没起来：{e}");
                }
            }
        }
        let settings = link.runtime(qmi::SVC_WDS, client4).unwrap_or_default();
        Ok((
            Self {
                link,
                client4,
                handle4,
                client6,
                handle6,
                wda_client,
                qmap,
                settings,
            },
            note,
        ))
    }

    /// 呼叫还活着吗？传输层出错返回 `None`（别因为一次读超时就重拨）。
    pub fn alive(&mut self) -> Option<bool> {
        self.link.connected(qmi::SVC_WDS, self.client4).ok()
    }

    /// 主动拆掉两个呼叫（重复调用安全）。
    pub fn stop(&mut self) {
        if let (Some(c), Some(h)) = (self.client6.take(), self.handle6.take()) {
            if let Err(e) = self.link.stop(qmi::SVC_WDS_IPV6, c, h) {
                log::debug!("原生 QMI：停 v6 失败: {e}");
            }
            if let Err(e) = self.link.release_client_id(qmi::SVC_WDS_IPV6, c) {
                log::debug!("原生 QMI：释放 v6 client 失败: {e}");
            }
        }
        if self.handle4 != 0 {
            let h = std::mem::take(&mut self.handle4);
            if let Err(e) = self.link.stop(qmi::SVC_WDS, self.client4, h) {
                log::debug!("原生 QMI：停 v4 失败: {e}");
            }
            if let Err(e) = self.link.release_client_id(qmi::SVC_WDS, self.client4) {
                log::debug!("原生 QMI：释放 v4 client 失败: {e}");
            }
        }
        // WDA client 最后还：数据格式是设备级状态，不需要「撤销」
        if let Some(w) = self.wda_client.take() {
            if let Err(e) = self.link.release_client_id(qmi::SVC_WDA, w) {
                log::debug!("原生 QMI：释放 WDA client 失败: {e}");
            }
        }
    }

    /// 状态字段（给 `/api/dial` 的 `host_dialer`）。
    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "handle4": self.handle4,
            "handle6": self.handle6,
            "mtu": self.settings.mtu,
            "modem_ipv4": self.settings.ipv4.map(|ip| ip.to_string()),
            "gateway": self.settings.gateway.map(|ip| ip.to_string()),
            "dns": self.settings.dns.iter().map(|ip| ip.to_string()).collect::<Vec<_>>(),
            "modem_ipv6": self.settings.ipv6.map(|(ip, p)| format!("{ip}/{p}")),
            "qmap": self.qmap.map(|q| serde_json::json!({
                "mux_id": q.mux_id,
                "qmap_version": q.qmap_version,
                "ep_type": q.ep_type,
                "rx_urb_size": q.rx_urb_size,
            })),
        })
    }
}

impl Drop for QmiSession {
    fn drop(&mut self) {
        self.stop();
    }
}
