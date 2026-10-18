//! 退出信号处理（Ctrl-C / SIGTERM）。

/// 注册退出钩子；注册失败不致命（可能已在别处装过处理器）。
pub fn install<F>(on_exit: F)
where
    F: Fn() + Send + 'static,
{
    if ctrlc::set_handler(on_exit).is_err() {
        log::warn!("安装退出信号处理器失败（可能已存在），将不响应 Ctrl-C/SIGTERM");
    }
}
