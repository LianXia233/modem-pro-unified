//! 客户端监管状态机：拉起 udhcpc / odhcp6c、每 3 秒巡检、连续失败 5 次整体退出。
//!
//! 与旧 `mp-client.sh` 的循环逐条对应（见 README 的行为对照表）：
//!
//! | 旧 shell | 此处 |
//! |:---|:---|
//! | `start_udhcpc` / `start_odhcp6c` | [`Supervisor::start_udhcpc`] / [`Supervisor::start_odhcp6c`] |
//! | `while [ "$STOPPING" = 0 ]; sleep $POLL; kill -0 ...` | `interval_at(now + 3s)` + `try_wait` |
//! | `UFAIL/OFAIL` 计数与 `MAX_FAILS` | [`FailCounter`] |
//! | `cleanup`（kill / sleep 1 / kill -9 / cleanup_stale / rm 脚本） | [`Supervisor::cleanup`] |
//! | `trap 'cleanup; exit 0' TERM INT HUP` | [`super::signals`] + `select!` |

use std::path::Path;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::time::{interval_at, Instant};

use super::env::Params;
use super::log;
use super::procscan;
use super::script;

/// 单个客户端连续退出这么多次才整体退出（交给 netifd 重启协议）。
const MAX_FAILS: u32 = 5;
/// 巡检间隔（秒）。
const POLL_SECS: u64 = 3;
/// 官方上报脚本路径（运行时读取，确保跟 netifd 一起升级）。
const V4_SRC: &str = "/lib/netifd/dhcp.script";
const V6_SRC: &str = "/lib/netifd/dhcpv6.script";

/// 单次巡检结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    /// 还活着（失败计数清零）。
    Alive,
    /// 已退出，但没到上限 —— 立刻重新拉起。
    Restart,
    /// 连续失败到上限 —— 整体退出，让 netifd 重启协议。
    GiveUp,
}

/// 单客户端连续失败计数。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FailCounter {
    count: u32,
}

impl FailCounter {
    pub fn count(&self) -> u32 {
        self.count
    }

    /// 巡检发现进程活着。
    pub fn on_alive(&mut self) -> PollOutcome {
        self.count = 0;
        PollOutcome::Alive
    }

    /// 巡检发现进程已退出。
    pub fn on_exited(&mut self, max: u32) -> PollOutcome {
        self.count += 1;
        if self.count >= max {
            PollOutcome::GiveUp
        } else {
            PollOutcome::Restart
        }
    }
}

/// udhcpc 的命令行（参数顺序与旧 shim 完全一致）。
pub fn udhcpc_argv(p: &Params) -> Vec<String> {
    let mut args = vec![
        "-i".to_string(),
        p.iface.clone(),
        "-s".to_string(),
        p.v4_script_path(),
        "-f".to_string(),
        "-R".to_string(),
        "-p".to_string(),
        p.udhcpc_pidfile(),
    ];
    args.extend(p.uopts.iter().cloned());
    args
}

/// odhcp6c 的命令行（`-P0` 在附加参数之后、网卡名之前，与旧 shim 一致）。
pub fn odhcp6c_argv(p: &Params) -> Vec<String> {
    let mut args = vec!["-s".to_string(), p.v6_script_path()];
    args.extend(p.oopts.iter().cloned());
    args.push("-P0".to_string());
    args.push(p.iface.clone());
    args
}

