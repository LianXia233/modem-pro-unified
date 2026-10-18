//! 主机网卡面板的视图组装（供 `handlers/host_nic*.rs` 复用）。
//!
//! 与 `http/schedule.rs` 同级：handler 只管取参数与回信封，这里管"这份数据怎么算出来"。
//! 全部是**本机**信息（枚举网卡、读台账），只有"模组上报的本机地址"可能需要问模组
//! —— 优先用拨号状态里已有的值，取不到才补一条 `AT+CGPADDR`。

use crate::http::AppState;
use crate::logging::timestamp;
use crate::modem::model::Modem;
use crate::runtime::hostnic;

/// 页面「主机网卡」卡片要的全部数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostNicView {
    pub modem_id: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// 这个机型是否需要手工配（FM350 = true）
    pub required: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// windows / linux / openwrt
    pub os: String,
    /// 本进程有改主机网卡的权限吗（root / 管理员）
    pub elevated: bool,
    pub interfaces: Vec<hostnic::HostNicIface>,
    pub plan: hostnic::HostNicPlan,
    pub saved: bool,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub saved_plans: std::collections::HashMap<String, hostnic::HostNicPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested: Option<hostnic::HostNicPlan>,
    /// 模组上报的本机地址（建议值的依据）
    #[serde(skip_serializing_if = "String::is_empty")]
    pub modem_ip: String,
    pub actions: Vec<hostnic::HostNicActionSet>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub platform: String,
    #[serde(skip_serializing_if = "is_false")]
    pub openwrt: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub persist: Vec<hostnic::HostNicCommand>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub persist_hint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub checked_at: String,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 组装某台模组的「主机网卡」视图。
pub fn view(state: &AppState, modem: &Modem) -> HostNicView {
    let platform = hostnic::platform();
    let openwrt = platform == "openwrt";
    let (mut required, reason) = hostnic::quirk(&modem.model().unwrap_or_default());
    if !required {
        // 型号短名可能还没读到（没发 ATI）→ 用原始 ATI 串再判一次
        required = hostnic::quirk(&modem.model_raw().unwrap_or_default()).0;
    }

    // 模组上报的本机地址：优先拨号状态（不发 AT），没有才补一条查询
    let mut modem_ip = modem.dial_state_ipv4();
    if modem_ip.is_empty() {
        modem_ip = modem.query_own_ipv4().unwrap_or_default();
    }

    let saved_plans = state.hostnic.saved_plans();
    // 选哪张网卡：已保存配置的那张 → 否则第一个像模组口的 → 否则第一张
    let ifaces = hostnic::interfaces();
    let mut selected = String::new();
    if !saved_plans.is_empty() {
        selected = state
            .hostnic
            .names()
            .into_iter()
            .find(|n| ifaces.iter().any(|it| it.name == *n))
            .or_else(|| state.hostnic.names().first().cloned())
            .unwrap_or_default();
    }
    if selected.is_empty() {
        selected = ifaces
            .iter()
            .find(|it| it.strong)
            .or_else(|| ifaces.first())
            .map(|it| it.name.clone())
            .unwrap_or_default();
    }

    let (mut plan, saved) = match state.hostnic.get(&selected) {
        Some(p) => (p, true),
        None => (hostnic::HostNicPlan::default(), false),
    };
    let suggested = hostnic::suggest_plan(&modem_ip).map(|mut s| {
        s.iface = selected.clone();
        s
    });
    if !saved {
        if let Some(s) = &suggested {
            plan = s.clone();
        }
    }
    if plan.iface.is_empty() {
        plan.iface = selected.clone();
    }

    let note = if ifaces.is_empty() {
        "本机没找到可用于上网的网卡（虚拟/环回接口已排除）".to_string()
    } else {
        String::new()
    };
    let persist = hostnic::persist_commands(&plan, platform);

    HostNicView {
        modem_id: modem.id,
        model: modem.model().unwrap_or_default(),
        required,
        reason: reason.to_string(),
        os: std::env::consts::OS.to_string(),
        elevated: hostnic::elevated(),
        interfaces: ifaces,
        plan: plan.clone(),
        saved,
        saved_plans,
        suggested,
        modem_ip,
        actions: hostnic::action_sets(&plan, platform),
        note,
        platform: platform.to_string(),
        openwrt,
        persist_hint: if openwrt && !persist.is_empty() {
            "这些命令只生成不执行：运行期的 ip addr/route 会被 netifd 的 reload 抹掉，\
             要真正持久就写进 /etc/config/network（接口名与防火墙分区请自己看一眼再贴）。"
                .to_string()
        } else {
            String::new()
        },
        persist,
        checked_at: timestamp(),
    }
}

/// 生成（并按需执行）一组动作：`mode` 见 `hostnic::MODE_*`。
///
/// 执行只在 `execute=true` 时进行，且走 `prog + args`（不拼 shell）；
/// 没有权限时命令照样生成 —— 页面提示"复制出去手工执行"。
pub fn apply(
    state: &AppState,
    modem: &Modem,
    plan: &hostnic::HostNicPlan,
    mode: &str,
    execute: bool,
) -> HostNicApply {
    let platform = hostnic::platform();
    let mut commands = hostnic::commands(mode, plan, platform);
    let label = match mode {
        hostnic::MODE_RENEW => "先试 DHCP",
        hostnic::MODE_STATIC => "应用静态配置",
        hostnic::MODE_DHCP => "恢复自动获取",
        other => other,
    }
    .to_string();
    let bad_plan = hostnic::validate_plan(&hostnic::normalize_plan(plan)).err();
    if let Some(e) = bad_plan {
        // 半套命令比没有更糟：配置不合法就一条都不给
        return HostNicApply {
            label,
            executed: false,
            note: e.to_string(),
            commands: Vec::new(),
            results: Vec::new(),
        };
    }
    let note = if execute && !hostnic::elevated() {
        "本进程没有管理员 / root 权限：命令已生成，请复制出去手工执行".to_string()
    } else {
        String::new()
    };
    let results = if execute && hostnic::elevated() {
        commands.iter().map(hostnic::run_command).collect()
    } else {
        Vec::new()
    };
    // 执行完视图里的地址可能变了：调用方会重新取一次 view
    let _ = state;
    let _ = modem;
    HostNicApply {
        label,
        executed: !results.is_empty(),
        note,
        commands: std::mem::take(&mut commands),
        results,
    }
}

/// 一组动作的（生成/执行）结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostNicApply {
    pub label: String,
    pub executed: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub commands: Vec<hostnic::HostNicCommand>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<hostnic::HostNicResult>,
}
