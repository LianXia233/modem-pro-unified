//! URC 消费端：把过滤层暂存的主动上报变成「该做什么」。
//!
//! ## 在整体里的位置
//!
//! [`crate::modem::at::urc`] 只做**过滤与暂存**（判定 + 队列），语义解释在这里：
//! 一批 [`UrcLine`] 进来，出来一份 [`UrcAction`]（该置哪些脏标记、有哪些待推送的
//! 内容）。纯函数、无 IO —— 应用动作由调用方（`http::loops::spawn_urc`）执行，
//! 单测直接打本模块。
//!
//! ## 为什么只做「置脏」而不直接改状态
//!
//! 单字段注册态 URC（`+CEREG: 5`）与 `AT+CEREG?` 在 n=0 时的应答**完全同形**
//! （见 `at/urc.rs` 的说明），它的值不足以推定当前状态；把它写进缓存等于用
//! 不可信数据覆盖可信数据。正确做法是拿它当「事情变了」的信号，让既有的
//! 权威查询路径去重读 —— 这就是本模块对注册态只产脏标记的原因。
//! `+CUSD` / `+CMTI` 例外：它们携带的内容**没有对应的查询路径**可以重读
//! （USSD 应答只在 URC 里来一次，新短信的索引也只有模组知道），所以原文照搬，
//! 由消费端解析。
//!
//! ## 每类的取证程度（加动作前先看这张表）
//!
//! | 类别 | 动作 | 依据 |
//! | --- | --- | --- |
//! | `GtSim` | SIM 置脏 | 广和通 MTK 的 SIM 状态上报；误判代价仅多读一次 `AT+CPIN?`（身份以 ICCID/IMSI 为准，还有 30 秒冷却），不会误动作 |
//! | `Reg` | 网络置脏 | 注册态变化 ⇒ `/api/network` 的缓存过期；重查节拍仍受推送循环限制（最快 1 秒一次） |
//! | `Cusd` | 原文交出去 | USSD 应答只在 URC 里到达，无从重读；消费端解析后写缓存并推送 |
//! | `SmsNew` | 短信缓存置脏 | `+CMTI` 是新短信到达；索引本身不够用，要重读列表才知道内容 |
//! | 其它 | 无（仅日志） | 手册与参考实现均无语义记载，不猜 |

use crate::modem::at::{UrcKind, UrcLine};

/// 一批 URC 的处置结论。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrcAction {
    /// 该去核实 SIM 身份了（拔卡 / 插卡 / 换卡能立刻发现，不用等轮询周期）。
    pub sim_dirty: bool,
    /// 该重查网络状态了（注册态变了，界面上的注册 / 运营商信息过期）。
    pub net_dirty: bool,
    /// 待处理的 `+CUSD:` 原文（按到达顺序；消费端解析、写缓存、推送）。
    pub cusd: Vec<String>,
    /// 短信缓存过期了（`+CMTI`：来了新短信，重读列表才知道内容）。
    pub sms_dirty: bool,
}

impl UrcAction {
    /// 有没有要做的动作（空动作就没必要惊动巡检）。
    pub fn any(&self) -> bool {
        self.sim_dirty || self.net_dirty || !self.cusd.is_empty() || self.sms_dirty
    }
}

/// 把一批 URC 折算成动作（纯函数）。
pub fn urc_actions(lines: &[UrcLine]) -> UrcAction {
    let mut act = UrcAction::default();
    for u in lines {
        match u.kind {
            UrcKind::GtSim => act.sim_dirty = true,
            UrcKind::Reg => act.net_dirty = true,
            UrcKind::Cusd => act.cusd.push(u.line.clone()),
            UrcKind::SmsNew => act.sms_dirty = true,
            _ => {}
        }
    }
    act
}

/// 一类的计数与首条原文（巡检日志用：既能看到发生了什么，又不被洪水刷屏）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindStat {
    pub kind: UrcKind,
    pub count: usize,
    pub first: String,
}

