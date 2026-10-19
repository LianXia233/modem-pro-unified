//! 数据漫游（`GET /api/roam`）。
//!
//! 与 `explorer` 无关：只读缓存（注册状态 + IMSI + 当前 PLMN），不发 AT。
//! `effective=false` 表示「开关关着 + 国际漫游」⇒ 数据链路应停。

use serde::Serialize;

use super::modem::Modem;
use super::siminfo::{self, RoamState};

/// 漫游视图。
#[derive(Debug, Clone, Serialize)]
pub struct RoamView {
    pub roaming: bool,
    #[serde(rename = "roam_scope")]
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_mcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visit_mcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roam_note: Option<String>,
    /// 界面上这个开关读的是 `allow_roaming`（与 `config.toml` 的 `[net] allow_roaming` 同名）
    #[serde(rename = "allow_roaming")]
    pub allowed: bool,
    pub applies: bool,
    pub effective: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Modem {
    /// 读取漫游视图。`allow_roaming` 来自 `[net] allow_roaming`。
    pub fn roam_view(&self, allow_roaming: bool) -> RoamView {
        let imsi = self
            .with_at(|p| p.command("AT+CIMI", self.cmd_timeout))
            .unwrap_or_default();
        let imsi = super::siminfo::parse_imsi(&imsi);
        let reg = self
            .with_at(|p| p.command("AT+CGREG?", self.cmd_timeout))
            .map(|raw| super::regstate::parse(&raw, "+CGREG"))
            .unwrap_or_default();
        let plmn = self
            .with_at(|p| p.command("AT+COPS?", self.cmd_timeout))
            .map(|raw| super::netselect::parse(&raw).plmn)
            .unwrap_or(None);

        let state = siminfo::roam_state(reg.roaming, imsi.as_deref(), plmn.as_deref());
        build_view(state, allow_roaming)
    }
}

/// 由 [`RoamState`] 与开关值构造视图。
pub fn build_view(state: RoamState, allowed: bool) -> RoamView {
    // 「漫游策略是否适用」：只有判据齐全时才成立。
    // `scope == "unknown"` 意味着 IMSI 或注册网 PLMN 至少缺一个，国别判不出来
    // （`roam_state` 里写明「所以不拦」）—— 那时策略谈不上适用，写死 true 会让
    // 界面把「判不出」显示成「已按策略处理」。
    let applies = matches!(state.scope.as_str(), "domestic" | "global");
    let blocked = state.roaming && !allowed && state.scope == "global";
    RoamView {
        roaming: state.roaming,
        scope: state.scope.clone(),
        home_mcc: state.home_mcc,
        visit_mcc: state.visit_mcc,
        roam_note: state.note,
        allowed,
        applies,
        effective: !blocked,
        reason: if blocked {
            Some("国际漫游已开启但「数据漫游」开关关闭，数据链路应停".to_string())
        } else {
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::build_view;
    use crate::modem::model::siminfo::{roam_state, RoamState};

    #[test]
    fn blocked_global_roaming() {
        let st = roam_state(true, Some("460110123456789"), Some("26201"));
        let v = build_view(st, false);
        assert!(v.roaming);
        assert!(!v.effective);
        assert!(v.reason.is_some());
    }

    #[test]
    fn domestic_not_blocked() {
        let st = roam_state(true, Some("460110123456789"), Some("46000"));
        let v = build_view(st, false);
        assert_eq!(v.scope, "domestic");
        assert!(v.effective);
    }

    #[test]
    fn no_roaming() {
        let v = build_view(
            RoamState {
                roaming: false,
                scope: "none".into(),
                home_mcc: None,
                visit_mcc: None,
                note: None,
            },
            false,
        );
        assert!(v.effective);
    }
}