pub async fn run(params: Params) -> i32 {
    // 0) 注册退出信号
    //
    // **必须先于任何子进程拉起**：netifd 只对它跟踪的那个 PID 发 TERM，进程若还没装着
    // 信号处理，TERM 会按默认动作直接终止 modemd，`Supervisor::cleanup` 不会执行，
    // udhcpc/odhcp6c 就成了占着网卡的孤儿（旧 shim 的 `trap` 也是挂在拉起客户端之前）。
    // 注册之后即使 TERM 落在 `sleep(1s)` 之类未 poll 的窗口里，tokio 也会把它排队，
    // 回到 `select!` 时立刻走清理路径。
    let mut sig = super::signals::Signals::new();

    // 1) accept_ra 前置：必须在 odhcp6c 发第一个 RS 之前完成
    if params.fix_accept_ra {
        super::sysctl::fix_accept_ra(&params.iface);
    }

    log::line(&format!(
        "mp-client: 启动 {}（设备 {}）",
        params.interface, params.iface
    ));

    // 2) 清掉同接口的残留客户端（孤儿占着网卡会让新实例收不到 RA）
    let stale = procscan::cleanup_stale(&params.interface);
    if stale > 0 {
        log::line(&format!(
            "mp-client: 清掉 {stale} 个残留客户端（避免孤儿占着 {}）",
            params.iface
        ));
    }

    let mut sup = Supervisor::new(params);

    // 3) 生成带 proto_set_keep 的上报脚本
    let v4 = sup.params().v4_script_path();
    if let Err(e) = script::generate(V4_SRC, Path::new(&v4)) {
        log::line(&format!("mp-client: 生成 v4 上报脚本失败（{e}）"));
        sup.cleanup().await;
        return 1;
    }

    let v6 = sup.params().v6_script_path();
    if sup.params().no_v6 {
        let _ = std::fs::remove_file(&v6);
        log::line(&format!(
            "mp-client: {} 只跑 udhcpc（mp_no_v6=1，IPv6 由独立接口负责）",
            sup.params().interface
        ));
    } else if let Err(e) = script::generate(V6_SRC, Path::new(&v6)) {
        log::line(&format!("mp-client: 生成 v6 上报脚本失败（{e}）"));
        sup.cleanup().await;
        return 1;
    }

    // 4) 拉起客户端（v6 晚 1 秒，与旧 shim 的 sleep 1 对齐）
    sup.start_udhcpc();
    if !sup.params().no_v6 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        sup.start_odhcp6c();
    }

    // 5) 巡检 + 退出信号（信号已在步骤 0 注册）
    let mut ticker = interval_at(
        Instant::now() + Duration::from_secs(POLL_SECS),
        Duration::from_secs(POLL_SECS),
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let code = loop {
        tokio::select! {
            _ = sig.recv() => {
                log::line("mp-client: 收到退出信号，清理客户端");
                break 0;
            }
            _ = ticker.tick() => {
                if let Some(code) = sup.poll().await {
                    break code;
                }
            }
        }
    };

    // 6) 退出清理（原 EXIT trap）
    sup.cleanup().await;
    code
}

struct Supervisor {
    params: Params,
    udhcpc: Option<Child>,
    odhcp6c: Option<Child>,
    ufail: FailCounter,
    ofail: FailCounter,
}

impl Supervisor {
    fn new(params: Params) -> Self {
        Self {
            params,
            udhcpc: None,
            odhcp6c: None,
            ufail: FailCounter::default(),
            ofail: FailCounter::default(),
        }
    }

    fn params(&self) -> &Params {
        &self.params
    }

    fn start_udhcpc(&mut self) {
        let mut cmd = Command::new("udhcpc");
        cmd.args(udhcpc_argv(&self.params)).kill_on_drop(true);
        match cmd.spawn() {
            Ok(c) => {
                log::line(&format!(
                    "mp-client: udhcpc 已拉起 (pid {})",
                    c.id().map(|p| p.to_string()).unwrap_or_default()
                ));
                self.udhcpc = Some(c);
            }
            Err(e) => {
                // 等价于旧 shim 里 fork 成功但 exec 失败：由巡检计数把它算成一次退出
                log::line(&format!("mp-client: udhcpc 拉起失败（{e}）"));
                self.udhcpc = None;
            }
        }
    }

    fn start_odhcp6c(&mut self) {
        let mut cmd = Command::new("odhcp6c");
        cmd.args(odhcp6c_argv(&self.params)).kill_on_drop(true);
        match cmd.spawn() {
            Ok(c) => {
                log::line(&format!(
                    "mp-client: odhcp6c 已拉起 (pid {})",
                    c.id().map(|p| p.to_string()).unwrap_or_default()
                ));
                self.odhcp6c = Some(c);
            }
            Err(e) => {
                log::line(&format!("mp-client: odhcp6c 拉起失败（{e}）"));
                self.odhcp6c = None;
            }
        }
    }

    /// 每 3 秒调用一次。返回 `Some(exit_code)` 表示需要整体退出。
    async fn poll(&mut self) -> Option<i32> {
        match self.check_alive(Counter::U) {
            PollOutcome::Alive => {}
            PollOutcome::Restart => self.start_udhcpc(),
            PollOutcome::GiveUp => {
                log::line(&format!(
                    "mp-client: udhcpc 连续退出 {} 次 → 整体重启协议（交给 netifd）",
                    self.ufail.count()
                ));
                return Some(1);
            }
        }

        if self.params.no_v6 {
            return None;
        }

        match self.check_alive(Counter::O) {
            PollOutcome::Alive => {}
            PollOutcome::Restart => self.start_odhcp6c(),
            PollOutcome::GiveUp => {
                log::line(&format!(
                    "mp-client: odhcp6c 连续退出 {} 次 → 整体重启协议（交给 netifd）",
                    self.ofail.count()
                ));
                return Some(1);
            }
        }
        None
    }

