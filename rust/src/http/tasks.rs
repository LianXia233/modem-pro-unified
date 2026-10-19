//! 异步任务：把「要独占串口好几秒」的操作从 HTTP 线程里挪走。
//!
//! ## 为什么需要它
//!
//! modemd 的 HTTP 是「一请求一线程」，但串口只有一把锁（`Modem::with_at`）。
//! 于是扫小区（5~20s）、扫网（1~10min）、USSD、锁频段这些操作会：
//!
//! 1. 占住串口 → 同一模组的信号/网络采样全部排队；
//! 2. 占住一个 HTTP 连接 → 浏览器或代理超时，页面看不懂发生了什么；
//! 3. 用户重复点两次 → 排两个同样的任务，串口被排队占满。
//!
//! 所以慢操作统一走「提交 → 立即返回 task_id → 轮询」：HTTP 线程只做参数校验与
//! 投递（毫秒级），真正的活在工作线程里跑；同一 `kind+modem+参数` 只允许一个
//! 在跑（[`Registry::submit`] 去重），页面可以随时取消（协作式）。
//!
//! ## 状态机
//!
//! ```text
//! queued ──► running ──► done
//!    │          │  └──► failed
//!    └──────────┴──────► cancelled   （未开始就被取消 / 任务自己检查到取消）
//! ```
//!
//! ⚠️ 取消是**协作式**的：已经发给模组的 AT 指令没法撤回，但 AT 读取循环会在
//! 每个 100ms 检查一次（`AtPort::command_cancellable`），所以扫网/长指令能被打断。
//! 不检查取消的闭包（例如 `scan_cells`）会跑完再落结果，只是结果被标成 cancelled。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::error::Result;

/// 已完成任务保留多久（页面刷新/看历史用；过期即回收）。
const KEEP_DONE: Duration = Duration::from_secs(600);

/// 最多保留多少个任务（防止长时间运行后无限增长）。
const MAX_TASKS: usize = 64;

/// 任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    /// 还会变化的状态（前端据此决定要不要继续轮询）。
    pub fn is_active(self) -> bool {
        matches!(self, State::Queued | State::Running)
    }
}

/// 提交任务时的静态描述。
pub struct Spec {
    /// 任务种类（`cells.scan` / `net.scan` / `at` / …），见 `handlers::tasks`。
    pub kind: &'static str,
    pub modem_id: Option<u32>,
    /// 给页面看的一行说明。
    pub label: String,
    /// 去重键：同键且在跑 → 复用同一个任务（默认 = kind + modem_id）。
    pub key: String,
}

impl Spec {
    pub fn new(kind: &'static str, modem_id: Option<u32>, label: impl Into<String>) -> Self {
        let key = match modem_id {
            Some(id) => format!("{kind}#{id}"),
            None => format!("{kind}#-"),
        };
        Self {
            kind,
            modem_id,
            label: label.into(),
            key,
        }
    }

    /// 把参数并入去重键：同一种操作但参数不同（AT 指令、频段列表）应当各跑一个。
    pub fn with_key(mut self, extra: &str) -> Self {
        if !extra.is_empty() {
            self.key = format!("{}:{}", self.key, fnv1a(extra));
        }
        self
    }
}

/// 一个任务的内部可变部分。
struct Inner {
    state: State,
    started: Option<Instant>,
    ended: Option<Instant>,
    progress: String,
    result: Option<Value>,
    error: Option<String>,
}

/// 一个任务（`Arc` 共享：注册表 + 工作线程 + 查询接口同时持有）。
pub struct Task {
    pub id: String,
    pub kind: String,
    pub modem_id: Option<u32>,
    pub label: String,
    pub key: String,
    created: Instant,
    created_text: String,
    inner: Mutex<Inner>,
    cancel: AtomicBool,
}

impl Task {
    fn new(id: String, spec: &Spec) -> Self {
        Self {
            id,
            kind: spec.kind.to_string(),
            modem_id: spec.modem_id,
            label: spec.label.clone(),
            key: spec.key.clone(),
            created: Instant::now(),
            created_text: crate::logging::timestamp(),
            inner: Mutex::new(Inner {
                state: State::Queued,
                started: None,
                ended: None,
                progress: String::new(),
                result: None,
                error: None,
            }),
            cancel: AtomicBool::new(false),
        }
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state
    }

    pub fn is_active(&self) -> bool {
        self.state().is_active()
    }

    /// 协作式取消：闭包在处理过程中调用它决定要不要提前收手。
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 更新进度文案（页面显示「正在扫网… 3 分钟」这类）。
    pub fn progress(&self, text: impl Into<String>) {
        if let Ok(mut g) = self.inner.lock() {
            g.progress = text.into();
            // 已经开始跑说明从队列里出来了
            if g.started.is_none() {
                g.started = Some(Instant::now());
                g.state = State::Running;
            }
        }
    }

