//! 各面板的推送：把「刚查到的数据」顺手广播给在线页面。
//!
//! 与 `handlers/*` 的分工：handler 管取参数与回信封，这里管「广播 + 记录波动」。
//! 记录点复用同一次查询的结果，**不额外发 AT**（见 `modem/metrics.rs`）。

use serde_json::{json, Value};

use super::AppState;
use crate::modem::model::{CellScanResult, LockStatus, Modem, NetworkStatus, Sms, UssdResult};

/// 推模组列表（热插拔 / 启动 / 身份信息刷新后调用）。
pub fn push_modems(state: &AppState) {
    let data = serde_json::to_value(state.mgr.list()).unwrap_or(Value::Null);
    state.ws.broadcast("modems", None, data);
}

/// 推信号面板；顺手记录信号波动（阈值 + 等级判断，复用同一次采样）。
pub fn push_signal(state: &AppState, modem: &Modem) {
    let sig = modem.signal();
    match sig {
        Ok(sig) => {
            let entries = state.metrics.note_signal(modem.id, &sig);
            super::handlers::push_changes(state, modem.id, &entries);
            let data = serde_json::to_value(sig).unwrap_or(Value::Null);
            // 顺手进缓存：/api/snapshot 就能立即读到，不用再发一次 AT
            state.cache.put("signal", modem.id, data.clone());
            state.ws.broadcast("signal", Some(modem.id), data);
        }
        Err(e) => log::debug!("后台查信号失败（模组 {}）: {e}", modem.id),
    }
}

/// 推网络状态（查到就顺手广播，避免重复查询）；同样记录网络波动。
pub fn push_network_state(state: &AppState, modem: &Modem, st: &NetworkStatus) {
    let entries = state.metrics.note_network(modem.id, st);
    super::handlers::push_changes(state, modem.id, &entries);
    let data = serde_json::to_value(st).unwrap_or(Value::Null);
    state.cache.put("network", modem.id, data.clone());
    state.ws.broadcast("network", Some(modem.id), data);
}

/// 后台循环用：查一次网络状态并推送（查失败只记日志，不打扰别人）。
pub fn push_network(state: &AppState, modem: &Modem) {
    let st = modem.network_status(state.cfg.net.allow_roaming);
    push_network_state(state, modem, &st);
}

/// 推上行 / 下行速度（每轮周期推送都会采样，前端才能持续看到实时速度）。
pub fn push_traffic(state: &AppState, modem: &Modem) {
    let view = modem.traffic_view();
    if !view.supported {
        return;
    }
    let data = serde_json::to_value(view).unwrap_or(Value::Null);
    state.ws.broadcast("traffic", Some(modem.id), data);
}

/// 推流量配额状态。
pub fn push_quota(state: &AppState, modem: &Modem, view: &Value) {
    state.ws.broadcast("quota", Some(modem.id), view.clone());
}

/// 推 SIM 热插拔事件（拔卡 / 插卡 / 换卡；`kind` 见 model/sim.rs）。
///
/// 前端只拿 `kind` 与 `sim.iccid`：拔卡时清屏、换卡时提示，所以载荷刻意做小。
pub fn push_sim(state: &AppState, modem: &Modem, kind: &str) {
    let ident = modem.sim_identity();
    let data = json!({
        "modem_id": modem.id,
        "kind": kind,
        "sim": {
            "present": ident.present,
            "state": ident.state,
            "iccid": ident.iccid,
            "imsi": ident.imsi,
        },
    });
    state.ws.broadcast("sim", Some(modem.id), data);
}

/// 推日志文件的新增行（`lines` 为空时什么都不做）。
pub fn push_logs(state: &AppState, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let data = json!({ "file": state.cfg.server.log_file, "lines": lines });
    state.ws.broadcast("logs", None, data);
}

/// 推基站扫描结果（「小区」卡片；主动扫网结束后调用，页面立即拿到新结果）。
pub fn push_cells(state: &AppState, modem: &Modem, result: &CellScanResult) {
    let data = serde_json::to_value(result).unwrap_or(Value::Null);
    state.cache.put("cells", modem.id, data.clone());
    state.ws.broadcast("cells", Some(modem.id), data);
}

/// 推锁网状态（锁频段 / 锁小区 / 改模式 / 全解锁成功后调用）。
///
/// 锁网是模组级状态，改一次所有在线页面都该看到，所以顺手广播。
pub fn push_lock(state: &AppState, modem: &Modem, st: &LockStatus) {
    let data = serde_json::to_value(st).unwrap_or(Value::Null);
    state.cache.put("lock", modem.id, data.clone());
    state.ws.broadcast("lock", Some(modem.id), data);
}

/// 推短信列表（读短信 / 换卡后调用）。
///
/// 短信密码已经取消（2026-10-19，用户要求）：内容与角标一样走普通广播，
/// 访问控制只剩外层「要不要登录路由器」这一道。
pub fn push_sms(state: &AppState, modem: &Modem, list: &[Sms]) {
    let data = serde_json::to_value(list).unwrap_or(Value::Null);
    state.ws.broadcast("sms", Some(modem.id), data);
}

/// 推短信条数角标（`{total, unread}`）。
pub fn push_smscount(state: &AppState, modem: &Modem) {
    let (total, unread) = modem.sms_counts();
    let data = json!({ "total": total, "unread": unread });
    state.ws.broadcast("smscount", Some(modem.id), data);
}

