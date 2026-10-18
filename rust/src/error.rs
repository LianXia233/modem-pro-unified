//! 统一错误类型别名。
//!
//! 骨架阶段不引入 anyhow/thiserror：直接用一个装箱 error，够用且零依赖
//! （依赖越少，OpenWrt 侧 cargo 拉取与交叉编译越省心）。

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, BoxError>;

/// 把任意可显示的字符串快速变成一个 error（用于 AT 层等纯文本错误）。
pub fn msg<T: Into<String>>(s: T) -> BoxError {
    s.into().into()
}
