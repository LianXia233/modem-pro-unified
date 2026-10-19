//! 编译期取 rustc 版本，供 `GET /api/version` 的 `runtime` 字段使用。
//!
//! ## 为什么要它
//!
//! `src/version.rs` 里原来写死了 `rustc stable` —— 那是个**假值**：任何一次编译出来的
//! 二进制都自称 "rustc stable"，排障时（交叉编译链不一致、云构建与本地构建行为不同）
//! 这个字段就完全没用了。作者当年的注释也承认是「骨架阶段的近似值」。
//!
//! 标准库没有暴露 rustc 版本号的稳定 API，所以只能在编译期问一次 `rustc --version`。
//! 这里刻意**不引第三方依赖**（`rustc_version` 之类）：本包要在 OpenWrt SDK 里交叉编译，
//! 依赖越少越省心，而取一个版本号用 `std::process::Command` 就够了。
//!
//! ## 失败怎么办
//!
//! 拿不到（PATH 里没有 rustc、`RUSTC` 指了个怪东西）就写 "unknown"，
//! **绝不退回写死一个看起来正常的版本号** —— 拿不到就该显示成拿不到。

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = std::process::Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    // `rustc --version` 形如 "rustc 1.77.2 (25ef9e3d8 2024-04-09)"。
    // 只取**第二段**（纯版本号）：第一段 "rustc" 由 version.rs 那边拼
    // （`format!("rustc {}", …)`），取了就会显示成 "rustc rustc 1.93.0"；
    // 后面的 commit 与日期对排障没用，还会带上发行版补丁后缀、显得很吵。
    let value = version
        .split_whitespace()
        .nth(1)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=MODEMD_RUSTC_VERSION={value}");
    // 只有 build.rs 自身变了才重跑：rustc 版本在一次构建里不会中途变化
    println!("cargo:rerun-if-changed=build.rs");
}
