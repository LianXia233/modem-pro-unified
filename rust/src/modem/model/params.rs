//! PDP 上下文与选网总览（`GET /api/params`）。
//!
//! 配置来自 `AT+CGDCONT?`，激活状态来自 `AT+CGACT?`，选网方式来自 `AT+COPS?`；
//! 三条命令合并在**同一次端口占用**里发（见 [`Modem::read_params`]）。
//!
//! 写入侧的校验口径与 Go `module/kernel/data_validate.go` 对齐
//!（`ValidAPN` / `ValidPLMN`），差异见 [`Modem::set_apn`] 的注释。

use std::time::Duration;

use serde::Serialize;

use crate::error::{msg, Result};

use super::super::at::{csv, extract_all, is_duplicate_activate, is_ok, Prio};
use super::modem::Modem;
use super::netselect::{self, NetSelect};

/// 一个 PDP 上下文。
#[derive(Debug, Clone, Serialize)]
pub struct Context {
    pub cid: i32,
    pub pdp_type: String,
    pub apn: String,
    pub active: bool,
    /// IMS / sos 等内部专用 APN，不用于上网
    pub internal: bool,
}

/// 参数总览。
#[derive(Debug, Clone, Serialize)]
pub struct ParamsStatus {
    pub modem_id: u32,
    /// **真实查询**时刻（命中缓存时是上一次问模组的时间，不是现在）
    pub queried_at: String,
    /// 当前在用的 APN：第一个**已激活且非内部专用**的上下文（无则没有这个字段）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apn: Option<String>,
    pub contexts: Vec<Context>,
    pub net_select: NetSelect,
    /// 这份数据是几秒前问的（0 / 缺省 = 刚刚实时查的）。
    ///
    /// 中文页面上要显示成「15 秒前」，别让用户以为看到的是此刻的值。
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub cached_secs: u64,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl Modem {
    /// 读取参数总览（**走缓存**）。
    ///
    /// 15 秒 TTL：进一次设置页要问三条命令（`AT+CGDCONT?` / `AT+CGACT?` /
    /// `AT+COPS?`），页面来回切两次就是六条 —— 而 APN 只有被人改的时候才变。
    /// 写操作（`set_apn` 等）成功后会 [作废缓存](super::super::at::Ttl::invalidate)，
    /// 所以「改完立刻看到新值」不受影响。
    pub fn params(&self) -> ParamsStatus {
        if let Some(cached) = self.params_cache.get() {
            let mut v = cached;
            v.cached_secs = self.params_cache.age_secs().unwrap_or(0);
            return v;
        }
        let v = self.read_params();
        self.params_cache.set(v.clone());
        v
    }

    /// 强制实时读取（页面「刷新」按钮、拨号流程要拿最新 APN 时用）。
    pub fn params_refresh(&self) -> ParamsStatus {
        self.params_cache.invalidate();
        self.params()
    }

    /// 真正发 AT 问一遍。
    ///
    /// 三条命令放在**同一次端口占用**里发：早先每条都单独排一次队，
    /// 光排队间隔（60ms × 2）就比命令本身还久。合起来之后，每条命令
    /// 「等应答」的时间天然构成间隔，模组不会被打得更急。
    fn read_params(&self) -> ParamsStatus {
        let (defined, activated, cops) = self
            .with_at_prio(Prio::Bulk, |p| {
                let t = self.cmd_timeout;
                let defined = p.command("AT+CGDCONT?", t).unwrap_or_default();
                let activated = p.command("AT+CGACT?", t).unwrap_or_default();
                let cops = p.command("AT+COPS?", t).unwrap_or_default();
                Ok((defined, activated, cops))
            })
            .unwrap_or_default();

        let net_select = netselect::parse(&cops);
        let contexts = build_contexts(&defined, &activated);
        let apn = contexts
            .iter()
            .find(|c| c.active && !c.internal)
            .map(|c| c.apn.clone());

        ParamsStatus {
            modem_id: self.id,
            queried_at: crate::logging::timestamp(),
            apn,
            contexts,
            net_select,
            cached_secs: 0,
        }
    }
}

/// PLMN 文本校验（手动选网要 5~6 位数字：3 位 MCC + 2~3 位 MNC）。
///
/// 依据 Go `module/kernel/data_validate.go:43` 的 `ValidPLMN` —— 两边同口径。
fn valid_plmn_text(p: &str) -> bool {
    p.len() >= 5 && p.len() <= 6 && p.chars().all(|c| c.is_ascii_digit())
}

/// APN 只允许字母 / 数字与 `. - _`。
///
/// 依据 Go `module/kernel/data_validate.go:23` 的 `ValidAPN`（同口径）：
/// 长度上限 100 是 3GPP 的规定；字符必须卡死是因为这个值会**原样拼进
/// `AT+CGDCONT=<cid>,"<pdp>","<apn>"`** —— 放过一个 `"` 或 `,` 就等于把用户输入
/// 拼进 AT 命令行（截断指令、甚至改写 cid）。`#` 同理。
///
/// 空串也不合法：空 APN 在多数固件上是「删掉这条定义」，不是「不改」。
pub fn valid_apn(apn: &str) -> bool {
    let s = apn.trim();
    !s.is_empty()
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

impl Modem {
    /// 写 APN：`AT+CGDCONT=<cid>,"<pdp>","<apn>"` → 回读确认 → 重新激活该 cid → 顺手激活 IMS。
    ///
    /// 返回（新的参数总览，每一步的说明）。回读是必须的：模组回 OK 不代表真存下了。
    pub fn set_apn(
        &self,
        cid: i32,
        pdp_type: &str,
        apn: &str,
        activate_ims: bool,
    ) -> Result<(ParamsStatus, String)> {
        let apn = apn.trim();
        if cid <= 0 {
            return Err(msg("上下文 cid 要是正数"));
        }
        if !valid_apn(apn) {
            return Err(msg("APN 只允许字母 / 数字与 . - _"));
        }
        let mut steps = Vec::new();

        // 幂等闸门：该 cid 已经是同 APN + 同类型时**跳过重写与重激活**。
        //
        // 为什么必须做：自动重拨调度在「PDP 已激活、主机链路还没就绪」的窗口里
        // 会带着同一个 APN 反复走到这里 —— 不做幂等的话，每次都会先断后连，
        // 30 秒拆一次正常会话（实测会话层看不出来，但流量/时延抖动真实存在）。
        // 「写入」的语义应该是「确保配置如此」，而不是「无条件重来一遍」。
        let defined = self.at_cmd_prio(Prio::Bulk, "AT+CGDCONT?");
        let contexts = build_contexts(&defined, "");
        let want_type = pdp_type.to_ascii_uppercase();
        let unchanged = contexts
            .iter()
            .find(|c| c.cid == cid)
            .map(|c| {
                c.apn.eq_ignore_ascii_case(apn) && c.pdp_type.to_ascii_uppercase() == want_type
            })
            .unwrap_or(false);

        // QMI / MBIM（主机侧拨号）：数据承载归内置原生拨号器独占，模组侧**只写配置、
        // 不激活**（上游 apiSetAPN 同款闸门：连显式要求激活也不激活）。模组侧一激活，
        // 拨号器请求数据呼叫就被拒（QMI 实测 QMUXError 0xe；MBIM 实测 CONNECT
        // Status=2 FAILURE、ActivationState 一直 Deactivated），IPv6 那条会话尤其
        // 起不来 —— 页面上原生拨号器就一直「未运行」。激活交给拨号器；IMS 的激活
        // 同样跳过（上游 doActivate=false 时 IMS 也不发）。
        let host_dial = super::dial::wan_host_dial_by_daemon(&self.ensure_dial_kind());

        // 幂等路径不读 was_active：配置没变就不该动已建立的会话，
        // 「是不是激活着」在这里没有后续动作。
        if unchanged {
            steps.push(format!(
                "cid {cid} 配置未变化（{pdp_type} {apn}），跳过重写（幂等）"
            ));
        } else {
            let cmd = format!("AT+CGDCONT={cid},\"{pdp_type}\",\"{apn}\"");
            let resp = self.with_at_prio(Prio::Bulk, |p| p.command(&cmd, self.cmd_timeout))?;
            if !is_ok(&resp) {
                return Err(msg(format!("写入失败：{}", resp.trim())));
            }
            steps.push(format!("已下发 {cmd}"));

            // 回读确认
            let defined = self.at_cmd_prio(Prio::Bulk, "AT+CGDCONT?");
            let contexts = build_contexts(&defined, "");
            let stored = contexts
                .iter()
                .find(|c| c.cid == cid)
                .map(|c| c.apn.clone());
            if stored.as_deref() != Some(apn) {
                return Err(msg(format!(
                    "模组回读了不一样的 APN（写入 {apn}，回读 {}）",
                    stored.unwrap_or_else(|| "空".to_string())
                )));
            }
            steps.push("回读确认一致".to_string());

            // 之前激活着的就先断后连，让新 APN 立即生效。
            // ⚠️ 主机侧拨号（QMI/MBIM）不走这里：先断后连会占住唯一数据承载
            //（见上面 host_dial 闸门），承载的建立/释放归内置原生拨号器管。
            let activated = self.at_cmd_prio(Prio::Bulk, "AT+CGACT?");
            let was_active = build_contexts(&defined, &activated)
                .iter()
                .any(|c| c.cid == cid && c.active);
            if was_active && !host_dial {
                let _ = self.at_cmd_prio(Prio::Bulk, &format!("AT+CGACT=0,{cid}"));
                let resp = self.at_cmd_prio(Prio::Bulk, &format!("AT+CGACT=1,{cid}"));
                steps.push(if is_ok(&resp) {
                    "已重新激活该上下文（先断后连）".to_string()
                } else {
                    format!("重新激活被拒：{}", resp.trim())
                });
            }
        }

        // IMS 是 VoLTE / IMS 短信的前提，顺手拉起来；失败不影响本次写入。
        // 主机侧拨号（QMI/MBIM）跳过：与上游口径一致，模组侧不为拨号器管的承载发激活。
        if activate_ims && !host_dial {
            if let Some(ims) = contexts.iter().find(|c| c.internal) {
                let resp = self.at_cmd_prio(Prio::Bulk, &format!("AT+CGACT=1,{}", ims.cid));
                steps.push(if is_ok(&resp) {
                    format!("已激活 IMS 上下文（cid {}）", ims.cid)
                } else {
                    format!(
                        "IMS 激活被拒（cid {}，固件不许手动激活），不影响本次写入",
                        ims.cid
                    )
                });
            }
        }
        // 主机侧拨号（QMI/MBIM）：把「为什么没激活」如实写进返回说明（上游同款 autoNote），
        // 否则用户看到只有「已写入」，会以为 APN 没生效。
        if host_dial {
            steps.push(
                "这是 QMI/MBIM（主机侧拨号）模式：只写配置、不激活 PDP —— 数据承载留给\
                 内置原生拨号器，模组侧一激活它就被顶掉（拨号器会自动用新配置建立承载）"
                    .to_string(),
            );
        }
        Ok((self.params(), steps.join("；")))
    }

    /// 删除一条 APN 定义：`AT+CGDCONT=<cid>`（不带后面的参数即删除）。
    pub fn delete_apn(&self, cid: i32) -> Result<(ParamsStatus, String)> {
        if cid <= 0 {
            return Err(msg("上下文 cid 要是正数"));
        }
        let resp = self.with_at_prio(Prio::Bulk, |p| {
            p.command(&format!("AT+CGDCONT={cid}"), self.cmd_timeout)
        })?;
        if !is_ok(&resp) {
            return Err(msg(format!("删除失败：{}", resp.trim())));
        }
        let defined = self.at_cmd_prio(Prio::Bulk, "AT+CGDCONT?");
        let still = build_contexts(&defined, "").iter().any(|c| c.cid == cid);
        let msg = if still {
            format!("已下发删除，但模组仍报有 cid {cid}（可能被固件保留）")
        } else {
            format!("已删除 cid {cid} 并回读确认")
        };
        Ok((self.params(), msg))
    }

    /// 激活 / 停用一条 PDP 上下文（`reconnect` = 先断后连）。
    pub fn set_pdp(
        &self,
        cid: i32,
        active: bool,
        reconnect: bool,
    ) -> Result<(ParamsStatus, String)> {
        if cid <= 0 {
            return Err(msg("上下文 cid 要是正数"));
        }
        // QMI / MBIM（主机侧拨号）：数据承载归内置原生拨号器独占 —— 模组侧激活会把
        // 承载占住，拨号器随后请求数据呼叫被拒、IPv6 起不来（上游 apiSetPDPActive
        // 同款闸门）。这里只允许**停用**（放掉承载）；要上网用「拨号上网」，
        // 那里拨号器会自己建承载。
        if (active || reconnect) && super::dial::wan_host_dial_by_daemon(&self.ensure_dial_kind()) {
            return Err(msg(
                "这是 QMI/MBIM（主机侧拨号）模式：数据呼叫由内置原生拨号器激活，\
                 模组侧不要手动激活（会占住唯一数据承载，拨号器随后被拒、IPv6 起不来）；\
                 要上网请用「拨号上网」",
            ));
        }
        let mut steps = Vec::new();
        if reconnect {
            // 部分固件不允许停用内部上下文（实测 RM502Q-GL 的 AT+CGACT=0,2 回 ERROR），
            // 被拒就跳过断开、直接激活
            let off = self.at_cmd_prio(Prio::Bulk, &format!("AT+CGACT=0,{cid}"));
            steps.push(if is_ok(&off) {
                "已断开".to_string()
            } else {
                "断开被拒（固件不许手动停用），直接激活".to_string()
            });
        }
        let want = if active { 1 } else { 0 };
        let resp = self.at_cmd_prio(Prio::Bulk, &format!("AT+CGACT={want},{cid}"));
        // `+CME ERROR: 5847` 是「重复激活」：上下文其实已经激活，不该判失败。
        // 只对激活放行 —— 停用回 5847 没有语义。
        let duplicate = active && is_duplicate_activate(&resp);
        if !is_ok(&resp) && !duplicate {
            return Err(msg(format!(
                "{}失败：{}",
                if active { "激活" } else { "停用" },
                resp.trim()
            )));
        }
        steps.push(
            if duplicate {
                "模组报告该上下文已激活（重复激活），按成功处理".to_string()
            } else if active {
                "已激活".to_string()
            } else {
                "已停用".to_string()
            }
            .to_string(),
        );
        Ok((self.params(), steps.join("；")))
    }

    /// 选网方式：`auto` → `AT+COPS=0`；`manual` → `AT+COPS=1,2,"<plmn>"`。
    pub fn set_netselect(&self, mode: &str, plmn: &str) -> Result<(ParamsStatus, String)> {
        let mode = mode.trim().to_ascii_lowercase();
        let cmd = match mode.as_str() {
            "auto" | "0" => "AT+COPS=0".to_string(),
            "manual" | "1" => {
                let p = plmn.trim();
                if !valid_plmn_text(p) {
                    return Err(msg("手动选网需要 5~6 位数字的 PLMN，如 46011"));
                }
                format!("AT+COPS=1,2,\"{p}\"")
            }
            other => return Err(msg(format!("不支持的选网方式 {other}"))),
        };
        // 选网会重注册，给足时间（普通命令超时只有几秒）
        let resp = self.with_at_prio(Prio::Bulk, |p| p.command(&cmd, Duration::from_secs(60)))?;
        if !is_ok(&resp) {
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        Ok((
            self.params(),
            format!("已下发 {cmd}（选网可能要几十秒完成重注册）"),
        ))
    }
}

/// 合并 `AT+CGDCONT?` 与 `AT+CGACT?` 的应答。
pub fn build_contexts(defined: &str, activated: &str) -> Vec<Context> {
    let active: Vec<(i32, bool)> = extract_all(activated, "+CGACT")
        .iter()
        .filter_map(|line| {
            let f = csv(line);
            Some((
                f.first()?.parse().ok()?,
                f.get(1)?.parse::<i32>().ok()? == 1,
            ))
        })
        .collect();

    extract_all(defined, "+CGDCONT")
        .iter()
        .filter_map(|line| {
            let f = csv(line);
            let cid: i32 = f.first()?.parse().ok()?;
            let pdp_type = f.get(1).cloned().unwrap_or_default();
            let apn = f.get(2).cloned().unwrap_or_default();
            let internal = is_internal_apn(&apn);
            let active = active.iter().any(|(c, a)| *c == cid && *a);
            Some(Context {
                cid,
                pdp_type,
                apn,
                active,
                internal,
            })
        })
        .collect()
}

/// 拨号用的 APN 上下文优选（**三方共用**：一键拨号 / 自动重拨调度 / 拨号器托管）。
///
/// 优先级：已激活且非 IMS 的 → 任意已定义且非 IMS 的 → None（调用方回落
/// 「沿用模组配置 + 默认 cid」）。之前这段逻辑只活在前端一键拨号里，
/// 自动拨号与托管 reconcile 拿不到 APN 时只能空手退避 —— MBIM 模组
/// （拨号器没有 -s 直接退出）会陷入「算不出计划 → 5 秒退避 → 再来」的死循环，
/// 死循环，明明模组里就存着可用的上下文。
pub fn pick_dial_context(ctxs: &[Context]) -> Option<Context> {
    let usable = || {
        ctxs.iter()
            .filter(|c| !c.internal && !c.apn.trim().is_empty())
    };
    usable()
        .find(|c| c.active)
        .cloned()
        .or_else(|| usable().next().cloned())
}

/// 内部专用 APN（IMS / sos / mms）不上网。
pub fn is_internal_apn(apn: &str) -> bool {
    let a = apn.to_ascii_lowercase();
    a.contains("ims") || a.contains("sos") || a.contains("mms")
}

/// 从 `AT+CGACT?` 应答判定指定 cid 的激活态。
///
/// 三态而不是 bool，因为「模组没回任何 `+CGACT:` 行」和「回了但目标 cid
/// 不在列里」是两件不同的事：
///
/// - `None` —— 命令不可用 / 未列明细，调用方应回落其它判据；
/// - `Some(false)` —— 明确列了明细而目标 cid 不在或状态为 0，**权威未激活**；
/// - `Some(true)` —— 目标 cid 状态为 1。
///
/// 之所以要区分：部分平台（实测广和通 MTK 平台）的 `AT+CGACT?` 只回 `OK`
/// 不列明细，此时不能把「列表为空」当成「全部未激活」。
pub fn cgact_state(resp: &str, cid: i32) -> Option<bool> {
    let mut seen = false;
    let mut active = false;
    for line in extract_all(resp, "+CGACT") {
        let f = csv(&line);
        let Some(rc) = f.first().and_then(|v| v.parse::<i32>().ok()) else {
            continue;
        };
        seen = true;
        if rc == cid {
            active = f.get(1).and_then(|v| v.parse::<i32>().ok()) == Some(1);
        }
    }
    if seen {
        Some(active)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_contexts, cgact_state, is_internal_apn, pick_dial_context, valid_apn,
        valid_plmn_text, Context,
    };

    #[test]
    fn merge_contexts() {
        let defined = "+CGDCONT: 1,\"IPV4V6\",\"3GNET\",\"0.0.0.0\"\r\n\
                       +CGDCONT: 2,\"IP\",\"ims\",\"0.0.0.0\"\r\nOK\r\n";
        let activated = "+CGACT: 1,1\r\n+CGACT: 2,1\r\nOK\r\n";
        let ctx = build_contexts(defined, activated);
        assert_eq!(ctx.len(), 2);
        assert!(ctx[0].active);
        assert!(!ctx[0].internal);
        assert!(ctx[1].internal);
    }

    #[test]
    fn cgact_state_three_way() {
        // 列了明细但目标 cid 不在 → 权威未激活
        assert_eq!(cgact_state("+CGACT: 0,1\r\nOK", 1), Some(false));
        // 目标 cid 状态 1 → 已激活
        assert_eq!(
            cgact_state("+CGACT: 0,1\r\n+CGACT: 1,1\r\nOK", 1),
            Some(true)
        );
        assert_eq!(cgact_state("+CGACT: 1,0\r\nOK", 1), Some(false));
        // 一条 +CGACT: 行都没有 → 不可用，调用方回落其它判据
        assert_eq!(cgact_state("\r\nOK\r\n", 1), None);
        assert_eq!(cgact_state("", 1), None);
        assert_eq!(cgact_state("+CME ERROR: unknown\r\n", 1), None);
    }

    #[test]
    fn apn_must_be_safe_to_embed() {
        assert!(valid_apn("3GNET"));
        assert!(valid_apn("cmnet.mnc011.mcc460.gprs"));
        assert!(valid_apn("a-b_c.d"));
        assert!(!valid_apn(""));
        assert!(!valid_apn("a b"), "空格不行");
        assert!(!valid_apn("a\"b"), "引号不行（会截断 AT 指令）");
        assert!(!valid_apn("a,b"), "逗号不行（会截断 AT 指令）");
    }

    #[test]
    fn plmn_text_must_be_digits() {
        assert!(valid_plmn_text("46011"));
        assert!(valid_plmn_text("460011"));
        assert!(!valid_plmn_text("4601"));
        assert!(!valid_plmn_text("4601x"));
    }

    #[test]
    fn pick_dial_context_prefers_active_then_any() {
        let mk = |cid: i32, apn: &str, active: bool, internal: bool| Context {
            cid,
            pdp_type: "IPV4V6".into(),
            apn: apn.into(),
            active,
            internal,
        };
        // 有已激活的非 IMS → 选它（哪怕排后面）
        let ctxs = [
            mk(0, "", false, false),
            mk(3, "ims", true, true),
            mk(5, "cmnet", true, false),
        ];
        assert_eq!(pick_dial_context(&ctxs).unwrap().cid, 5);
        // 没有已激活的 → 任意已定义非 IMS（选先出现的）
        let ctxs = [
            mk(0, "", false, false),
            mk(2, "ims", true, true),
            mk(4, "3gnet", false, false),
        ];
        assert_eq!(pick_dial_context(&ctxs).unwrap().cid, 4);
        // 全是 IMS / 空 APN → None（调用方回落默认）
        let ctxs = [mk(0, "", false, false), mk(1, "ims", true, true)];
        assert!(pick_dial_context(&ctxs).is_none());
        assert!(pick_dial_context(&[]).is_none());
    }

    #[test]
    fn internal_apn_detection() {
        assert!(is_internal_apn("ims"));
        assert!(is_internal_apn("IMS"));
        assert!(is_internal_apn("sos"));
        assert!(!is_internal_apn("3gnet"));
    }
}
