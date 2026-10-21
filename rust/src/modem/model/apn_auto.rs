//! 运营商感知的 APN 自动配置（决策 + 落地）。
//!
//! ## 解决什么问题
//!
//! 部分模组**必须显式下发 APN** 才能拨号：模组里没有可用上下文时，自动拨号
//! 「沿用模组配置」拨上去就是不通；而页面上明明显示着运营商（`AT+COPS?` 名称
//! / `AT+QNWINFO` 的 PLMN）甚至显示着一个在用 APN（`AT+CGCONTRDP`）——
//! 之前没有任何路径把这些信息用于拨号。本模块把「识别运营商 → 匹配预制 APN →
//! 校验/写入 → 确认生效 → 才拨号」串成一条链，接进拨号协调器的两条自动路径。
//!
//! ## 优先级表（`decide` 纯函数钉死，用户配置永远最高）
//!
//! | 级 | 证据 | 动作 |
//! | --- | --- | --- |
//! | 1 | 用户/表单显式指定的 APN（`DialState.apn`） | **沿用，什么都不写** |
//! | 2a | 模组上下文 = 预制 APN 且与识别出的运营商**同家** | 沿用（已正确配置，跳过写入） |
//! | 2b | 模组上下文 = 预制 APN 但**是别家**的（插过别家卡 / 运营商已切换） | 校正为本家预制 APN（需名称/PLMN 级证据） |
//! | 2c | 模组上下文 = **自定义** APN（企业卡 / 专用套餐） | 沿用，**绝不覆盖** |
//! | 3 | 模组上下文为空，但 `CGCONTRDP` 报了一个在用 APN | 沿用那个（页面显示的就是它） |
//! | 4 | 模组没有可用 APN + 运营商识别成功 | **写入**预制 APN（唯一自动写路径） |
//! | 5 | 运营商识别失败 | 保留现状（行为与旧版一致） |
//!
//! 「运营商切换 / 换卡」不需要单独分支：切换后 2a 沿用、2b 校正、2c 保留，
//! 三条路自动覆盖；SIM 换卡时 `reset_auto_apn` 作废「自动下发」记录，
//! 校正原因里会注明该值是不是本程序上次写的。
//!
//! ## 落地方式与后端的差异（不假设所有模组用同一种 APN 设置方法）
//!
//! - **AT 路径**（ECM/RNDIS/NCM）：[`Modem::auto_apn_prepare`] 在**拨号线程内**
//!   发 `AT+COPS?` + `AT+QNWINFO` + `AT+CGCONTRDP`（识别）与 `AT+CGDCONT`（按需
//!   写入，`set_apn` 自带幂等闸门与回读确认）—— 拨号本来就要占串口，这里多两条
//!   不破坏边界；**协调器线程绝不发 AT**。
//! - **原生路径**（QMI/MBIM）：APN 不写模组，作为参数直接进拨号器计划
//!   （QMI `WDS Start` / MBIM `CONNECT`），会话建成功即「配置生效」——
//!   见 [`Modem::auto_apn_suggestion`]（只读缓存，供 `reconcile_native_one`）。
//! - **PPP / 厂商私有**：同样经 `DialRequest.apn` 走各自配方（`vendor::datacall`），
//!   本模块不关心最终落到哪条指令。

use crate::modem::at::Prio;
use crate::modem::model::netselect;
use crate::modem::model::network::{parse_active_context, NetworkStatus};
use crate::modem::model::params::{is_internal_apn, pick_dial_context, Context};
use crate::modem::model::Modem;

use super::carrier::{self, Carrier, CarrierMatch, Evidence};
use super::dial::DEFAULT_DATA_CID;

/// 一条「我们自动下发过 APN」的记录（内存态：重启后重新识别，行为等价）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoApnRecord {
    /// 下发时的 PLMN（运营商切换检测的基准）
    pub plmn: String,
    /// 下发的 APN
    pub apn: String,
    /// 写入的 cid
    pub cid: i32,
}

