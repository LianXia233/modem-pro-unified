//! netifd 接口拉起（原 `mp-client.sh`）的 Rust/Tokio 实现。
//!
//! # 与 shim 的职责边界
//!
//! | 层 | 位置 | 职责 |
//! |:---|:---|:---|
//! | proto handler | `files/lib/netifd/proto/modem_pro.sh` | 只做 netifd 契约：`proto_config_add_*`、拼 `uopts`/`oopts`、`proto_export`、`proto_run_command`、`teardown` |
//! | shim | `files/usr/lib/modem-pro/mp-client.sh` | 极薄转发：校验 `modemd` 存在 → `exec modemd wan-client`，不解析参数、不做重试 |
//! | daemon 子命令 | 本模块 | 上报脚本生成、udhcpc/odhcp6c 监管与重试、accept_ra、孤儿清理、退出清理 |
//!
//! # 为什么用 CLI 子命令而不是 unix socket
//!
//! `proto_run_command` 跟踪的是「它 exec 出来的那个进程」的 PID：协议 down 时 netifd 直接对这个
//! PID 发信号，超时后 SIGKILL。若把真正的客户端交给常驻 daemon 托管，shim 被 SIGKILL 后 daemon
//! 仍持有 udhcpc/odhcp6c，netifd 看不到也收不掉，正好复现 2026-10-13 那次「孤儿占着 wwan0 →
//! 雪崩重拨」的事故。
//!
//! 走 CLI 子命令时：进程树就是 netifd → modemd(wan-client) → udhcpc/odhcp6c，与旧 shell 完全一致，
//! netifd 的信号能直达；子命令退出码非 0 时 netifd 按协议重启；无需新增 socket 文件、无额外权限面、
//! 不需要考虑「daemon 未启动时排队」的语义。并发调用也天然隔离：一个接口一个子进程，
//! 上报脚本名里带 `INTERFACE`，互不影响。
//!
//! 唯一与旧实现的差异：accept_ra 的写入从 proto handler（shim）移到本子命令启动时，
//! 时序仍是「odhcp6c 发出第一个 RS 之前」，见 [`sysctl`]。

mod env;
mod log;
mod procscan;
mod script;
mod signals;
mod supervisor;
mod sysctl;

/// 由 `main.rs` 在 `modemd wan-client` 时调用；返回进程退出码。
pub fn run() -> i32 {
    let params = match env::Params::from_process() {
        Ok(p) => p,
        Err(e) => {
            log::line(&format!("mp-client: {e}"));
            return 1;
        }
    };

    // current_thread 足够：本子命令只监管 2 个外部进程 + 每 3 秒巡检一次，
    // 不需要多线程运行时的开销（OpenWrt 设备普遍是 1~2 核）。
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::line(&format!("mp-client: 创建 Tokio 运行时失败（{e}）"));
            return 1;
        }
    };

    rt.block_on(supervisor::run(params))
}