    fn run(&self) {
        if self.cancelled() {
            self.finish(State::Cancelled, None, None);
            return;
        }
        {
            let mut g = self.inner.lock().unwrap();
            g.state = State::Running;
            g.started = Some(Instant::now());
        }
    }

    fn finish(&self, state: State, result: Option<Value>, error: Option<String>) {
        let mut g = self.inner.lock().unwrap();
        // 已被取消的任务不再接受正常结果（避免「取消后忽然变成功」的错觉）
        if g.state == State::Cancelled && state != State::Cancelled {
            g.ended = Some(Instant::now());
            return;
        }
        g.state = state;
        g.ended = Some(Instant::now());
        g.result = result;
        g.error = error;
    }

    fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        let mut g = self.inner.lock().unwrap();
        if g.state.is_active() {
            // 还没开始跑的立刻落 cancelled；在跑的等闭包自己检查
            let started = g.started.is_some();
            g.state = State::Cancelled;
            if !started {
                g.ended = Some(Instant::now());
            }
        }
    }

    fn elapsed_ms(&self, g: &Inner) -> u64 {
        let end = g.ended.unwrap_or_else(Instant::now);
        let begin = g.started.unwrap_or(self.created);
        end.saturating_duration_since(begin).as_millis() as u64
    }

    /// 给页面的视图。
    pub fn view(&self) -> TaskView {
        let g = self.inner.lock().unwrap();
        TaskView {
            id: self.id.clone(),
            kind: self.kind.clone(),
            modem_id: self.modem_id,
            label: self.label.clone(),
            state: g.state.as_str().to_string(),
            progress: g.progress.clone(),
            created_at: self.created_text.clone(),
            elapsed_ms: self.elapsed_ms(&g),
            result: g.result.clone(),
            error: g.error.clone(),
        }
    }
}

/// 任务的对外表示（可直接 `serde_json::to_value`）。
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modem_id: Option<u32>,
    pub label: String,
    /// `queued` / `running` / `done` / `failed` / `cancelled`
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub progress: String,
    pub created_at: String,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 任务注册表（`AppState::tasks`）。
pub struct Registry {
    map: Mutex<BTreeMap<String, Arc<Task>>>,
    seq: AtomicU64,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(BTreeMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    /// 提交一个任务；同 `key` 已有在跑的任务时直接复用它（不重复占串口）。
    ///
    /// 返回的是最终会跑的那个任务（新建的或复用的），调用方据此回 `task_id`。
    pub fn submit<F>(&self, spec: Spec, run: F) -> Arc<Task>
    where
        F: FnOnce(Arc<Task>) -> Result<Value> + Send + 'static,
    {
        self.gc();

        // ① 去重：同键且在跑 → 直接复用
        {
            let map = self.map.lock().unwrap();
            if let Some(t) = map
                .values()
                .find(|t| t.key == spec.key && t.is_active())
                .cloned()
            {
                log::debug!(
                    "任务去重：{} 已在跑（{}），复用 {}",
                    spec.kind,
                    t.id,
                    spec.key
                );
                return t;
            }
        }

        let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("t{n}");
        let task = Arc::new(Task::new(id.clone(), &spec));
        self.map.lock().unwrap().insert(id.clone(), task.clone());

        let worker = task.clone();
        std::thread::spawn(move || {
            worker.run();
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(worker.clone())));
            match outcome {
                Ok(Ok(v)) => worker.finish(
                    if worker.cancelled() {
                        State::Cancelled
                    } else {
                        State::Done
                    },
                    Some(v),
                    None,
                ),
                Ok(Err(e)) => worker.finish(State::Failed, None, Some(e.to_string())),
                Err(_) => worker.finish(State::Failed, None, Some("任务线程 panic".to_string())),
            }
            log::debug!(
                "任务 {} 结束：{:?}（{}ms）",
                worker.id,
                worker.state(),
                worker.view().elapsed_ms
            );
        });

