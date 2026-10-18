//! 进程入口：只做最薄的转发，真正逻辑都在 `lib.rs` 的 [`modemd::run`]。

fn main() {
    if let Err(e) = modemd::run() {
        eprintln!("modemd 启动失败: {e}");
        std::process::exit(1);
    }
}
