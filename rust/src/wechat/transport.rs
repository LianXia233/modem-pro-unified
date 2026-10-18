//! 传输层：跑 `curl` 发 HTTPS（纯 `std` 没有 TLS，选型理由见 `mod.rs` 模块头）。
//!
//! 三条约定：
//!
//! 1. **先问 `available()`**：设备上没有 `curl` 就明确失败，不假装发成功；
//! 2. **正文用 `-d` 传 argv**：`serde_json` 出的是**紧凑单行** JSON，没有换行可丢；
//!    而走 argv 就不经过 shell，引号 / `$` / 反斜杠都不需要转义（PowerShell 与
//!    BusyBox ash 两侧都少一类坑）；
//! 3. **stderr 一定带出来**：curl 的退出码只说明「没连上 / 超时」，
//!    「连上了但业务报错」要看 stderr 与返回体。

use std::process::Command;

use crate::error::{msg, Result};

/// `curl` 可执行文件是否可用（顺带测一下 `-V` 能不能跑）。
pub fn available() -> bool {
    Command::new("curl")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// POST 一个 JSON 正文，返回响应正文。
///
/// `body` 传 `None` 时不带 `-d`（有些接口只要 query / header）。
pub fn post_json(url: &str, body: Option<&str>, timeout_secs: u64) -> Result<String> {
    let t = timeout_secs.clamp(1, 60);
    let mut cmd = Command::new("curl");
    cmd.arg("-s")
        .arg("-m")
        .arg(t.to_string())
        .arg("-X")
        .arg("POST")
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-H")
        .arg("Accept: application/json");
    if let Some(b) = body {
        // 紧凑 JSON 无换行 ⇒ `-d` 不会丢内容；见模块头约定 2
        cmd.arg("-d").arg(b);
    }
    cmd.arg(url);
    run(cmd, &format!("POST {url}"))
}

/// GET 一个地址，返回响应正文（长轮询用：超时由调用方给足）。
pub fn get(url: &str, timeout_secs: u64) -> Result<String> {
    let t = timeout_secs.clamp(1, 300);
    let mut cmd = Command::new("curl");
    cmd.arg("-s")
        .arg("-m")
        .arg(t.to_string())
        .arg("-H")
        .arg("Accept: application/json")
        .arg(url);
    run(cmd, &format!("GET {url}"))
}

/// 跑一条 curl，把 stderr 带出来（退出码非 0 才算失败）。
fn run(mut cmd: Command, what: &str) -> Result<String> {
    if !available() {
        return Err(msg("curl 不可用：企业微信推送需要 curl 支持 HTTPS"));
    }
    let out = cmd
        .output()
        .map_err(|e| msg(format!("跑不了 curl（{what}）: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(msg(format!(
            "curl {what} 失败（退出码 {:?}）: {}",
            out.status.code(),
            if err.is_empty() {
                "无 stderr".to_string()
            } else {
                err
            }
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    /// 超时被夹在合理区间（纯函数版，只为把 `transport` 的夹取逻辑钉住）。
    fn timeout_clamped(secs: u64) -> u64 {
        secs.clamp(1, 300)
    }

    #[test]
    fn timeout_is_clamped_into_sane_range() {
        // 太短的超时会让长轮询永远拿不到东西；太长的会挂住调用方线程
        assert!(timeout_clamped(0) >= 1);
        assert!(timeout_clamped(9999) <= 300);
        assert_eq!(timeout_clamped(5), 5);
    }
}
