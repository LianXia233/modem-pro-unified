//! 只读状态缓存：让「页面渲染」不再等价于「发一串 AT」。
//!
//! ## 为什么需要它
//!
//! 概览页一次要信号、网络、小区、锁网、拨号。这些接口原先各自发 AT，一个页面
//! 打开就是 5~8 条指令排队；多个标签页/多个人看更糟。而信号与网络本来就有
//! 后台采样循环（`http/loops.rs`），数据是现成的 —— 只是没有地方存。
//!
//! 本模块就是一个「按 (种类, 模组) 存最近一次结果 + 时间戳」的表：
//!
//! | 谁写 | 谁读 |
//! | --- | --- |
//! | 同步接口（`/api/signal` 等）成功返回时 | `GET /api/snapshot`（页面首屏） |
//! | 后台采样循环（信号/网络） | 同上 |
//! | 异步任务完成时（`cells.scan` / `network.refresh` / `lock.status`） | 同上 |
//!
//! 页面因此可以「先渲染上次的值 + 显示它的年龄」，需要新数据时提交一个任务，
//! 任务完成后再拉一次快照 —— 全程没有一条请求在等串口。
//!
//! ⚠️ 这里只存**只读观测值**。开关类状态（拨号开关、锁网目标）仍以 `config.toml`
//! 与模组为准，不能拿缓存去回答「现在允许漫游吗」这种问题。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde_json::Value;

/// 一类观测值在某个模组上的最近快照。
struct Entry {
    value: Value,
    at: Instant,
}

/// 状态缓存（`AppState::cache`）。
#[derive(Default)]
pub struct StateCache {
    inner: Mutex<HashMap<(&'static str, u32), Entry>>,
}

impl StateCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入一条（覆盖旧值）。
    pub fn put(&self, kind: &'static str, modem_id: u32, value: Value) {
        let mut g = self.inner.lock().unwrap();
        g.insert(
            (kind, modem_id),
            Entry {
                value,
                at: Instant::now(),
            },
        );
    }

    /// 读一条：`None` = 从没采过；`Some((值, 年龄毫秒))`。
    pub fn get(&self, kind: &'static str, modem_id: u32) -> Option<(Value, u64)> {
        let g = self.inner.lock().unwrap();
        g.get(&(kind, modem_id))
            .map(|e| (e.value.clone(), e.at.elapsed().as_millis() as u64))
    }

    /// 只要值（不关心年龄）。
    #[cfg(test)]
    pub fn value(&self, kind: &'static str, modem_id: u32) -> Option<Value> {
        self.get(kind, modem_id).map(|(v, _)| v)
    }

    /// 模组消失时清掉它的条目（拔卡后别拿着旧号码当现况）。
    /// 当前只有热插拔路径会调用；快照页不会读到已消失模组的条目（按 id 查），
    /// 这里保留成显式 API 是为了「模组被删」这个事实只表达一次。
    #[allow(dead_code)]
    pub fn forget(&self, modem_id: u32) {
        let mut g = self.inner.lock().unwrap();
        g.retain(|(_, id), _| *id != modem_id);
    }

    /// 全部种类（`/api/snapshot` 的 `ages` 字段用）。
    pub const KINDS: &'static [&'static str] = &[
        "signal", "network", "cells", "lock", "usbnet", "netscan", "dial",
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn put_then_get_returns_age() {
        let c = StateCache::new();
        assert!(c.get("signal", 0).is_none());
        c.put("signal", 0, json!({ "rssi": -70 }));
        let (v, age) = c.get("signal", 0).unwrap();
        assert_eq!(v["rssi"], -70);
        assert!(age < 1000);
        // 不同模组互不覆盖
        assert!(c.get("signal", 1).is_none());
        c.put("signal", 1, json!({ "rssi": -80 }));
        assert_eq!(c.value("signal", 1).unwrap()["rssi"], -80);
        assert_eq!(c.value("signal", 0).unwrap()["rssi"], -70);
    }

    #[test]
    fn forget_drops_only_that_modem() {
        let c = StateCache::new();
        c.put("network", 0, json!({ "apn": "a" }));
        c.put("network", 1, json!({ "apn": "b" }));
        c.forget(0);
        assert!(c.get("network", 0).is_none());
        assert!(c.get("network", 1).is_some());
    }
}