/// 屏端「组网模式」要的那份信息（`mode` 消息）。
///
/// 与 `/api/mode` 的区别：屏宽 320px，右上角那行放不下"自动（2G/3G/4G/5G）"这种
/// 全名，所以额外给一个**短名**；`screen` 是那三个按钮（值 + 4G/5G/4G+5G + 本机
/// 能不能写），`choices` 是全部可选档位（排查用）。`msg` 是最近一次操作的一句话
/// 反馈（"已切换到 仅 5G（NR）" / "设置失败：…"），空串表示没话说。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DisplayMode {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub short: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub msg: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<DisplayModeChoice>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub screen: Vec<DisplayModeChoice>,
    #[serde(skip_serializing_if = "is_false")]
    pub read_only: bool,
}

/// 一个可选档位（屏端按钮与排查列表共用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DisplayModeChoice {
    pub value: String,
    pub name: String,
    /// 本机这一档能不能写（展锐只有两档，缺的那档按钮要置灰）
    pub writable: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 屏端三个按钮的语义值（顺序 = 界面从左到右）。
const SCREEN_MODES: [(&str, &str); 3] = [("LTE", "4G"), ("NR5G", "5G"), ("LTE:NR5G", "4G+5G")];

/// 语义值 → 屏端右上角用的**短名**（长名放不下，见 [`DisplayMode`]）。
pub fn mode_short(value: &str) -> String {
    match value.to_ascii_uppercase().as_str() {
        "AUTO" => "自动".to_string(),
        "WCDMA" => "仅3G".to_string(),
        "LTE" => "仅4G".to_string(),
        "NR5G" => "仅5G".to_string(),
        "WCDMA:LTE" => "3G+4G".to_string(),
        "WCDMA:NR5G" => "3G+5G".to_string(),
        "LTE:NR5G" | "NR5G:LTE" => "4G+5G".to_string(),
        other => other.to_string(),
    }
}

/// 组装屏端的组网模式信息（读一次当前值 + 本机可选档位）。
pub fn display_mode(modem: &Modem, msg: String) -> DisplayMode {
    let pref = modem.mode_pref();
    let choices: Vec<DisplayModeChoice> = pref
        .options
        .iter()
        .map(|o| DisplayModeChoice {
            value: o.value.clone(),
            name: o.label.clone(),
            writable: pref.supported,
        })
        .collect();
    let screen = SCREEN_MODES
        .iter()
        .map(|(v, label)| DisplayModeChoice {
            value: (*v).to_string(),
            name: (*label).to_string(),
            // 本机档位表里没有这一档 ⇒ 置灰（点了必定失败，别让用户点）
            writable: pref.supported && pref.options.iter().any(|o| o.value == *v),
        })
        .collect();
    DisplayMode {
        value: pref.value.clone(),
        name: pref.value_text.clone(),
        short: mode_short(&pref.value),
        msg,
        choices,
        screen,
        read_only: !pref.supported,
    }
}

/// 推屏端组网模式（`mode` 消息）。
pub fn push_mode(state: &AppState, modem: &Modem, msg: String) {
    let data = serde_json::to_value(display_mode(modem, msg)).unwrap_or(Value::Null);
    state.ws.broadcast("mode", Some(modem.id), data);
}

/// 推屏端「发短信」结果（`sendsms` 消息：号码 / 正文 / 成败 / 一句话反馈）。
pub fn push_sendsms(
    state: &AppState,
    modem: &Modem,
    number: &str,
    text: &str,
    err: Option<String>,
) {
    let data = json!({
        "number": number,
        "text": text,
        "ok": err.is_none(),
        "msg": match &err {
            Some(e) => format!("发送失败：{e}"),
            None => "已发送，等运营商回复".to_string(),
        },
    });
    state.ws.broadcast("sendsms", Some(modem.id), data);
}

/// 推 USSD 应答（`+CUSD` 到达即推，页面据此渲染）。
pub fn push_ussd(state: &AppState, modem: &Modem, result: &UssdResult) {
    let data = serde_json::to_value(result).unwrap_or(Value::Null);
    state.ws.broadcast("ussd", Some(modem.id), data);
}

#[cfg(test)]
mod tests {
    use super::{mode_short, DisplayMode, DisplayModeChoice, SCREEN_MODES};

    #[test]
    fn screen_short_names_fit_the_tiny_corner() {
        assert_eq!(mode_short("AUTO"), "自动");
        assert_eq!(mode_short("lte"), "仅4G");
        assert_eq!(mode_short("NR5G"), "仅5G");
        assert_eq!(mode_short("LTE:NR5G"), "4G+5G");
        assert_eq!(mode_short("nr5g:lte"), "4G+5G");
        assert_eq!(mode_short("WCDMA"), "仅3G");
        assert_eq!(mode_short("WCDMA:LTE"), "3G+4G");
        assert_eq!(mode_short(""), "");
        // 屏端那三个按钮必须在短名表里 —— 否则右上角会显示原始码
        for (v, _) in SCREEN_MODES {
            assert!(!mode_short(v).is_empty(), "{v} 缺短名");
        }
    }

    #[test]
    fn display_mode_json_shape() {
        let m = DisplayMode {
            value: "LTE".into(),
            name: "仅 4G LTE".into(),
            short: "仅4G".into(),
            msg: String::new(),
            choices: vec![],
            screen: vec![DisplayModeChoice {
                value: "LTE".into(),
                name: "4G".into(),
                writable: true,
            }],
            read_only: false,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["value"], "LTE");
        assert_eq!(v["short"], "仅4G");
        assert_eq!(v["screen"][0]["writable"], true);
        // 空串 / false 一律不出现（屏端按"字段在不在"判断要不要画那一行）
        assert!(v.get("msg").is_none());
        assert!(v.get("choices").is_none());
        assert!(v.get("read_only").is_none());

        let m = DisplayMode {
            msg: "设置失败：模组拒绝".into(),
            read_only: true,
            ..m
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["msg"], "设置失败：模组拒绝");
        assert_eq!(v["read_only"], true);
    }
}
