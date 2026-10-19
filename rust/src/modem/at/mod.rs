//! AT 串口收发层。
//!
//! 拆分为：`port`（打开/配置串口）、`command`（发送 + 读终结符）、`parser`（应答解析）、
//! `urc`（主动上报的行级过滤，防 URC 混进命令应答）。
//! 骨架阶段只做「一问一答」的同步收发；`urc` 只负责「别让 URC 污染应答」，
//! 语义解释与事件化消费在 `model::urc`（折算成脏标记）与 `http::loops::spawn_urc`
//! （后台循环），不要再把解析逻辑塞回本层。

mod cache;
mod command;
mod cooldown;
mod pacing;
mod parser;
mod port;
mod presets;
mod sched;
mod urc;

pub use cache::Ttl;
pub use cooldown::{OpenCooldown, DEFAULT_COOLDOWN};
pub use pacing::{Pacing, DEFAULT_GAP};
pub use parser::{
    csv, error_line, extract_all, extract_field, field_part, is_duplicate_activate, is_ok, to_i32,
};
pub use port::AtPort;
pub use presets::{presets_for, AtPreset};
pub use sched::{Limits, Prio, SchedStats, Scheduler};
pub use urc::{classify_urc, LineGate, UrcKind, UrcLine};