    /// 用 `try_wait` 取代旧 shim 的 `kill -0`：子进程已退出时 `try_wait` 会顺带回收，
    /// 不会像 `kill -0` 那样在子进程变僵尸后仍报「活着」。
    fn check_alive(&mut self, which: Counter) -> PollOutcome {
        let exhausted = {
            let slot = match which {
                Counter::U => &mut self.udhcpc,
                Counter::O => &mut self.odhcp6c,
            };
            match slot {
                Some(c) => match c.try_wait() {
                    Ok(None) => false,
                    // 已退出或 try_wait 出错，一律按「退出一次」处理
                    Ok(Some(_)) | Err(_) => true,
                },
                // 从没拉起来过（spawn 失败）：也算一次退出
                None => true,
            }
        };

        let counter = match which {
            Counter::U => &mut self.ufail,
            Counter::O => &mut self.ofail,
        };

        if !exhausted {
            return counter.on_alive();
        }

        let outcome = counter.on_exited(MAX_FAILS);
        if outcome == PollOutcome::Restart {
            let n = counter.count();
            let name = match which {
                Counter::U => "udhcpc",
                Counter::O => "odhcp6c",
            };
            log::line(&format!("mp-client: {name} 退出，重新拉起（第 {n} 次）"));
        }
        outcome
    }

    /// 退出清理：TERM → 等 1 秒 → KILL → 扫残留 → 删上报脚本。
    async fn cleanup(&mut self) {
        self.signal_children(false);
        tokio::time::sleep(Duration::from_secs(1)).await;
        self.signal_children(true);

        let stale = procscan::cleanup_stale(&self.params.interface);
        if stale > 0 {
            log::line(&format!(
                "mp-client: 清掉 {stale} 个残留客户端（避免孤儿占着 {}）",
                self.params.iface
            ));
        }

        let _ = std::fs::remove_file(self.params.v4_script_path());
        let _ = std::fs::remove_file(self.params.v6_script_path());
    }

    fn signal_children(&mut self, hard: bool) {
        for child in [self.udhcpc.as_mut(), self.odhcp6c.as_mut()]
            .into_iter()
            .flatten()
        {
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    if hard {
                        procscan::kill9(pid);
                    } else {
                        procscan::terminate(pid);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = hard;
                let _ = child.start_kill();
            }
        }
    }
}

/// `check_alive` 需要知道该把这次结论记到哪个计数器上。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Counter {
    U,
    O,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(no_v6: bool) -> Params {
        Params {
            interface: "modem1".to_string(),
            iface: "wwan0".to_string(),
            uopts: vec![
                "-B".to_string(),
                "-x".to_string(),
                "hostname:router".to_string(),
            ],
            oopts: vec!["-t120".to_string()],
            no_v6,
            fix_accept_ra: true,
        }
    }

    #[test]
    fn counter_restarts_then_gives_up_at_max() {
        let mut c = FailCounter::default();
        for i in 1..MAX_FAILS {
            assert_eq!(c.on_exited(MAX_FAILS), PollOutcome::Restart);
            assert_eq!(c.count(), i);
        }
        assert_eq!(c.on_exited(MAX_FAILS), PollOutcome::GiveUp);
        assert_eq!(c.count(), MAX_FAILS);
    }

    #[test]
    fn counter_resets_on_alive() {
        let mut c = FailCounter::default();
        c.on_exited(MAX_FAILS);
        c.on_exited(MAX_FAILS);
        assert_eq!(c.on_alive(), PollOutcome::Alive);
        assert_eq!(c.count(), 0);
    }

    #[test]
    fn udhcpc_args_match_old_shim() {
        assert_eq!(
            udhcpc_argv(&params(false)),
            vec![
                "-i",
                "wwan0",
                "-s",
                "/tmp/mp-dhcp-modem1.script",
                "-f",
                "-R",
                "-p",
                "/var/run/mp-udhcpc-modem1.pid",
                "-B",
                "-x",
                "hostname:router",
            ]
        );
    }

    #[test]
    fn odhcp6c_args_put_p0_after_user_opts() {
        assert_eq!(
            odhcp6c_argv(&params(false)),
            vec![
                "-s",
                "/tmp/mp-dhcpv6-modem1.script",
                "-t120",
                "-P0",
                "wwan0",
            ]
        );
        // uopts 不应混进 odhcp6c 的参数里
        assert!(!odhcp6c_argv(&params(false)).contains(&"-B".to_string()));
    }
}