/// 运营商证据缓存（由 `sync_dial_state` / `carrier_snapshot` 填充）。
///
/// 原生路径的托管在不发 AT 的约束下只能用这里存的事实：采样循环每 3 秒
/// 查一次网络状态（有人看时），顺手把运营商同步过来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierEvidence {
    pub operator: Option<String>,
    pub plmn: Option<String>,
    /// 记录时刻（unix 秒）；过期证据不用于决策
    pub at: i64,
}

/// 证据的有效期（秒）：运营商极少变，但太久之前的显示名不值得再信。
const EVIDENCE_TTL_SECS: i64 = 600;

/// 决策结果（`apn` 会作为 `DialRequest.apn` 传下去；空串 = 沿用模组配置）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApnDecision {
    /// 配置已可用：直接用这个 APN（模组里已经是它，不需要写）
    Use { apn: String, reason: String },
    /// 需要写入这个预制 APN（写入 + 回读确认由调用方执行）
    Write {
        apn: String,
        carrier: Carrier,
        reason: String,
    },
    /// 识别失败 / 无可用证据：保持旧行为（`apn` 为空 = 沿用模组配置）
    KeepCurrent { reason: String },
}

/// 决策输入（全部是**已采集的事实**，纯函数可测）。
pub struct ApnFacts<'a> {
    /// 用户/表单显式指定的 APN（`DialState.apn`；内部 APN 已被排除）
    pub user_apn: Option<&'a str>,
    /// `AT+CGCONTRDP` 报的当前在用 APN（页面「网络」卡片显示的那个）
    pub in_use_apn: Option<&'a str>,
    /// 模组 `AT+CGDCONT?` 上下文现状
    pub contexts: &'a [Context],
    /// `pick_dial_context` 选出的可用上下文（`None` = 模组没有可用 APN）
    pub picked: Option<&'a Context>,
    /// 运营商识别结果
    pub carrier: &'a CarrierMatch,
    /// 本次识别到的 PLMN（运营商切换检测用；可能为空）
    pub plmn_now: Option<&'a str>,
    /// 上次自动下发的记录（内存态）
    pub auto_written: Option<&'a AutoApnRecord>,
}

