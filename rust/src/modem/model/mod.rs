//! 模组模型：领域对象、查询与对外视图。

mod addr;

pub(crate) use addr::format_ipv6_bytes;
mod airplane;
mod band;
mod bind;
pub mod ca;
mod cells;
pub mod changes;
mod device;
pub mod dial;
pub mod history;
mod imei;
mod lock;
mod mode;
mod modem;
mod netscan;
mod netselect;
mod network;
mod params;
// 拨号 APN 优选：一键拨号 / 自动拨号守护 / 拨号器托管三方共用（见 params.rs）
pub use params::pick_dial_context;
pub mod quota;
mod regstate;
mod roam;
pub mod schedule;
mod signal;
pub mod sim;
mod siminfo;
mod sms;
mod traffic;
mod urc;
mod usbnet;
mod ussd;
mod view;

pub use addr::{is_link_local_v6, is_valid_ipv4, looks_v6, normalize_ipv6};
pub use airplane::Airplane;
pub use band::{
    band_label, bandwidth_text, lte_band_from_code, lte_band_from_earfcn, nr_band_from_arfcn,
    nr_band_from_code,
};
pub use bind::{
    bind_l2p, unbind_l2p, BindEntry, BindState, EMBIND_READ, EMBIND_TEST, L2P_CCMNI, L2P_LHIF,
    L2P_MBIM, L2P_RNDIS,
};
pub use ca::{parse_ca, CaInfo};
pub use cells::{CellInfo, CellScanResult};
pub use changes::{ChangeEntry, ChangeState, GLOBAL_MAX};
pub use device::{ClockSyncResult, RebootResult};
pub use dial::usb_netifs_for_port;
pub use dial::{
    dial_kind_from_name, dial_kind_label, dial_kind_of, dial_state_text, dial_type_by_kind,
    dial_type_by_value, dial_types, valid_cid, valid_pdp_type, DialRequest, DialState, DialStep,
    DialType, DialTypeInfo, HostIfaceHint, IfaceInfo, UsbDesc,
};
pub use history::{HistoryStore, Sample, SAMPLE_MAX};
pub use imei::{ImeiStatus, ModemInfo};
pub use lock::{BandLock, CellLock, LockCaps, LockStatus};
pub use mode::ModePref;
pub use modem::{at_resp_is_error, split_at_lines};
pub use modem::{is_fibocom_350, Modem};
pub use netscan::{NetScanResult, Operator};
pub use netselect::NetSelect;
pub use network::{NetworkInfo, NetworkStatus};
pub use params::ParamsStatus;
pub use quota::{QuotaSettings, QuotaState};
pub use regstate::RegState;
pub use roam::RoamView;
pub use schedule::{
    action_text, action_views, actions_label, dedupe_actions, due_phases, earfcn_valid,
    kind_info_for, kind_label, kinds, looks_like_mode_code, new_id, next_runs, parse_hhmm,
    parse_lock_tech, quota_key, task_view_of, validate_action, validate_task, weekday_cn, Action,
    ActionView, KindInfo, ModemOption, Phase, Run, SchedView, Task, TaskView, CATCH_UP,
    KIND_AIRPLANE_OFF, KIND_AIRPLANE_ON, KIND_BAND_LOCK, KIND_BAND_UNLOCK, KIND_CELL_LOCK,
    KIND_CELL_UNLOCK, KIND_MODE_PREF, KIND_UNLOCK_ALL, LOG_EVERY, MAX_ACTIONS, MAX_TASKS,
    MODE_PREFS, NEXT_SHOW, PHASE_END, PHASE_MANUAL, PHASE_START, SCHED_SAVE_EVERY,
};
pub use signal::SignalInfo;
pub use sim::{
    cpin_present, should_check_sim, should_reset_modem, sim_hotswap_event, sim_identity_differs,
    SimIdentity, SIM_COOLDOWN, SIM_INSERTED, SIM_REMOVED, SIM_SWAPPED, SIM_WATCH_TICK,
};
pub use siminfo::{ModemStatus, RoamState, Sensor};
pub use sms::{Sms, SmsPart, SmsStorage};
pub use traffic::{TrafficSample, TrafficView, KIND_COUNTER, KIND_NONE, KIND_RATE};
pub use urc::{kind_stats, urc_actions, KindStat, UrcAction};
pub use usbnet::UsbNetMode;
pub use ussd::{parse_cusd_line, valid_ussd_code, UssdResult};
pub use view::ModemView;
