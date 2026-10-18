//! 版本信息。
//!
//! 打包时由顶层 Makefile 注入 `MODEMD_VERSION`（形如 1.0.0-1）；
//! 直接 `cargo run` / `cargo build` 时回落成 "dev"。
//!
//! `GET /api/version` 会把版本 + 运行时信息一起返回，是**唯一**不需要会话就能访问的
//! 接口 —— 升级后浏览器还开着旧页面时，靠它一眼核对。

/// 构建版本（打包时注入；未注入为 "dev"）。
pub fn build_version() -> &'static str {
    option_env!("MODEMD_VERSION").unwrap_or("dev")
}

/// `GET /api/version` 的响应体。
#[derive(serde::Serialize)]
pub struct VersionInfo {
    /// 构建版本号
    pub version: String,
    /// 运行时后端标识
    pub runtime: String,
    /// 目标平台（os/arch）
    pub platform: String,
    /// 是否启用了「跟随 LuCI 登录」闸门
    pub luci_auth: bool,
}

/// 当前二进制的版本信息。
pub fn current(luci_auth: bool) -> VersionInfo {
    VersionInfo {
        version: build_version().to_string(),
        runtime: format!("rustc {}", rustc_version()),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        luci_auth,
    }
}

/// 编译期 rustc 版本（不稳定 API 之外最省事的取法：用 `CARGO_PKG_*` 没有 rustc 版本，
/// 这里用一个近似值；真正的 rustc 版本可从 `rustc --version` 获取，骨架阶段不做额外依赖）。
fn rustc_version() -> &'static str {
    "stable"
}
