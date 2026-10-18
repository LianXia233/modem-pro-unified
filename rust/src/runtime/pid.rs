//! PID 文件读写（失败不阻断启动）。

/// 写 PID 文件。
pub fn write(path: &str) {
    if path.trim().is_empty() {
        return;
    }
    if let Err(e) = std::fs::write(path, std::process::id().to_string()) {
        log::warn!("写 PID 文件 {path} 失败: {e}");
    }
}

/// 删除 PID 文件。
pub fn remove(path: &str) {
    if !path.trim().is_empty() {
        let _ = std::fs::remove_file(path);
    }
}
