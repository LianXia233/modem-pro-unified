//! 退出信号：TERM / INT / HUP（与旧 shim 的 `trap 'cleanup; exit 0' TERM INT HUP` 对齐）。
//!
//! netifd 收尾先发 TERM，超时才 SIGKILL；捕获 TERM 是为了在 SIGKILL 之前把
//! udhcpc/odhcp6c 收干净，不留孤儿。

pub struct Signals {
    #[cfg(unix)]
    inner: Option<UnixSignals>,
}

impl Signals {
    pub fn new() -> Self {
        Self {
            #[cfg(unix)]
            inner: UnixSignals::new(),
        }
    }

    /// 阻塞到收到任一退出信号。注册失败时永不返回（不出错、也不误触发清理）。
    pub async fn recv(&mut self) {
        #[cfg(unix)]
        {
            if let Some(s) = self.inner.as_mut() {
                s.recv().await;
                return;
            }
            std::future::pending::<()>().await;
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(unix)]
struct UnixSignals {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl UnixSignals {
    fn new() -> Option<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Some(Self {
            term: signal(SignalKind::terminate()).ok()?,
            int: signal(SignalKind::interrupt()).ok()?,
            hup: signal(SignalKind::hangup()).ok()?,
        })
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.term.recv() => {}
            _ = self.int.recv() => {}
            _ = self.hup.recv() => {}
        }
    }
}
