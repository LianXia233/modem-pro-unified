//! 冷门查询的 TTL 缓存。
//!
//! ## 为什么要单独一层
//!
//! 串口是**独占**资源，而且很慢（一条 `AT+CGDCONT?` 在 FM350 上要 120~200ms）。
//! 页面每打开一次设置页就问一遍 APN / PDP / 激活态，来回就是三条命令；
//! 加上概览页 3 秒一轮的信号采样，串口几乎没有空闲。这些「冷门」数据的特点是：
//!
//! - 变化极慢：APN 改了才变，USB 网卡模式改了才变，硬件型号开机后就不变；
//! - 问得却不慢：每次进页面都问。
//!
//! 所以给它们一层短 TTL 缓存，命中就不发 AT。这与 [`super::sched`] 是**两个正交**的层次：
//! 调度管「谁先发」，缓存管「根本不用发」。
//!
//! ## 三条纪律（踩过坑，写在代码里）
//!
//! 1. **失败绝不入缓存**。串口偶发超时很常见，如果把 `Err` 也钉住 30 秒，
//!    用户会看到「明明插着卡，却连续半分钟报错」。只有 `Ok` 才回填。
//! 2. **写操作必须 [`Ttl::invalidate`]**。改了 APN 却还读旧值，比慢更糟 —— 那是错的。
//!    每个写路径的末尾都要清掉对应缓存，见各 `model/*.rs` 的写函数。
//! 3. **要能强制刷新**。页面上的「刷新」按钮必须真的重新问模组，
//!    所以读取入口都保留 `refresh` 参数直接绕过缓存。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 带过期时间的单值缓存。
pub struct Ttl<T> {
    slot: Mutex<Option<(Instant, T)>>,
    ttl: Duration,
}

impl<T: Clone> Ttl<T> {
    pub const fn new(ttl: Duration) -> Self {
        Self {
            slot: Mutex::new(None),
            ttl,
        }
    }

    /// 命中（且未过期）返回值。
    pub fn get(&self) -> Option<T> {
        let g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_ref() {
            Some((at, v)) if at.elapsed() < self.ttl => Some(v.clone()),
            _ => None,
        }
    }

    /// 主动回填（写操作之后想「顺手把新值塞进去」时用）。
    pub fn set(&self, v: T) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), v));
    }

    /// 作废（写操作改动了底层状态时调）。
    pub fn invalidate(&self) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// 命中就返回；未命中则跑 `f`，**成功**才回填。
    ///
    /// `f` 在锁外执行 —— 它要发 AT，可能阻塞几百毫秒，
    /// 持锁跑会让 `invalidate()`（写路径）一起卡住。
    /// 代价是并发两个读者可能都跑一次 `f`：可接受，串口那边本来就排队。
    pub fn get_or_try<F, E>(&self, f: F) -> std::result::Result<T, E>
    where
        F: FnOnce() -> std::result::Result<T, E>,
    {
        if let Some(v) = self.get() {
            return Ok(v);
        }
        let v = f()?;
        self.set(v.clone());
        Ok(v)
    }
}

impl<T> Ttl<T> {
    /// 当前缓存年龄（秒）；`None` = 没有缓存。
    ///
    /// 给接口展示「这份数据是多久前问的」，用户能判断要不要点刷新。
    pub fn age_secs(&self) -> Option<u64> {
        let g = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().map(|(at, _)| at.elapsed().as_secs())
    }

    /// 当前 TTL（秒），给接口/日志展示。
    pub fn ttl_secs(&self) -> u64 {
        self.ttl.as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_within_ttl() {
        let c: Ttl<u32> = Ttl::new(Duration::from_secs(10));
        assert_eq!(c.get_or_try(|| Ok::<_, ()>(7)).unwrap(), 7);
        // 第二次不该再跑闭包
        let mut ran = false;
        let v = c
            .get_or_try(|| {
                ran = true;
                Ok::<_, ()>(99)
            })
            .unwrap();
        assert_eq!(v, 7);
        assert!(!ran, "TTL 内不该重复问模组");
    }

    #[test]
    fn error_is_not_cached() {
        // 关键纪律：一次串口超时不能把错误钉住整个 TTL
        let c: Ttl<u32> = Ttl::new(Duration::from_secs(10));
        assert!(c.get_or_try(|| Err::<u32, &str>("串口超时")).is_err());
        assert_eq!(c.get(), None, "失败后不该留下缓存");
        // 紧接着重试应该真的再问一次，并且能成功
        assert_eq!(c.get_or_try(|| Ok::<u32, &str>(5)).unwrap(), 5);
    }

    #[test]
    fn invalidate_forces_refetch() {
        let c: Ttl<u32> = Ttl::new(Duration::from_secs(60));
        c.set(1);
        assert_eq!(c.get(), Some(1));
        c.invalidate();
        assert_eq!(c.get(), None);
        assert_eq!(c.get_or_try(|| Ok::<_, ()>(2)).unwrap(), 2);
    }

    #[test]
    fn expires_after_ttl() {
        let c: Ttl<u32> = Ttl::new(Duration::from_millis(30));
        c.set(1);
        assert_eq!(c.get(), Some(1));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.get(), None);
    }

    #[test]
    fn age_is_reported() {
        let c: Ttl<u32> = Ttl::new(Duration::from_secs(60));
        assert_eq!(c.age_secs(), None);
        c.set(1);
        assert_eq!(c.age_secs(), Some(0));
        assert_eq!(c.ttl_secs(), 60);
    }
}