        task
    }

    pub fn get(&self, id: &str) -> Option<Arc<Task>> {
        self.map.lock().unwrap().get(id).cloned()
    }

    /// 列出任务（新的在前）；`active_only` 只要在跑的，`modem_id` 过滤模组。
    pub fn list(&self, modem_id: Option<u32>, active_only: bool) -> Vec<TaskView> {
        self.gc();
        let map = self.map.lock().unwrap();
        let mut out: Vec<TaskView> = map
            .values()
            .filter(|t| modem_id.map(|id| t.modem_id == Some(id)).unwrap_or(true))
            .filter(|t| !active_only || t.is_active())
            .map(|t| t.view())
            .collect();
        out.sort_by(|a, b| b.elapsed_ms.cmp(&a.elapsed_ms));
        out
    }

    /// 取消（协作式）。返回是否找到了这个任务。
    pub fn cancel(&self, id: &str) -> bool {
        match self.get(id) {
            Some(t) => {
                t.request_cancel();
                true
            }
            None => false,
        }
    }

    /// 回收：过期（已完成超过 10 分钟）或超出上限（保留最新的 64 个）时删除。
    fn gc(&self) {
        let mut map = self.map.lock().unwrap();
        map.retain(|_, t| {
            let g = t.inner.lock().unwrap();
            match (g.state, g.ended) {
                (State::Done | State::Failed | State::Cancelled, Some(end)) => {
                    end.elapsed() < KEEP_DONE
                }
                _ => true,
            }
        });
        while map.len() > MAX_TASKS {
            // BTreeMap 按 id 字典序，t1/t10/t2 不好比；按创建时间找最老的
            let oldest = map
                .iter()
                .filter(|(_, t)| !t.is_active())
                .min_by_key(|(_, t)| t.created)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    map.remove(&k);
                }
                None => break,
            }
        }
    }
}

/// FNV-1a：只用来把参数压成去重键，不用于安全场景。
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_key_includes_params() {
        let a = Spec::new("at", Some(0), "AT").with_key("AT+CSQ");
        let b = Spec::new("at", Some(0), "AT").with_key("AT+CIMI");
        assert_ne!(a.key, b.key, "不同指令不能互相去重");
        let c = Spec::new("at", Some(1), "AT").with_key("AT+CSQ");
        assert_ne!(a.key, c.key, "不同模组不能互相去重");
        // 同参数 → 同键（去重才生效）
        let d = Spec::new("at", Some(0), "AT").with_key("AT+CSQ");
        assert_eq!(a.key, d.key);
    }

    #[test]
    fn task_runs_and_reports_result() {
        let reg = Registry::new();
        let t = reg.submit(Spec::new("test", Some(0), "测试"), |task| {
            task.progress("一半");
            Ok(json!({ "n": 42 }))
        });
        for _ in 0..100 {
            if !t.is_active() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let v = t.view();
        assert_eq!(v.state, "done", "{v:?}");
        assert_eq!(v.result.unwrap()["n"], 42);
        assert!(!v.progress.is_empty());
    }

    #[test]
    fn duplicate_submit_reuses_running_task() {
        let reg = Registry::new();
        let t1 = reg.submit(Spec::new("slow", Some(0), "慢活"), |_| {
            std::thread::sleep(Duration::from_millis(120));
            Ok(Value::Null)
        });
        let t2 = reg.submit(Spec::new("slow", Some(0), "慢活"), |_| Ok(Value::Null));
        assert_eq!(t1.id, t2.id, "同键在跑必须复用，不能排两个占串口");
    }

    #[test]
    fn cancel_before_start_marks_cancelled() {
        let reg = Registry::new();
        let gate = Arc::new(Mutex::new(()));
        let t = reg.submit(Spec::new("gate", Some(0), "阻塞"), {
            let gate = gate.clone();
            move |_| {
                let _g = gate.lock().unwrap();
                Ok(Value::Null)
            }
        });
        assert!(reg.cancel(&t.id));
        for _ in 0..100 {
            if !t.is_active() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(t.state(), State::Cancelled);
    }

    #[test]
    fn failure_is_reported_with_message() {
        let reg = Registry::new();
        let t = reg.submit(Spec::new("bad", None, "失败"), |_| {
            Err(crate::error::msg("串口没了"))
        });
        for _ in 0..100 {
            if !t.is_active() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let v = t.view();
        assert_eq!(v.state, "failed");
        assert!(v.error.unwrap().contains("串口没了"));
    }

    #[test]
    fn list_filters_by_modem_and_active() {
        let reg = Registry::new();
        let gate = Arc::new(Mutex::new(()));
        // 先把闸门锁住，保证任务 A 在断言时还在跑（否则它瞬间完成就不算 active）
        let _hold = gate.lock().unwrap();
        let _a = reg.submit(Spec::new("hold", Some(0), "A"), {
            let gate = gate.clone();
            move |_| {
                let _g = gate.lock().unwrap();
                Ok(Value::Null)
            }
        });
        let _b = reg.submit(Spec::new("hold", Some(1), "B"), |_| Ok(Value::Null));
        assert!(!reg.list(Some(0), true).is_empty());
        assert!(!reg.list(None, true).is_empty());
    }
}