/// 按类别统计一批 URC（保持首次出现顺序）。
pub fn kind_stats(lines: &[UrcLine]) -> Vec<KindStat> {
    let mut out: Vec<KindStat> = Vec::new();
    for u in lines {
        if let Some(s) = out.iter_mut().find(|s| s.kind == u.kind) {
            s.count += 1;
        } else {
            out.push(KindStat {
                kind: u.kind,
                count: 1,
                first: u.line.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modem::at::UrcKind as K;

    fn line(kind: UrcKind, text: &str) -> UrcLine {
        UrcLine {
            line: text.to_string(),
            kind,
        }
    }

    #[test]
    fn sim_urc_marks_sim_dirty() {
        let act = urc_actions(&[line(K::GtSim, "+GTSIM: 1")]);
        assert!(act.sim_dirty);
        assert!(!act.net_dirty);
        assert!(act.any());
    }

    #[test]
    fn reg_urc_marks_network_dirty() {
        for kind_line in ["+CEREG: 5", "+CGREG: 1", "+CREG: 3"] {
            let act = urc_actions(&[line(K::Reg, kind_line)]);
            assert!(act.net_dirty, "{kind_line} 应触发网络重查");
            assert!(!act.sim_dirty);
        }
    }

    #[test]
    fn unclassified_kinds_do_nothing() {
        // 手册与参考实现都没有语义记载的类别：只记录，不动作
        for k in [
            K::Ver,
            K::GtStatus,
            K::GtRgc,
            K::GtUsbmode,
            K::GtSrv,
            K::GtPin,
            K::GtDss,
            K::GtCellInfo,
        ] {
            let act = urc_actions(&[line(k, "+X: 1")]);
            assert!(!act.any(), "{k:?} 不该产生动作");
        }
    }

    #[test]
    fn empty_batch_is_noop() {
        assert!(!urc_actions(&[]).any());
    }

    #[test]
    fn mixed_batch_sets_both_flags() {
        let act = urc_actions(&[
            line(K::GtStatus, "+GTSTATUS: 1,1"),
            line(K::GtSim, "+GTSIM: 1"),
            line(K::Reg, "+CEREG: 5"),
        ]);
        assert!(act.sim_dirty && act.net_dirty);
    }

    #[test]
    fn stats_group_by_kind_keep_first_line() {
        let stats = kind_stats(&[
            line(K::Reg, "+CEREG: 5"),
            line(K::GtSim, "+GTSIM: 1"),
            line(K::Reg, "+CGREG: 1"),
            line(K::GtSim, "+GTSIM: 0"),
        ]);
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].kind, K::Reg);
        assert_eq!(stats[0].count, 2);
        assert_eq!(stats[0].first, "+CEREG: 5");
        assert_eq!(stats[1].kind, K::GtSim);
        assert_eq!(stats[1].count, 2);
        assert_eq!(stats[1].first, "+GTSIM: 1");
    }

    // ---------- USSD / 新短信 ----------

    #[test]
    fn cusd_lines_are_handed_over_in_order() {
        // USSD 应答没有可重读的查询路径，只能原文照搬
        let act = urc_actions(&[
            line(K::Cusd, "+CUSD: 0,\"余额 12.5 元\",15"),
            line(K::Cusd, "+CUSD: 2"),
        ]);
        assert_eq!(
            act.cusd,
            vec![
                "+CUSD: 0,\"余额 12.5 元\",15".to_string(),
                "+CUSD: 2".to_string()
            ]
        );
        assert!(act.any());
    }

    #[test]
    fn cmti_marks_sms_dirty() {
        for l in ["+CMTI: \"SM\",3", "+CMTI: \"ME\",12"] {
            let act = urc_actions(&[line(K::SmsNew, l)]);
            assert!(act.sms_dirty, "{l}");
            assert!(!act.sim_dirty && !act.net_dirty);
        }
    }

    #[test]
    fn all_four_actions_coexist() {
        let act = urc_actions(&[
            line(K::GtSim, "+GTSIM: 1"),
            line(K::Reg, "+CEREG: 5"),
            line(K::Cusd, "+CUSD: 1,\"继续\""),
            line(K::SmsNew, "+CMTI: \"SM\",7"),
        ]);
        assert!(act.sim_dirty);
        assert!(act.net_dirty);
        assert_eq!(act.cusd.len(), 1);
        assert!(act.sms_dirty);
    }
}
