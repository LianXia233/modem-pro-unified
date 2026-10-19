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

/// 编译期 rustc 版本（由 `build.rs` 执行 `rustc --version` 注入）。
///
/// 早先这里写死返回 `"stable"`，于是 `/api/version` 的 `runtime` 恒为
/// `rustc stable` —— 一个**看起来正常、实则无信息量**的假值，交叉编译链不一致时
/// 完全派不上用场。现在拿不到就是 `unknown`，不再拿假值充数。
fn rustc_version() -> &'static str {
    option_env!("MODEMD_RUSTC_VERSION").unwrap_or("unknown")
}