/// APN 决策（优先级表见模块头；**唯一**决定「写不写、写什么」的地方）。
pub fn decide(f: ApnFacts<'_>) -> ApnDecision {
    // ① 用户显式配置：最高优先级，连运营商识别都不参与
    if let Some(a) = f.user_apn.map(str::trim).filter(|s| !s.is_empty()) {
        return ApnDecision::Use {
            apn: a.to_string(),
            reason: "用户显式配置的 APN，优先沿用（不做自动匹配）".into(),
        };
    }

    let picked_apn = f
        .picked
        .map(|c| c.apn.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // ② 模组里已有可用上下文
    //
    // 「运营商切换后的重新校验」也走这里：切换后 picked_apn 要么与本家预制一致
    // （沿用）、要么是别家的预制值（识别足够强时校正为本家）、要么是自定义值
    // （保留）—— 三条路都不需要知道「上次是谁写的」，`auto_written` 只用来把
    // 原因说得更明白（区分「我们上次自动下发的」与「用户手写的」）。
    if let Some(cur) = picked_apn {
        let preset = carrier::apn_carrier(&cur);
        let identified = f.carrier.carrier;
        let context_note = cur.clone();
        let provenance = match f
            .auto_written
            .filter(|rec| rec.apn.eq_ignore_ascii_case(&cur))
        {
            Some(rec) if !rec.plmn.is_empty() => {
                format!("（该值是本程序按运营商 {} 自动下发的）", rec.plmn)
            }
            _ => String::new(),
        };
        return match (preset, identified) {
            // 与识别出的运营商同家：已正确配置
            (Some(_), Some(c)) if preset == Some(c) => ApnDecision::Use {
                apn: cur,
                reason: format!(
                    "模组上下文已是 {} 的预制 APN（{context_note}），跳过写入",
                    c.label()
                ),
            },
            // 是别家的预制 APN（插过别家卡留下的 / 运营商已切换），且本次识别足够强 ⇒ 校正
            (Some(other), Some(c)) if f.carrier.strong() => ApnDecision::Write {
                apn: c.default_apn().to_string(),
                carrier: c,
                reason: format!(
                    "模组上下文是 {} 的预制 APN（{context_note}）{provenance}，已识别为 {}，校正为 {}",
                    other.label(),
                    c.label(),
                    c.default_apn()
                ),
            },
            // 识别弱 / 识别失败 / 自定义 APN：一律保留现状
            _ => ApnDecision::Use {
                apn: cur,
                reason: if f.carrier.matched() {
                    format!(
                        "模组上下文 {context_note} 不在预制表内（自定义或专用套餐），保留不覆盖"
                    )
                } else {
                    format!("模组上下文 {context_note} 可用，运营商识别失败，保留现有配置")
                },
            },
        };
    }

    // ③ 模组没有可用上下文，但 CGCONTRDP 报了一个在用 APN（页面显示的那个）
    if let Some(a) = f.in_use_apn.map(str::trim).filter(|s| !s.is_empty()) {
        return ApnDecision::Use {
            apn: a.to_string(),
            reason: "模组没有已定义上下文，但 CGCONTRDP 报了在用 APN，直接用它拨号".into(),
        };
    }

    // ④ 模组没有可用 APN + 运营商识别成功 ⇒ 下发预制 APN（唯一自动写路径）
    if let Some(c) = f.carrier.carrier {
        if f.carrier.matched() {
            return ApnDecision::Write {
                apn: c.default_apn().to_string(),
                carrier: c,
                reason: format!(
                    "模组没有可用 APN，按识别到的 {}（{}）下发预制 APN",
                    c.label(),
                    f.carrier.note
                ),
            };
        }
    }

    // ⑤ 识别失败：保持旧行为
    ApnDecision::KeepCurrent {
        reason: f.carrier.note.clone(),
    }
}

/// 一次自动拨号前的 APN 准备结果（给 `DialRequest` 用）。
#[derive(Debug, Clone, Default)]
pub struct AutoApnPrep {
    /// 传给 `DialRequest.apn`；空串 = 沿用模组配置
    pub apn: String,
    /// 建议的 cid（ picked 的；没有可用上下文时用数据默认 cid）
    pub cid: i32,
    /// 建议的 PDP 类型（picked 的；写预制 APN 时 `IPV4V6`）
    pub pdp_type: String,
    /// 一句话说明（写日志；详细过程已由 note_step 进页面步骤）
    pub note: String,
}

impl AutoApnPrep {
    fn keep(cid: i32, pdp_type: String, apn: String, note: String) -> Self {
        Self {
            apn,
            cid,
            pdp_type,
            note,
        }
    }
}

// ---------- 运营商证据缓存（Modem 字段在 modem.rs，方法集中在这里） ----------

use crate::logging::unix_now;

impl Modem {
    /// 运营商识别 + 按需写入 + 回读确认（**AT 拨号线程内专用**：会发 AT）。
    ///
    /// 时序：`AT+COPS?` / `AT+QNWINFO`（识别）→ 决策 → 需要时 `AT+CGDCONT`
    /// （`set_apn` 幂等 + 回读）→ 返回给 `DialRequest` 的 (cid, apn, pdp_type)。
    /// 任何一步失败都不阻断拨号：回落到旧行为（沿用模组配置），
    /// 过程与原因由 `note_step` 记进页面步骤。
    pub fn auto_apn_prepare(&self) -> AutoApnPrep {
        let snap = self.carrier_snapshot();
        let params = self.params();
        let picked = pick_dial_context(&params.contexts);
        let iccid = self
            .sim_ident
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iccid
            .clone();
        let carrier = carrier::identify(
            snap.operator.as_deref(),
            snap.plmn.as_deref(),
            Some(&iccid).filter(|s| !s.is_empty()).map(|s| s.as_str()),
        );
        let user_apn = {
            let st = self.dial_state.lock().unwrap_or_else(|e| e.into_inner());
            let a = st.apn.trim();
            (!a.is_empty() && !is_internal_apn(a)).then(|| a.to_string())
        };
        let auto = self
            .auto_apn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let decision = decide(ApnFacts {
            user_apn: user_apn.as_deref(),
            in_use_apn: snap.in_use_apn.as_deref(),
            contexts: &params.contexts,
            picked: picked.as_ref(),
            carrier: &carrier,
            plmn_now: snap.plmn.as_deref(),
            auto_written: auto.as_ref(),
        });
        // 有识别结论就顺手记证据缓存（原生路径的托管要用）
        self.note_carrier_match(&snap, &carrier);

        // cid / pdp_type 的基准：picked 的；没有就数据默认 cid + 双栈
        let (base_cid, base_type) = picked
            .as_ref()
            .map(|c| (c.cid, c.pdp_type.trim().to_ascii_uppercase()))
            .unwrap_or((DEFAULT_DATA_CID, String::new()));

        match decision {
            ApnDecision::Use { apn, reason } => {
                self.note_step("运营商感知 APN", true, &reason);
                let cid = picked.as_ref().map(|c| c.cid).unwrap_or(base_cid);
                AutoApnPrep::keep(
                    cid,
                    picked
                        .as_ref()
                        .map(|c| c.pdp_type.clone())
                        .unwrap_or_default(),
                    apn,
                    reason,
                )
            }
            ApnDecision::KeepCurrent { reason } => {
                self.note_step("运营商感知 APN", false, &reason);
                AutoApnPrep::keep(
                    base_cid,
                    base_type,
                    picked.as_ref().map(|c| c.apn.clone()).unwrap_or_default(),
                    reason,
                )
            }
            ApnDecision::Write {
                apn,
                carrier: c,
                reason,
            } => {
                self.note_step(
                    "运营商识别",
                    true,
                    &format!(
                        "{}（证据：{:?}）：{}",
                        c.label(),
                        carrier.evidence,
                        carrier.note
                    ),
                );
                // 写入用的类型：有上下文沿用其类型，否则双栈（v4/v6 都兼容）
                let pt = if base_type.is_empty() {
                    "IPV4V6".to_string()
                } else {
                    base_type.clone()
                };
                match self.set_apn(base_cid, &pt, &apn, false) {
                    Ok(_) => {
                        // 回读确认：params_refresh 是权威读（不是只看 AT+CGDCONT 的 OK）
                        let fresh = self.params_refresh();
                        let ok = fresh
                            .contexts
                            .iter()
                            .any(|ctx| ctx.cid == base_cid && ctx.apn.eq_ignore_ascii_case(&apn));
                        self.note_step(
                            "APN 自动配置",
                            ok,
                            &if ok {
                                format!(
                                    "已下发 {}（cid={base_cid}）并回读确认生效；{}",
                                    apn, reason
                                )
                            } else {
                                format!(
                                    "下发 {apn} 后回读不到该值（模组可能不支持该 cid 或写入了别的位置），按现有配置继续；{reason}"
                                )
                            },
                        );
                        if ok {
                            *self.auto_apn.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(AutoApnRecord {
                                    plmn: snap.plmn.clone().unwrap_or_default(),
                                    apn: apn.clone(),
                                    cid: base_cid,
                                });
                        }
                        AutoApnPrep::keep(base_cid, pt, apn, reason)
                    }
                    Err(e) => {
                        // 写失败不阻断拨号：模组侧没有 APN 时 dial 仍会尝试「沿用配置」，
                        // 失败进统一退避，错误在步骤里可追踪
                        self.note_step("APN 自动配置", false, &format!("写入失败：{e}；{reason}"));
                        AutoApnPrep::keep(
                            base_cid,
                            base_type,
                            picked.as_ref().map(|c| c.apn.clone()).unwrap_or_default(),
                            format!("APN 写入失败：{e}"),
                        )
                    }
                }
            }
        }
    }

    /// 原生托管（QMI/MBIM）用的 APN 建议：**只读缓存，绝不发 AT**。
    ///
    /// 调用点已在「用户 / 上次 / 模组上下文」三级都为空之后 —— 走到这里说明
    /// 模组里没有可用 APN，识别成功就给预制值（作为拨号器参数下发，
    /// 会话建立即生效）；识别失败返回空串（旧行为：不带 APN）。
    pub(crate) fn auto_apn_suggestion(&self) -> String {
        let ev = {
            let g = self
                .carrier_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match g {
                Some(ev) if ev.fresh() => ev,
                _ => return String::new(),
            }
        };
        let iccid = self
            .sim_ident
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iccid
            .clone();
        let m = carrier::identify(
            ev.operator.as_deref(),
            ev.plmn.as_deref(),
            Some(&iccid).filter(|s| !s.is_empty()).map(|s| s.as_str()),
        );
        match (m.carrier, m.evidence) {
            (Some(c), ev) if ev != Evidence::None => {
                log::info!(
                    "原生拨号：模组 {} 没有可用 APN，按 {}（{:?}）下发预制 APN {}",
                    self.id,
                    c.label(),
                    m.evidence,
                    c.default_apn()
                );
                c.default_apn().to_string()
            }
            _ => String::new(),
        }
    }

    /// 网络查询结果里的运营商证据顺手入缓存（`sync_dial_state` 调用，不发 AT）。
    pub(crate) fn note_carrier_evidence(&self, st: &NetworkStatus) {
        *self.carrier_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(CarrierEvidence {
            operator: st.operator.clone(),
            plmn: st.plmn.clone(),
            at: crate::logging::unix_now(),
        });
    }

    /// 把一次主动探测的结果记进缓存（`auto_apn_prepare` 调用）。
    fn note_carrier_match(&self, snap: &CarrierSnapshot, m: &CarrierMatch) {
        if !m.matched() {
            return;
        }
        *self.carrier_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(CarrierEvidence {
            operator: snap.operator.clone(),
            plmn: snap.plmn.clone(),
            at: crate::logging::unix_now(),
        });
    }

    /// SIM 更换 / 模组重初始化后作废「自动下发」记录：
    /// 新卡可能是别的运营商，旧记录不能再作为「这个 APN 是我们写的」的证据。
    pub(crate) fn reset_auto_apn(&self) {
        *self.auto_apn.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.carrier_cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// 轻量运营商探测（**三条 AT**：`AT+COPS?` + `AT+QNWINFO?` + `AT+CGCONTRDP`，
    /// 取名称 / PLMN / 在用 APN）。比整趟 `network_status()`（十几条）便宜得多，
    /// 拨号前做前置体检的代价可接受。
    fn carrier_snapshot(&self) -> CarrierSnapshot {
        let cops = self.at_cmd_prio(Prio::Data, "AT+COPS?");
        let sel = netselect::parse(&cops);
        // mode=2（已注销）/ 3（被拒）时模组没有在网运营商：名称/PLMN 都不可信
        let (operator, plmn) = if sel.mode == 2 || sel.mode == 3 {
            (None, None)
        } else {
            (sel.operator.clone(), sel.plmn.clone())
        };
        // COPS 显示名格式下没有数字 PLMN，用 QNWINFO 补（多数固件支持，失败不碍事）
        let plmn = plmn.or_else(|| {
            let q = self.at_cmd_prio(Prio::Data, "AT+QNWINFO?");
            super::network::parse_qnwinfo(&q).plmn
        });
        let (in_use_apn, _, _) = {
            let resp = self.at_cmd_prio(Prio::Data, "AT+CGCONTRDP");
            parse_active_context(&resp)
        };
        CarrierSnapshot {
            operator,
            plmn,
            in_use_apn,
        }
    }
}

/// [`Modem::carrier_snapshot`] 的结果。
struct CarrierSnapshot {
    operator: Option<String>,
    plmn: Option<String>,
    in_use_apn: Option<String>,
}

impl CarrierEvidence {
    fn fresh(&self) -> bool {
        (unix_now() - self.at) < EVIDENCE_TTL_SECS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modem::model::carrier;

    fn ctx(cid: i32, apn: &str, active: bool) -> Context {
        Context {
            cid,
            pdp_type: "IPV4V6".into(),
            apn: apn.into(),
            active,
            internal: false,
        }
    }

    fn m(carrier: Option<Carrier>, evidence: Evidence) -> CarrierMatch {
        CarrierMatch {
            carrier,
            evidence,
            note: "test".into(),
        }
    }

    #[test]
    fn user_apn_always_wins() {
        let cmcc = m(Some(Carrier::Cmcc), Evidence::Name);
        let f = |ua: &'static str| ApnFacts {
            user_apn: Some(ua),
            in_use_apn: Some("ctnet"),
            contexts: &[],
            picked: None,
            carrier: &cmcc,
            plmn_now: Some("46000"),
            auto_written: None,
        };
        // 用户显式配置：识别结果不参与，哪怕指向别家
        let d = decide(f("my.company.apn"));
        assert!(matches!(
            d,
            ApnDecision::Use { ref apn, .. } if apn == "my.company.apn"
        ));
        let d = decide(f("ctnet"));
        assert!(matches!(
            d,
            ApnDecision::Use { ref apn, .. } if apn == "ctnet"
        ));
    }

    #[test]
    fn same_carrier_preset_is_kept_without_rewrite() {
        // 模组里已是 cmnet + 识别出移动：跳过写入（不重复下 AT）
        let ctxs = [ctx(1, "cmnet", false)];
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &ctxs,
            picked: Some(&ctxs[0]),
            carrier: &m(Some(Carrier::Cmcc), Evidence::Plmn),
            plmn_now: Some("46000"),
            auto_written: None,
        });
        assert!(matches!(
            d,
            ApnDecision::Use { ref apn, .. } if apn == "cmnet"
        ));
    }

    #[test]
    fn other_carrier_preset_is_corrected_when_confident() {
        // 模组里是 3gnet（联通的）但识别出电信（PLMN 级）：校正
        let ctxs = [ctx(1, "3gnet", false)];
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &ctxs,
            picked: Some(&ctxs[0]),
            carrier: &m(Some(Carrier::Cect), Evidence::Plmn),
            plmn_now: Some("46011"),
            auto_written: None,
        });
        match d {
            ApnDecision::Write { apn, carrier, .. } => {
                assert_eq!(apn, "ctnet");
                assert_eq!(carrier, Carrier::Cect);
            }
            other => panic!("期望 Write，实际 {other:?}"),
        }
        // 识别证据只有 ICCID 级：不校正（宁可保守）
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &ctxs,
            picked: Some(&ctxs[0]),
            carrier: &m(Some(Carrier::Cect), Evidence::Iccid),
            plmn_now: None,
            auto_written: None,
        });
        assert!(matches!(d, ApnDecision::Use { ref apn, .. } if apn == "3gnet"));
    }

    #[test]
    fn custom_apn_is_never_overwritten() {
        // 企业卡 / 专用套餐的自定义 APN：任何识别强度都不覆盖
        for ev in [Evidence::Iccid, Evidence::Plmn, Evidence::Name] {
            let ctxs = [ctx(1, "my.company.apn", false)];
            let d = decide(ApnFacts {
                user_apn: None,
                in_use_apn: None,
                contexts: &ctxs,
                picked: Some(&ctxs[0]),
                carrier: &m(Some(Carrier::Cmcc), ev),
                plmn_now: Some("46000"),
                auto_written: None,
            });
            assert!(
                matches!(d, ApnDecision::Use { ref apn, .. } if apn == "my.company.apn"),
                "证据 {ev:?} 下不该覆盖自定义 APN"
            );
        }
    }

    #[test]
    fn carrier_switch_updates_auto_written_apn() {
        // 上次给移动写了 cmnet，现在 PLMN 变成 46011（电信）⇒ 更新
        let rec = AutoApnRecord {
            plmn: "46000".into(),
            apn: "cmnet".into(),
            cid: 1,
        };
        let ctxs = [ctx(1, "cmnet", false)];
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &ctxs,
            picked: Some(&ctxs[0]),
            carrier: &m(Some(Carrier::Cect), Evidence::Plmn),
            plmn_now: Some("46011"),
            auto_written: Some(&rec),
        });
        match d {
            ApnDecision::Write { apn, .. } => assert_eq!(apn, "ctnet"),
            other => panic!("期望 Write，实际 {other:?}"),
        }
        // 切换了但识别不出新运营商：保守保留（Use = 模组里的 cmnet 继续用）
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &ctxs,
            picked: Some(&ctxs[0]),
            carrier: &carrier::identify(None, None, None),
            plmn_now: Some("46013"),
            auto_written: Some(&rec),
        });
        assert!(matches!(d, ApnDecision::Use { .. }));
        // PLMN 没变：走「同家预制」分支沿用
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &ctxs,
            picked: Some(&ctxs[0]),
            carrier: &m(Some(Carrier::Cmcc), Evidence::Plmn),
            plmn_now: Some("46007"),
            auto_written: Some(&rec),
        });
        assert!(matches!(d, ApnDecision::Use { .. }));
    }

    #[test]
    fn empty_context_falls_back_to_in_use_then_preset() {
        // 模组没有上下文：先看 CGCONTRDP 的在用 APN（页面显示的那个）
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: Some("cmiot5g"),
            contexts: &[],
            picked: None,
            carrier: &m(Some(Carrier::Cmcc), Evidence::Name),
            plmn_now: Some("46000"),
            auto_written: None,
        });
        assert!(matches!(
            d,
            ApnDecision::Use { ref apn, .. } if apn == "cmiot5g"
        ));
        // 也没有在用 APN + 识别成功 ⇒ 写预制（唯一自动写路径）
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &[],
            picked: None,
            carrier: &m(Some(Carrier::Cmcc), Evidence::Name),
            plmn_now: Some("46000"),
            auto_written: None,
        });
        match d {
            ApnDecision::Write { apn, .. } => assert_eq!(apn, "cmnet"),
            other => panic!("期望 Write，实际 {other:?}"),
        }
        // 识别失败 ⇒ 保持旧行为（沿用模组配置，不写）
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &[],
            picked: None,
            carrier: &carrier::identify(None, None, None),
            plmn_now: None,
            auto_written: None,
        });
        assert!(matches!(d, ApnDecision::KeepCurrent { .. }));
    }

    #[test]
    fn internal_apn_contexts_are_ignored_by_picker() {
        // IMS / sos 不算「可用上下文」：picked 为 None，仍走预制 APN
        let ctxs = [Context {
            cid: 1,
            pdp_type: "IPV4V6".into(),
            apn: "ims".into(),
            active: true,
            internal: true,
        }];
        let picked = pick_dial_context(&ctxs);
        let d = decide(ApnFacts {
            user_apn: None,
            in_use_apn: None,
            contexts: &ctxs,
            picked: picked.as_ref(),
            carrier: &m(Some(Carrier::Cmcc), Evidence::Name),
            plmn_now: Some("46000"),
            auto_written: None,
        });
        match d {
            ApnDecision::Write { apn, .. } => assert_eq!(apn, "cmnet"),
            other => panic!("期望 Write，实际 {other:?}"),
        }
    }
}
