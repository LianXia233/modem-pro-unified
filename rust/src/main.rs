//! 进程入口：只做最薄的转发，真正逻辑都在 `lib.rs` 的 [`modemd::run`] 与
//! `wanclient`（`modemd wan-client`）。

fn main() {
    match modemd::cli::command() {
        // netifd 拉起的子命令：退出码直接回给 netifd（非 0 即触发协议重启）
        modemd::cli::Command::WanClient => std::process::exit(modemd::wanclient::run()),
        modemd::cli::Command::Daemon { .. } => {
            if let Err(e) = modemd::run() {
                eprintln!("modemd 启动失败: {e}");
                std::process::exit(1);
            }
        }
    }
}
