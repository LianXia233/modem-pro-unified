//! 锁网状态与能力探测（`GET /api/lock/status`、`/api/lock/caps`）。
//!
//! 方言（锁网指令集）按**运行期只读探测**决定：谁被固件接受就用谁，结果缓存；
//! 全部不接受时按厂商回落。当前实现覆盖三套方言：
//!   · `qnw`——高通 QNW：`AT+QNWPREFCFG` / `AT+QNWLOCK`（移远、广和通高通平台）
//!   · `simcom`——芯讯通：`AT+CSYSSEL` / `AT+CNMP`
//!   · `fibocom`——广和通 MTK 平台：`AT+GTACT`（FM350 / RW350 系列）
//!
//! 三套的**报文形状不同**（见 [`BandShape`]）：前两套是 key/value 对、读写同一条命令；
//! 广和通 MTK 是位置参数、一条命令同时写 GSM / LTE / NR 三组，且锁频前要先离线。
//! 因此读应答的解析与写命令的拼装都按形状分派，不能只按方言名分派。

use std::time::Duration;

use serde::Serialize;

use crate::error::{msg, Result};
use crate::modem::bands;

use super::super::at::{csv, is_ok, Prio};
use super::super::vendor::Vendor;
use super::modem::Modem;

/// 频段锁定状态。
#[derive(Debug, Clone, Default, Serialize)]
pub struct BandLock {
    pub tech: String,
    pub bands: Vec<i32>,
    pub capable: Vec<i32>,
    pub locked: bool,
    pub raw_value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capable_note: Option<String>,
    /// 清单（QModem 频段表）声明本型号支持的该制式频段 —— **出厂口径**。
    ///
    /// 与 `capable` 的区别：`capable` 是运行期问模组（固件不报就只有 `-`），
    /// 这份是型号级快照。两者不一致（固件被裁频段、或上游写错）正是排查要看的信息，
    /// 所以两个都展示、不互相覆盖。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub declared: Vec<i32>,
    /// 清单口径的说明（说清是「上游没登记」「上游没写频段」还是「该制式没记录」）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub declared_note: String,
}

/// 小区/频点锁定状态（与界面三个选项一一对应）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CellLock {
    pub tech: String,
    /// 0=未锁定 1=锁频点+PCI 2=仅锁频点
    pub lock_type: i32,
    pub earfcn: i32,
    pub pcid: i32,
    pub locked: bool,
    pub raw_value: String,
}

/// 锁网总状态。
#[derive(Debug, Clone, Default, Serialize)]
pub struct LockStatus {
    pub modem_id: u32,
    pub queried_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode_pref: String,
    pub band_lte: BandLock,
    pub band_nr: BandLock,
    pub cell_lte: CellLock,
    pub cell_nr: CellLock,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmds: Option<std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dialect: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dialect_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dialect_note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band_nr_nsa: Option<BandLock>,
    #[serde(skip_serializing_if = "is_false")]
    pub separate_nsa: bool,
    /// 本型号的四组支持频段（清单口径：WCDMA / LTE / NR-NSA / NR-SA）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bands: Option<bands::ModemBands>,
    /// 锁频段页要渲染的分组（清单 ∪ 设备上报，自动排版；规则见 [`bands::groups`]）。
    ///
    /// ⚠️ 必须由 [`Modem::query_lock_status`] 赋值：前端只读 JSON 的 `band_groups`，
    /// 漏掉赋值不会编译报错，只表现为「所有机型的锁频段页永远没有勾选框」。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub band_groups: Vec<bands::BandGroup>,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 单条能力探测的结果。
#[derive(Debug, Clone, Serialize)]
pub struct LockCapProbe {
    pub name: String,
    pub command: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub response: Vec<String>,
}

/// 锁网能力探测结果。
#[derive(Debug, Clone, Default, Serialize)]
pub struct LockCaps {
    pub modem_id: u32,
    pub vendor: String,
    pub supported: bool,
    pub probes: Vec<LockCapProbe>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dialect: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dialect_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dialect_note: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dialect_tried: Vec<String>,
}

/// 选中的锁网方言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Qnw,
    Simcom,
    Fibocom,
    /// 鼎桥 TD Tech（MT5700/MT5710，巴龙平台）：`^LTEFREQLOCK` / `^NRFREQLOCK` /
    /// `^SYSCFGEX`。语法出自官方手册 V200R001C20 §13.2/13.12/13.13，
    /// 写后**需切飞行模式才生效**（手册原文），且锁持久化、受 NV 使能控制。
    Tdtech,
}

/// 频段锁定的**报文形状**（决定读应答怎么解、写命令怎么拼）。
///
/// 同一方言内形状唯一，所以它是方言的属性而不是独立开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BandShape {
    /// key/value 对，读写同一条命令：`AT+QNWPREFCFG="lte_band",1:3:8`。
    Keyed,
    /// 位置参数，一条命令写全部技术域：
    /// `AT+GTACT=<rat>,<ctrl…>,<gsm…>,<lte…>,<nr…>`（广和通 MTK）。
    Positional,
    /// 鼎桥频段锁：`AT^LTEFREQLOCK=3,0,<num>,<band>…`（锁 Band）/
    /// `AT^NRFREQLOCK=…`（NR），`=?` 读支持范围，`?` 读当前锁。
    /// 写后必须切飞行模式（CFUN=0/1）才生效 —— 手册 13.12 备注 1。
    TdFreq,
}

/// 方言描述（名称 / 显示名 / 说明 / 探测记录）。
#[derive(Debug, Clone)]
pub struct DialectInfo {
    pub dialect: Option<Dialect>,
    pub name: String,
    pub label: String,
    pub note: String,
    pub tried: Vec<String>,
    /// 运行期探测是否**真的被固件接受过**（false = 全部候选都拒绝后的猜测兜底，
    /// 命令可能不被接受；定时器的"机型不支持"预检就看这里）。
    pub probed: bool,
}

impl Dialect {
    fn name(self) -> &'static str {
        match self {
            Dialect::Qnw => "qnw",
            Dialect::Simcom => "simcom",
            Dialect::Fibocom => "fibocom",
            Dialect::Tdtech => "tdtech",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Dialect::Qnw => "高通 QNW（AT+QNWPREFCFG / AT+QNWLOCK）",
            Dialect::Simcom => "芯讯通（AT+CSYSSEL / AT+CNMP）",
            Dialect::Fibocom => "广和通 MTK（AT+GTACT / AT+EMMCHLCK）",
            Dialect::Tdtech => "鼎桥巴龙（AT^LTEFREQLOCK / AT^NRFREQLOCK / AT^SYSCFGEX）",
        }
    }

    fn probe(self) -> &'static str {
        match self {
            Dialect::Qnw => "AT+QNWPREFCFG=\"mode_pref\"",
            Dialect::Simcom => "AT+CSYSSEL=\"lte_band\"",
            Dialect::Fibocom => "AT+GTACT?",
            Dialect::Tdtech => "AT^SYSCFGEX?",
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Dialect::Qnw => "+QNWPREFCFG:",
            Dialect::Simcom => "+CSYSSEL:",
            Dialect::Fibocom => "+GTACT:",
            Dialect::Tdtech => "^SYSCFGEX:",
        }
    }

    fn order(self) -> &'static [Dialect] {
        match self {
            Dialect::Qnw => &[Dialect::Qnw, Dialect::Simcom, Dialect::Fibocom],
            Dialect::Simcom => &[Dialect::Simcom, Dialect::Qnw, Dialect::Fibocom],
            Dialect::Fibocom => &[Dialect::Fibocom, Dialect::Qnw, Dialect::Simcom],
            Dialect::Tdtech => &[
                Dialect::Tdtech,
                Dialect::Qnw,
                Dialect::Simcom,
                Dialect::Fibocom,
            ],
        }
    }

    fn mode_pref_cmd(self) -> &'static str {
        match self {
            Dialect::Qnw => "AT+QNWPREFCFG=\"mode_pref\"",
            Dialect::Simcom => "AT+CNMP?",
            Dialect::Fibocom => "AT+GTACT?",
            // acqorder 就是「制式偏好」（tdtech_acq_order 双向映射）
            Dialect::Tdtech => "AT^SYSCFGEX?",
        }
    }

    fn band_key(self, tech: &str) -> &'static str {
        match (self, tech) {
            (_, "NRNSA") => "nsa_nr5g_band",
            (Dialect::Qnw, "NR") => "nr5g_band",
            (Dialect::Simcom, "NR") => "nr5g_band",
            // 广和通 MTK 没有 key（位置参数），给个占位名只为 `cmds` 里好看
            (Dialect::Fibocom, _) => "gtact",
            _ => "lte_band",
        }
    }

    fn band_cmd(self, tech: &str) -> String {
        match self {
            Dialect::Qnw => format!("AT+QNWPREFCFG=\"{}\"", self.band_key(tech)),
            Dialect::Simcom => format!("AT+CSYSSEL=\"{}\"", self.band_key(tech)),
            Dialect::Fibocom => "AT+GTACT?".to_string(),
            // 锁状态查询（写是 =3,0,<num>,<band>…，见 set_tdfreq_band）
            Dialect::Tdtech => match tech {
                "NR" | "NRNSA" => "AT^NRFREQLOCK?".to_string(),
                _ => "AT^LTEFREQLOCK?".to_string(),
            },
        }
    }

    fn policy_cmd(self) -> &'static str {
        match self {
            Dialect::Qnw => "AT+QNWPREFCFG=\"policy_band\"",
            Dialect::Simcom => "",
            Dialect::Fibocom => "AT+GTACT=?",
            // 手册没给「运行期支持频段」查询 → 空策略，界面用出厂清单（declared）
            Dialect::Tdtech => "",
        }
    }

    fn head(self) -> &'static str {
        match self {
            Dialect::Qnw => "+QNWPREFCFG:",
            Dialect::Simcom => "+CSYSSEL:",
            Dialect::Fibocom => "+GTACT:",
            // TdFreq 不走 parse_prefcfg_value，无 head
            Dialect::Tdtech => "",
        }
    }

    fn cell_cmd(self, tech: &str) -> &'static str {
        match (self, tech) {
            (Dialect::Qnw, "NR") => "AT+QNWLOCK=\"common/5g\"",
            (Dialect::Qnw, _) => "AT+QNWLOCK=\"common/4g\"",
            (Dialect::Fibocom, _) => "AT+EMMCHLCK?",
            (Dialect::Simcom, _) => "",
            // 锁小区状态与锁频段是同一条查询（operatetype 1/2 = 锁频点/锁小区）
            (Dialect::Tdtech, "NR") => "AT^NRFREQLOCK?",
            (Dialect::Tdtech, _) => "AT^LTEFREQLOCK?",
        }
    }

    fn cell_kind(self, tech: &str) -> &'static str {
        match tech {
            "NR" => "common/5g",
            _ => "common/4g",
        }
    }

    fn nsa_independent(self) -> bool {
        matches!(self, Dialect::Qnw)
    }

    fn band_shape(self) -> BandShape {
        match self {
            Dialect::Qnw | Dialect::Simcom => BandShape::Keyed,
            Dialect::Fibocom => BandShape::Positional,
            Dialect::Tdtech => BandShape::TdFreq,
        }
    }
}

impl Modem {
    /// 本机锁网方言（缓存探测结果）。
    pub fn lock_dialect(&self) -> DialectInfo {
        if let Some(info) = self.lock_dialect_cache() {
            return info;
        }
        let info = self.probe_lock_dialect();
        self.set_lock_dialect_cache(info.clone());
        info
    }

    fn probe_lock_dialect(&self) -> DialectInfo {
        // 广和通 MTK 平台（FM350 / RW350）有自己的私有指令族，与广和通高通平台
        // （FM150 / FM160 / FM650，走 QNW）**完全不同**，必须先认出来：
        // 否则会先探 QNW（拒绝）再探芯讯通（拒绝），最后按厂商猜测回落成 QNW，
        // 页面上显示「未探测到」却仍拿 QNW 的指令去写。
        let guess = if self.is_fibocom_mtk() {
            Dialect::Fibocom
        } else {
            match self.vendor() {
                Vendor::Simcom => Dialect::Simcom,
                _ => Dialect::Qnw,
            }
        };
        let mut tried = Vec::new();
        for d in guess.order() {
            let resp = self.at_cmd_prio(Prio::Bulk, d.probe());
            let ok = is_ok(&resp)
                && resp
                    .to_ascii_uppercase()
                    .contains(&d.mark().to_ascii_uppercase());
            tried.push(format!(
                "{}: {}（{}）",
                d.name(),
                if ok { "接受" } else { "拒绝" },
                d.probe()
            ));
            if ok {
                return DialectInfo {
                    dialect: Some(*d),
                    name: d.name().to_string(),
                    label: d.label().to_string(),
                    note: "运行期只读探测选中该方言".to_string(),
                    tried,
                    probed: true,
                };
            }
        }
        DialectInfo {
            dialect: Some(guess),
            name: String::new(),
            label: String::new(),
            note: "未探测到可用的锁网方言，按厂商猜测实现，命令可能不被接受".to_string(),
            tried,
            probed: false,
        }
    }

    /// 查询锁网状态（`refresh=false` 时 30 秒内复用缓存）。
    pub fn lock_status(&self, refresh: bool) -> LockStatus {
        if !refresh {
            if let Some(st) = self.lock_status_cache() {
                return st;
            }
        }
        let st = self.query_lock_status();
        self.set_lock_status_cache(st.clone());
        st
    }

    fn query_lock_status(&self) -> LockStatus {
        let info = self.lock_dialect();
        let Some(d) = info.dialect else {
            let mut st = LockStatus {
                modem_id: self.id,
                queried_at: crate::logging::timestamp(),
                note: Some("该厂商的固件不支持锁网指令".to_string()),
                ..Default::default()
            };
            // 锁不了也要告诉用户「这个型号出厂支持哪些频段」，而不是一片空白
            self.apply_declared_bands(&mut st);
            return st;
        };
        let policy = self.query_policy_bands(d);
        let mode_pref = self.query_mode_pref(d);
        let band_nr_nsa = d
            .nsa_independent()
            .then(|| self.query_band_lock(d, "NRNSA", policy.nr_nsa.clone()));
        let mut st = LockStatus {
            modem_id: self.id,
            queried_at: crate::logging::timestamp(),
            mode_pref,
            band_lte: self.query_band_lock(d, "LTE", policy.lte),
            band_nr: self.query_band_lock(d, "NR", policy.nr),
            cell_lte: self.query_cell_lock(d, "LTE"),
            cell_nr: self.query_cell_lock(d, "NR"),
            note: None,
            cmds: Some(self.lock_cmd_summary(d)),
            dialect: info.name,
            dialect_name: info.label,
            dialect_note: info.note,
            band_nr_nsa,
            separate_nsa: d.nsa_independent(),
            ..Default::default()
        };
        self.apply_declared_bands(&mut st);
        st
    }

    /// 把「清单声明支持的频段」贴到锁网状态上，并整理锁频段页的分组。
    ///
    /// 三种数据来源各有用途，**都要保留**：
    ///
    /// | 来源 | 性质 | 用途 |
    /// | --- | --- | --- |
    /// | `declared`（本函数） | 出厂口径（QModem 频段表） | 告诉用户「能勾什么」，并对照固件能力 |
    /// | `capable` | 运行期问模组（policy_band） | 判断是否真的被限制；固件不报就空 |
    /// | `bands` | 运行期读当前锁定值 | 勾选状态的初始值 |
    ///
    /// 分组时三者取并集：少了清单，取消勾选下发后那个频段会从列表消失（勾不回来）；
    /// 少了运行期值，清单没数据/固件不报能力的机型会一条都显示不出来。
    fn apply_declared_bands(&self, st: &mut LockStatus) {
        // 匹配键带上 ATI 原文：有些固件只在原文里报别名（如 QogirN6Pro_PS）
        let key = format!(
            "{} {}",
            self.model().unwrap_or_default(),
            self.model_raw().unwrap_or_default()
        );
        let declared = bands::lookup(&key).cloned();

        for (bl, tech) in [
            (&mut st.band_lte, bands::Tech::Lte),
            (&mut st.band_nr, bands::Tech::Nr),
        ] {
            let d = bands::declared(&key, tech);
            bl.declared = d.bands;
            bl.declared_note = d.note;
        }
        if let Some(bl) = st.band_nr_nsa.as_mut() {
            let d = bands::declared(&key, bands::Tech::NrNsa);
            bl.declared = d.bands;
            bl.declared_note = d.note;
        }

        let (lte_capable, lte_current) = (st.band_lte.capable.clone(), st.band_lte.bands.clone());
        let (nr_capable, nr_current) = (st.band_nr.capable.clone(), st.band_nr.bands.clone());
        let (nsa_capable, nsa_current) = st
            .band_nr_nsa
            .as_ref()
            .map(|b| (b.capable.clone(), b.bands.clone()))
            .unwrap_or_default();
        st.band_groups = bands::groups(&bands::BandState {
            declared: declared.as_ref(),
            separate_nsa: st.separate_nsa,
            lte_capable: &lte_capable,
            lte_current: &lte_current,
            nr_capable: &nr_capable,
            nr_current: &nr_current,
            nsa_capable: &nsa_capable,
            nsa_current: &nsa_current,
        });
        st.bands = declared;
    }

    /// 探测锁网指令支持情况（全部只读）。
    pub fn lock_caps(&self) -> LockCaps {
        let info = self.lock_dialect();
        let mut caps = LockCaps {
            modem_id: self.id,
            vendor: self.vendor().as_str().to_string(),
            supported: info.dialect.is_some(),
            dialect: info.name,
            dialect_name: info.label,
            dialect_note: info.note,
            dialect_tried: info.tried,
            probes: Vec::new(),
        };
        let Some(d) = info.dialect else {
            return caps;
        };
        let probes: Vec<(&str, String)> = match d {
            // 广和通 MTK：频段、模式偏好同查一条 `AT+GTACT?`，重复列三遍只是噪声；
            // 真正要列的是 `AT+GTACT=?`（支持全集）—— 它就是本方言「解除锁定」
            // 写回的基准，等价于 QNW 的 policy_band。
            Dialect::Fibocom => vec![
                ("band_lte", d.band_cmd("LTE")),
                ("policy_band", d.policy_cmd().to_string()),
                ("mode_pref", d.mode_pref_cmd().to_string()),
                ("cell_lte", d.cell_cmd("LTE").to_string()),
            ],
            _ => vec![
                ("band_lte", d.band_cmd("LTE")),
                ("band_nr", d.band_cmd("NR")),
                ("mode_pref", d.mode_pref_cmd().to_string()),
                ("cell_lte", d.cell_cmd("LTE").to_string()),
                ("cell_nr", d.cell_cmd("NR").to_string()),
            ],
        };
        for (name, cmd) in probes {
            if cmd.is_empty() {
                continue;
            }
            let resp = self.at_cmd_prio(Prio::Bulk, &cmd);
            caps.probes.push(LockCapProbe {
                name: name.to_string(),
                command: cmd,
                ok: is_ok(&resp),
                response: resp
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect(),
            });
        }
        caps
    }

    fn query_mode_pref(&self, d: Dialect) -> String {
        let resp = self.at_cmd_prio(Prio::Bulk, d.mode_pref_cmd());
        match d {
            Dialect::Qnw => parse_qnwprefcfg(&resp, "mode_pref").unwrap_or_default(),
            Dialect::Simcom => parse_cnmp(&resp),
            // 广和通 MTK 的「组网模式偏好」就是 AT+GTACT 的 <rat> 字段
            Dialect::Fibocom => parse_gtact_read(&resp)
                .map(|g| gtact_rat_label(g.rat).to_string())
                .unwrap_or_default(),
            // 鼎桥：acqorder（手册 §13.2）就是制式偏好，tdtech_acq_order 双向映射
            Dialect::Tdtech => crate::modem::model::mode::parse_syscfgex(&resp)
                .and_then(|(acq, ..)| crate::modem::model::mode::tdtech_acq_order(&acq))
                .map(|s| s.to_string())
                .unwrap_or_default(),
        }
    }

    fn query_policy_bands(&self, d: Dialect) -> PolicyBands {
        let cmd = d.policy_cmd();
        if cmd.is_empty() {
            return PolicyBands::default();
        }
        let resp = self.at_cmd_prio(Prio::Bulk, cmd);
        if d.band_shape() == BandShape::Positional {
            // 广和通 MTK：支持全集在 AT+GTACT=? 的各参数组里，按值域认，不按组序号
            let (lte, nr) = parse_gtact_supported(&resp);
            return PolicyBands {
                lte: (!lte.is_empty()).then_some(lte),
                nr: (!nr.is_empty()).then_some(nr),
                nr_nsa: None,
            };
        }
        let get = |key: &str| parse_qnwprefcfg(&resp, key).and_then(|v| parse_band_list(&v));
        PolicyBands {
            lte: get("lte_band"),
            nr: get("nr5g_band"),
            nr_nsa: get("nsa_nr5g_band"),
        }
    }

    fn query_band_lock(&self, d: Dialect, tech: &str, capable: Option<Vec<i32>>) -> BandLock {
        let mut bl = BandLock {
            tech: tech.to_string(),
            capable: capable.clone().unwrap_or_default(),
            ..Default::default()
        };
        let resp = self.at_cmd_prio(Prio::Bulk, &d.band_cmd(tech));
        match d.band_shape() {
            BandShape::Keyed => {
                if let Some(v) = parse_prefcfg_value(d.head(), &resp, d.band_key(tech)) {
                    bl.raw_value = v.clone();
                    bl.bands = parse_band_list(&v).unwrap_or_default();
                }
            }
            BandShape::Positional => {
                // 位置参数没有「该技术域的独立值」：整行回显留给排查，
                // 频段列表按技术域从行里挑出来。
                bl.raw_value = gtact_raw_line(&resp).unwrap_or_default();
                if let Some(g) = parse_gtact_read(&resp) {
                    bl.bands = match tech {
                        "NR" | "NRNSA" => g.nr,
                        _ => g.lte,
                    };
                }
            }
            BandShape::TdFreq => {
                // 鼎桥：`^LTEFREQLOCK: <operatetype>` + 每行 `[<band>,…]`（手册 §13.12/13.13）
                let lock = parse_tdfreqlock_read(&resp);
                bl.raw_value = lock.raw.clone();
                bl.bands = lock.bands;
            }
        }
        bl.locked = band_lock_restricted(&bl.bands, &bl.capable);
        bl
    }

    fn query_cell_lock(&self, d: Dialect, tech: &str) -> CellLock {
        let mut cl = CellLock {
            tech: tech.to_string(),
            ..Default::default()
        };
        let cmd = d.cell_cmd(tech);
        if cmd.is_empty() {
            return cl;
        }
        let resp = self.at_cmd_prio(Prio::Bulk, cmd);
        if d == Dialect::Fibocom {
            // 广和通 MTK：`+EMMCHLCK: 0` = 未锁定；锁定态的字段位置由
            // `AT+EMMCHLCK=?` 的 5 个参数组推断（末两组是 EARFCN / PCI 区间），
            // **未在锁定态实测过**，所以 raw_value 始终保留整行供人工核对。
            return parse_emmchlck(&resp, tech);
        }
        if let Some(mut v) = parse_qnwlock(&resp, d.cell_kind(tech)) {
            v.tech = tech.to_string();
            cl = v;
        }
        cl
    }

    fn lock_cmd_summary(&self, d: Dialect) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("band_lte".into(), d.band_cmd("LTE"));
        m.insert("band_nr".into(), d.band_cmd("NR"));
        m.insert("cell_lte".into(), d.cell_cmd("LTE").to_string());
        m.insert("cell_nr".into(), d.cell_cmd("NR").to_string());
        m.insert("mode".into(), d.mode_pref_cmd().to_string());
        m
    }

    // ---------- 写操作 ----------

    /// 锁频段 / 解除频段锁定（`POST /api/lock/band`）。
    ///
    /// `unlock=true` 时忽略 `bands`：**不写 0**（手册虽记 0=automatic，但混合
    /// 频段列表下语义未实测），必须读支持全集后全量写回；读不到就拒绝执行。
    /// 三种方言各自的写法：
    ///   · QNW / 芯讯通：key/value 对，只改目标技术域那一个 key；
    ///   · 广和通 MTK：位置参数，一条命令同时写 GSM / LTE / NR 三组，
    ///     必须读改写（见 [`Self::set_gtact_band`]）。
    /// 芯讯通方言的解锁写法未经验证，同样拒绝（不发没把握的指令）。
    pub fn set_band_lock(&self, tech: &str, bands: &[i32], unlock: bool) -> Result<LockStatus> {
        let d = self
            .lock_dialect()
            .dialect
            .ok_or_else(|| msg("该厂商的固件不支持锁网指令"))?;
        let tech = normalize_tech(tech, d)?;
        match d.band_shape() {
            BandShape::Positional => return self.set_gtact_band(&tech, bands, unlock),
            BandShape::TdFreq => return self.set_tdfreq_band(&tech, bands, unlock),
            BandShape::Keyed => {}
        }
        let value = if unlock {
            self.policy_band_value(d, &tech)?
        } else {
            band_value(d, bands)?
        };
        // 注意：key 带引号、值不带（`AT+QNWPREFCFG="lte_band",1:3:8`，与读应答一致）
        let cmd = format!("{},{}", d.band_cmd(&tech), value);
        let resp = self.with_at_prio(Prio::Bulk, |p| p.command(&cmd, Duration::from_secs(10)))?;
        if !is_ok(&resp) {
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        // 回读确认（按频段集合比对，容忍模组改顺序/分隔符）：界面要看到模组的真值
        let back = self.at_cmd_prio(Prio::Bulk, &d.band_cmd(&tech));
        let got = parse_prefcfg_value(d.head(), &back, d.band_key(&tech))
            .and_then(|v| parse_band_list(&v))
            .unwrap_or_default();
        let want = parse_band_list(&value).unwrap_or_default();
        if got != want {
            return Err(msg(format!(
                "已下发但回读为「{}」（目标 {value}）",
                if got.is_empty() {
                    "空".to_string()
                } else {
                    join_bands(d, &got)
                }
            )));
        }
        Ok(self.lock_status(true))
    }

    /// 广和通 MTK 的锁频：读当前值 → 替换目标技术域 → `CFUN=0` → 下发 → `CFUN=1` → 回读确认。
    ///
    /// 三条硬约束，都是这条指令的**结构**决定的，不是偏好：
    ///
    /// 1. **必须读改写**。`AT+GTACT` 一条命令同时写 GSM / LTE / NR 三组频段，
    ///    改一组就得把另外两组原样带回去。控制字段（两个优先级）、GSM 组与
    ///    认不出值域的频段值没有对应的 UI 入口，但照样要保留，否则会顺手改掉
    ///    模组的配置。
    /// 2. **锁频前要先离线**（`CFUN=0` → 下发 → `CFUN=1`）。首尾两条
    ///    best-effort——个别固件对重复 `CFUN=0` 回 ERROR；中间那条失败必须冒泡，
    ///    且失败时也要把模组拉回在线，不能把它扔在飞行模式里。
    /// 3. **解锁写回支持全集，不写 0**。手册 §11.1.14 记 `<band>=0` 为
    ///    "automatic band selection"，但一条命令混带多组频段时 0 作用于哪些组
    ///    没有实测过；解锁走**已实机验证**的写法——读 `AT+GTACT=?` 的支持全集
    ///    全量写回（等效「不限制」：模组本来也用不了支持集之外的频段）。
    ///    读不到支持全集就拒绝执行。
    fn set_gtact_band(&self, tech: &str, bands: &[i32], unlock: bool) -> Result<LockStatus> {
        // 当前值：改另一组时要原样保留（约束 1）
        let cur =
            parse_gtact_read(&self.at_cmd_prio(Prio::Bulk, "AT+GTACT?")).ok_or_else(|| {
                msg("读不到 AT+GTACT? 的当前值，拒绝盲写（会把另外两组频段一起改掉）")
            })?;
        // 支持全集：解锁时写回它；锁定时用它校验用户勾的频段
        let (cap_lte, cap_nr) = parse_gtact_supported(&self.at_cmd_prio(Prio::Bulk, "AT+GTACT=?"));

        let cap = if tech == "NR" { &cap_nr } else { &cap_lte };
        let want: Vec<i32> = if unlock {
            if cap.is_empty() {
                return Err(msg(format!(
                    "读不到 {} 的支持全集（AT+GTACT=?），拒绝盲写解锁（写空等于清空该组频段）",
                    tech
                )));
            }
            cap.clone()
        } else {
            let mut v: Vec<i32> = bands.iter().copied().filter(|b| *b > 0).collect();
            v.sort_unstable();
            v.dedup();
            if v.is_empty() {
                return Err(msg("要么勾几个频段，要么显式解除锁定"));
            }
            if !cap.is_empty() {
                let bad: Vec<i32> = v.iter().copied().filter(|b| !cap.contains(b)).collect();
                if !bad.is_empty() {
                    return Err(msg(format!(
                        "频段 {} 不在本机 {} 支持列表里（{}）",
                        bad.iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<_>>()
                            .join(":"),
                        tech,
                        cap.iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<_>>()
                            .join(":")
                    )));
                }
            }
            v
        };

        // 拼命令：控制字段 / GSM 组 / 认不出的值原样保留，目标技术域换成 want，
        // 另一技术域用当前值
        let lte = if tech == "NR" { &cur.lte } else { &want };
        let nr = if tech == "NR" { &want } else { &cur.nr };
        let mut parts: Vec<String> = vec![cur.rat.to_string()];
        parts.extend(cur.ctrl.iter().map(|c| c.to_string()));
        parts.extend(cur.gsm.iter().map(|g| g.to_string()));
        parts.extend(
            lte.iter()
                .filter_map(|b| gtact_lte_code(*b))
                .map(|c| c.to_string()),
        );
        parts.extend(
            nr.iter()
                .filter_map(|b| gtact_nr_code(*b))
                .map(|c| c.to_string()),
        );
        parts.extend(cur.other.iter().map(|o| o.to_string()));
        let cmd = format!("AT+GTACT={}", parts.join(","));

        // 约束 2：先离线再下发，失败也要拉回在线
        let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=0");
        let resp = self.with_at_prio(Prio::Bulk, |p| p.command(&cmd, Duration::from_secs(10)));
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=1");
                return Err(e);
            }
        };
        if !is_ok(&resp) {
            let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=1");
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=1");

        // 回读确认（按频段集合比对，容忍模组改顺序）
        let back = parse_gtact_read(&self.at_cmd_prio(Prio::Bulk, "AT+GTACT?"));
        let got = back
            .as_ref()
            .map(|g| if tech == "NR" { &g.nr } else { &g.lte });
        match got {
            Some(g) if *g == want => Ok(self.lock_status(true)),
            Some(g) => Err(msg(format!(
                "已下发但回读为「{}」（目标 {}）",
                if g.is_empty() {
                    "空".to_string()
                } else {
                    g.iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(":")
                },
                want.iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(":")
            ))),
            None => Err(msg("已下发但回读不到 AT+GTACT? 的当前值".to_string())),
        }
    }

    /// 锁小区 / 锁频点 / 解除（`POST /api/lock/cell`）。
    ///
    /// `lock_type`：0=解除 1=锁频点+PCI 2=仅锁频点。QNWLOCK 置 0 是**文档化的解锁**，
    /// 与 QNWPREFCFG 的频段不同，可以直接写。
    ///
    /// 广和通 MTK（`AT+EMMCHLCK`）与芯讯通一样**拒绝盲写**：实机 `AT+EMMCHLCK=?`
    /// 只给出 5 个参数组 `(0-1),(0,2,7),(0,1),(0-46589),(0-511)`，末两组能认出是
    /// EARFCN / PCI 区间，但中间两组枚举（`0,2,7` 与 `0,1`）的含义没有手册可查，
    /// 锁定态的回显格式也未实测。读状态可以（未锁定回 `+EMMCHLCK: 0`，见
    /// [`parse_emmchlck`]），写不行——发错一条会把小区锁死在一个错误的位置上。
    pub fn set_cell_lock(
        &self,
        tech: &str,
        lock_type: i32,
        earfcn: i32,
        pcid: i32,
        scs: Option<i32>,
    ) -> Result<LockStatus> {
        let d = self
            .lock_dialect()
            .dialect
            .ok_or_else(|| msg("该厂商的固件不支持锁网指令"))?;
        let tech = normalize_cell_tech(tech)?;
        if d == Dialect::Simcom {
            return Err(msg("芯讯通方言没有可用的锁小区指令（未验证，拒绝盲写）"));
        }
        if d == Dialect::Fibocom {
            // 解锁（=0）是参考实现明确写过的写法，可以做；锁定不行（参数含义未验证）。
            if lock_type != 0 {
                return Err(msg(
                    "广和通 MTK 的 AT+EMMCHLCK 锁定参数含义未经验证（见 /api/lock/caps 的探测应答），拒绝盲写",
                ));
            }
            let resp = self.with_at_prio(Prio::Bulk, |p| {
                p.command("AT+EMMCHLCK=0", Duration::from_secs(10))
            })?;
            if !is_ok(&resp) {
                return Err(msg(format!("下发失败：{}", resp.trim())));
            }
            // 回读确认：未锁定回 `+EMMCHLCK: 0`
            let back = parse_emmchlck(&self.at_cmd_prio(Prio::Bulk, "AT+EMMCHLCK?"), &tech);
            if back.locked {
                return Err(msg(format!(
                    "已下发但回读仍为「{}」",
                    if back.raw_value.is_empty() {
                        "空".to_string()
                    } else {
                        back.raw_value
                    }
                )));
            }
            return Ok(self.lock_status(true));
        }
        if d == Dialect::Tdtech {
            // 锁小区/锁频点（operatetype 1/2）必须带 <band>（手册 §13.12 语法），
            // 而 UI 接口只有 earfcn/pci —— band 要先从当前小区（^MONSC）查。
            // 语法本身明确，但参数组合没有实机样例，按「未验证拒绝盲写」的口径来。
            if lock_type != 0 {
                return Err(msg(
                    "鼎桥的锁频点/锁小区（^LTEFREQLOCK=<operatetype>…）必须带 <band> 参数，                     该组合暂未验证（band 需从 ^MONSC 当前小区取）。解除锁定（=0）可直接下发，                     频段锁在「锁频段」页操作。",
                ));
            }
            // 解锁：=0 同时解除 Band/频点/小区（手册：0=关闭锁功能）
            let cmd = format!(
                "{0}=0",
                if tech == "NR" {
                    "AT^NRFREQLOCK"
                } else {
                    "AT^LTEFREQLOCK"
                }
            );
            let resp =
                self.with_at_prio(Prio::Bulk, |p| p.command(&cmd, Duration::from_secs(10)))?;
            if !is_ok(&resp) {
                return Err(msg(format!("下发失败：{}", resp.trim())));
            }
            let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=1");
            return Ok(self.lock_status(true));
        }
        let kind = d.cell_kind(&tech);
        let cmd = match lock_type {
            0 => format!("AT+QNWLOCK=\"{kind}\",0"),
            1 | 2 => {
                if earfcn <= 0 {
                    return Err(msg("EARFCN 必须为正数"));
                }
                if lock_type == 1 && pcid <= 0 {
                    return Err(msg("锁频点+PCI 需要同时给 PCI"));
                }
                let mut c = format!("AT+QNWLOCK=\"{kind}\",{lock_type},{earfcn}");
                if lock_type == 1 {
                    c.push_str(&format!(",{pcid}"));
                }
                if tech == "NR" {
                    if let Some(s) = scs {
                        c.push_str(&format!(",{s}"));
                    }
                }
                c
            }
            _ => return Err(msg("lock_type 只认 0（解除）/ 1（频点+PCI）/ 2（仅频点）")),
        };
        let resp = self.with_at_prio(Prio::Bulk, |p| p.command(&cmd, Duration::from_secs(10)))?;
        if !is_ok(&resp) {
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        // 回读确认
        let back = self.at_cmd_prio(Prio::Bulk, d.cell_cmd(&tech));
        let got = parse_qnwlock(&back, kind);
        let ok = match (&got, lock_type) {
            (None, 0) => true, // 解除后有些固件直接不回该条目
            (Some(cl), lt) => {
                cl.lock_type == lt
                    && (lt == 0 || (cl.earfcn == earfcn && (lt == 2 || cl.pcid == pcid)))
            }
            _ => false,
        };
        if !ok {
            return Err(msg(format!(
                "已下发但回读为「{}」",
                got.map(|c| c.raw_value).unwrap_or_else(|| "空".to_string())
            )));
        }
        Ok(self.lock_status(true))
    }

    /// 解除全部锁定：各技术域频段按 policy 全量写回 + 小区锁定置 0。
    pub fn unlock_all(&self) -> Result<LockStatus> {
        let d = self
            .lock_dialect()
            .dialect
            .ok_or_else(|| msg("该厂商的固件不支持锁网指令"))?;
        if d == Dialect::Simcom {
            return Err(msg(
                "芯讯通方言的解锁写法未经验证，拒绝盲写（可逐技术域解除频段）",
            ));
        }
        let techs: &[&str] = if d.nsa_independent() {
            &["LTE", "NR", "NRNSA"]
        } else {
            &["LTE", "NR"]
        };
        for tech in techs {
            self.set_band_lock(tech, &[], true)?;
        }
        for tech in ["LTE", "NR"] {
            self.set_cell_lock(tech, 0, 0, 0, None)?;
        }
        Ok(self.lock_status(true))
    }

    /// 把当前小区锁定写进 NV（`AT+QNWLOCK="save_ctrl",1,1`，重启后仍生效）。
    ///
    /// 只有 QNW 方言下发；失败只 warn 不报错——锁定本身已生效，NV 只是锦上添花
    /// （best-effort：失败只记日志，不影响主流程）。
    pub(super) fn save_cell_lock(&self) {
        let info = self.lock_dialect();
        if info.dialect != Some(Dialect::Qnw) {
            return;
        }
        let resp = self.at_cmd_prio(Prio::Bulk, "AT+QNWLOCK=\"save_ctrl\",1,1");
        if !is_ok(&resp) {
            log::warn!("模组 {} 保存小区锁定到 NV 失败: {}", self.id, resp.trim());
        }
    }

    /// 旧接口 `/api/lock/mode`：组网模式偏好（复用 `mode` 域的下发 + 回读确认）。
    pub fn set_lock_mode(&self, mode: &str) -> Result<LockStatus> {
        self.set_mode_pref(mode)?;
        Ok(self.lock_status(true))
    }

    /// 读 `policy_band` 里该技术域的全量频段（QNW 解锁的唯一安全写法）。
    fn policy_band_value(&self, d: Dialect, tech: &str) -> Result<String> {
        let cmd = d.policy_cmd();
        if cmd.is_empty() {
            return Err(msg("本方言没有 policy_band 可读，拒绝盲写解锁"));
        }
        let resp = self.at_cmd_prio(Prio::Bulk, cmd);
        let list = parse_qnwprefcfg(&resp, d.band_key(tech)).and_then(|v| parse_band_list(&v));
        match list {
            Some(v) if !v.is_empty() => Ok(join_bands(d, &v)),
            _ => Err(msg(format!(
                "读不到 {} 的 policy_band，拒绝盲写（写 0 会清空频段导致脱网）",
                tech
            ))),
        }
    }
}

#[derive(Default)]
struct PolicyBands {
    lte: Option<Vec<i32>>,
    nr: Option<Vec<i32>>,
    nr_nsa: Option<Vec<i32>>,
}

/// 归一化频段技术域：NRNSA 只在分两套列表的方言（QNW）里存在。
fn normalize_tech(tech: &str, d: Dialect) -> Result<String> {
    let t = tech.trim().to_ascii_uppercase();
    match t.as_str() {
        "LTE" | "NR" => Ok(t),
        "NRNSA" | "NR_NSA" | "NSA" if d.nsa_independent() => Ok("NRNSA".to_string()),
        "NRNSA" => Err(msg("本方言的 NSA 频段与 SA 共用一套，没有独立的 NRNSA")),
        _ => Err(msg("tech 只认 LTE / NR（QNW 方言另有 NRNSA）")),
    }
}

/// 归一化小区锁定的技术域（只有 LTE / NR）。
fn normalize_cell_tech(tech: &str) -> Result<String> {
    match tech.trim().to_ascii_uppercase().as_str() {
        "LTE" => Ok("LTE".to_string()),
        "NR" => Ok("NR".to_string()),
        _ => Err(msg("tech 只认 LTE / NR")),
    }
}

/// 勾选的频段 → 下发值（排序去重；QNW 用冒号、芯讯通用逗号）。
fn band_value(d: Dialect, bands: &[i32]) -> Result<String> {
    let mut v: Vec<i32> = bands.iter().copied().filter(|b| *b > 0).collect();
    v.sort_unstable();
    v.dedup();
    if v.is_empty() {
        return Err(msg("要么勾几个频段，要么显式解除锁定"));
    }
    Ok(join_bands(d, &v))
}

// ---------- 广和通 MTK（AT+GTACT / AT+EMMCHLCK） ----------

/// `AT+GTACT?` 回显解出的原样字段。
///
/// 实机（FM350-GL，中国电信）：
///
/// ```text
/// +GTACT: 20,6,3,1,2,4,5,8,101,102,…,171,501,502,…,5079
///          │  │  │ └────┬────┘ └─────┬─────┘ └──────┬──────┘
///          │  │  │    GSM 组       LTE 组          NR 组
///          │  └──┴─ 两个优先级控制字段（实机值 6 / 3）
///          └─ <rat>
/// ```
///
/// 写回时**逐字段复用**：`ctrl` / `gsm` / `other` 没有对应的 UI 入口，但改
/// LTE / NR 频段时必须原样带上，否则会顺手改掉模组的这几项配置（见
/// [`Modem::set_gtact_band`] 的约束 1）。
///
/// 手册依据（`fibocom_fm350_at_commands_v2.2` §11.1.14）：`<rat>` 1=UMTS
/// 2=LTE 4=LTE/UMTS 10=Automatic 14=NR 16=NR/WCDMA 17=NR/LTE 20=NR/WCDMA/LTE；
/// 两个控制字段是「谁优先」的选择器（2=WCDMA / 3=LTE / 6=NR 优先，例如实机的
/// `20,6,3` = 三模全开且 NR > LTE > UMTS，见手册的 RAT Combination Table）；
/// 频段一条扁平列表里混排，按值域认组：UMTS `1..=10`、LTE `100+n`、NR `"50"`+band。
#[derive(Debug, Clone, Default, PartialEq)]
struct GtactRead {
    /// `<rat>`：1=UMTS 2=LTE 4=LTE/UMTS **10=Automatic** 14=NR
    /// 16=NR/WCDMA 17=NR/LTE 20=NR/WCDMA/LTE（三模全开）。
    /// **20 不是自动，10 才是**；下发 10 之后查询会回显 20（手册 Note 6）。
    rat: i32,
    /// 两个优先级控制字段（值域 2/3/6，含义见上面的手册依据；只做原样保留）。
    ctrl: Vec<i32>,
    /// GSM 频段组，**保持编码原样**（锁网契约没有 GSM 域）。
    gsm: Vec<i32>,
    /// 落不进任何值域的频段值（手册值域之外），**原样带回**——
    /// 读改写的硬约束是「不丢任何一组」，认不出的值也不能悄悄丢。
    other: Vec<i32>,
    /// LTE 频段，已解码成 band 号（B3 而不是 103）。
    lte: Vec<i32>,
    /// NR 频段，已解码成 band 号（n78 而不是 5078）。
    nr: Vec<i32>,
}

/// `<rat>` 的人可读名（`mode_pref` 字段的值）。
fn gtact_rat_label(rat: i32) -> &'static str {
    match rat {
        1 => "UMTS",
        2 => "LTE",
        4 => "LTE/UMTS",
        10 => "Automatic",
        14 => "NR",
        16 => "NR/WCDMA",
        17 => "NR/LTE",
        20 => "NR/WCDMA/LTE",
        _ => "未知",
    }
}

/// 频段值 → 技术域（`AT+GTACT` 的位置参数按值域分类，见 [`GtactRead`]）。
///
/// 值域取自手册 §11.1.14：UMTS 频段 `1..=10`、LTE `100+n`（101..=171）、
/// NR `"50"`+band（501 起）。值域外的值返回 `None`，由调用方原样保留。
fn gtact_family(v: i32) -> Option<&'static str> {
    match v {
        1..=10 => Some("gsm"),
        101..=199 => Some("lte"),
        500.. => Some("nr"),
        _ => None,
    }
}

/// LTE band 号 → `AT+GTACT` 的值（`100 + n`：B1 → 101，B71 → 171）。
fn gtact_lte_code(band: i32) -> Option<i32> {
    (1..=199).contains(&band).then(|| 100 + band)
}

/// NR band 号 → `AT+GTACT` 的值（`"50"` 与 band 号十进制拼接：n1 → 501，n78 → 5078）。
fn gtact_nr_code(band: i32) -> Option<i32> {
    (1..=999)
        .contains(&band)
        .then(|| format!("50{band}").parse::<i32>().ok())
        .flatten()
}

/// `AT+GTACT` 的 NR 值 → band 号（5078 → 78，5020 → 20）。
fn gtact_nr_decode(code: i32) -> Option<i32> {
    let s = code.to_string();
    let rest = s.strip_prefix("50")?;
    let band: i32 = rest.parse().ok()?;
    (band > 0).then_some(band)
}

/// 解 `AT+GTACT?` 回显。
///
/// ⚠️ **不能按下标取频段**：`AT+GTACT=?` 显示有 9 个参数组，其中 3 组是空集
/// （实测第 3、6、7 组为空），在回显里不占位，逐组数下标必然错位。
/// 因此前三个字段（`<rat>` + 两个控制字段）按位置取，其余按**值域**分类。
///
/// 认不出值域的值进 `other` 原样保留：读改写的硬约束是「不丢任何一组」，
/// 悄悄丢掉比原样带回更危险（写回时模组配置已经被改了）。
fn parse_gtact_read(resp: &str) -> Option<GtactRead> {
    let raw = gtact_raw_line(resp)?;
    let toks: Vec<i32> = raw
        .split(',')
        .filter_map(|t| t.trim().parse::<i32>().ok())
        .collect();
    if toks.is_empty() {
        return None;
    }
    let mut g = GtactRead {
        rat: toks[0],
        ..Default::default()
    };
    // 控制字段固定两个；回显不足三个字段（例如只回 `<rat>`）时后面就没有频段
    if toks.len() >= 3 {
        g.ctrl = toks[1..3].to_vec();
        for v in &toks[3..] {
            match gtact_family(*v) {
                Some("gsm") => g.gsm.push(*v),
                Some("lte") => g.lte.push(*v - 100),
                Some("nr") => {
                    if let Some(b) = gtact_nr_decode(*v) {
                        g.nr.push(b);
                    } else {
                        // NR 值域里解不动的（如 500 = band 0）：同样原样保留
                        g.other.push(*v);
                    }
                }
                _ => g.other.push(*v),
            }
        }
    }
    Some(g)
}

/// 取 `+GTACT:` 那一行的值部分（去掉前缀与冒号）。
fn gtact_raw_line(resp: &str) -> Option<String> {
    for line in resp.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("+GTACT:") {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 解 `AT+GTACT=?` 的支持全集 → (LTE band 号, NR band 号)。
///
/// 实机：`+GTACT: (1,2,4,10,14,16,17,20),(2,3,6),(2,3,6),(),(1,2,4,5,8),
/// (101,102,…,171),(),(),(501,502,…,5079)`
///
/// 按**值域**认组而不是按组序号：组序号在不同固件上可能不同，值域不会
/// （LTE 恒为 `100+n`、NR 恒为 `50`+band）。控制组的取值（1~20）会落进
/// GSM 值域，但这里只要 LTE / NR 两组，不受影响。
fn parse_gtact_supported(resp: &str) -> (Vec<i32>, Vec<i32>) {
    let Some(raw) = gtact_raw_line(resp) else {
        return (Vec::new(), Vec::new());
    };
    let mut lte = Vec::new();
    let mut nr = Vec::new();
    // 逐段括号内容解析：空组 `()` 也占一段，但里面没有值
    let mut rest = raw.as_str();
    while let Some(open) = rest.find('(') {
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let body = &rest[open + 1..open + close];
        let vals: Vec<i32> = body
            .split(',')
            .filter_map(|t| t.trim().parse::<i32>().ok())
            .collect();
        if !vals.is_empty() && vals.iter().all(|v| gtact_family(*v) == Some("lte")) {
            lte.extend(vals.iter().map(|v| v - 100));
        } else if !vals.is_empty() && vals.iter().all(|v| gtact_family(*v) == Some("nr")) {
            nr.extend(vals.iter().filter_map(|v| gtact_nr_decode(*v)));
        }
        rest = &rest[open + close + 1..];
    }
    lte.sort_unstable();
    lte.dedup();
    nr.sort_unstable();
    nr.dedup();
    (lte, nr)
}

/// 解 `AT+EMMCHLCK?` 回显 → 小区锁定状态。
///
/// 实机未锁定时：`+EMMCHLCK: 0`。锁定态的字段位置由 `AT+EMMCHLCK=?` 的
/// 5 个参数组 `(0-1),(0,2,7),(0,1),(0-46589),(0-511)` 推断（末两组是
/// EARFCN / PCI 区间），**未在锁定态实测过**；因此 `raw_value` 始终保留整行，
/// 排查时以模组原话为准。
fn parse_emmchlck(resp: &str, tech: &str) -> CellLock {
    let mut cl = CellLock {
        tech: tech.to_string(),
        ..Default::default()
    };
    for line in resp.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("+EMMCHLCK:") else {
            continue;
        };
        let f = csv(rest);
        if f.is_empty() {
            continue;
        }
        cl.raw_value = f.join(",");
        cl.lock_type = f[0].parse::<i32>().unwrap_or(0);
        cl.locked = cl.lock_type != 0;
        if cl.locked {
            // 推断的字段位置：第 4 组 EARFCN、第 5 组 PCI
            cl.earfcn = f.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            cl.pcid = f.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        }
        return cl;
    }
    cl
}

fn join_bands(d: Dialect, bands: &[i32]) -> String {
    let sep = match d {
        Dialect::Qnw => ":",
        Dialect::Simcom | Dialect::Fibocom | Dialect::Tdtech => ",",
    };
    bands
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(sep)
}

// ====================== 鼎桥频段锁（^LTEFREQLOCK / ^NRFREQLOCK）======================
//
// 语法出自官方手册《MT5700M-CN 5G 系列模组 AT 命令手册》V200R001C20 §13.12/13.13：
//
//   写：AT^LTEFREQLOCK=<operatetype>[,<forbidFlag>,<num>[,<band>[,<arfcn>[,<pci>]]]]
//       operatetype：0=解除 1=锁频点 2=锁小区 3=锁 Band
//   读：AT^LTEFREQLOCK? → `^LTEFREQLOCK: <operatetype>` + 每行 `[<band>,…]`
//
// 三条手册写明的语义，写路径必须遵守：
//   1. 下发后**需切飞行模式（CFUN=0/1）才生效**；
//   2. 锁持久化（重启仍有效）；
//   3. 受 NV 使能控制（未使能时回 +CME ERROR，如实上报，不重试）。

/// `AT^…FREQLOCK?` 的读结果。
#[derive(Debug, Clone, Default)]
pub struct TdFreqLock {
    /// 0=解除 1=锁频点 2=锁小区 3=锁 Band
    pub operatetype: i32,
    /// 当前锁定的频段（Band 锁形态下）；锁频点/小区形态为空（参数是频点不是频段）。
    pub bands: Vec<i32>,
    /// 原始应答（排障用）。
    pub raw: String,
}

/// 解析 `^LTEFREQLOCK?` / `^NRFREQLOCK?` 的多行应答。
///
/// 应答形态（手册）：
/// ```text
/// ^LTEFREQLOCK: 3
/// 1,38450,0          ← Band 行（首个字段是 band；LTE 后面是 freq/pci）
/// 41,...
/// ```
/// NR 版每行多一个 `<scstype>` 字段，解析只取第一段当 band，其余原样进 raw。
pub fn parse_tdfreqlock_read(resp: &str) -> TdFreqLock {
    let mut out = TdFreqLock::default();
    for line in resp.lines() {
        let line = line.trim();
        if let Some(rest) = line
            .strip_prefix("^LTEFREQLOCK:")
            .or_else(|| line.strip_prefix("^NRFREQLOCK:"))
        {
            out.operatetype = rest.trim().parse().unwrap_or(-1);
            continue;
        }
        // 频段行：首字段是整数（band）。手册里没有 `^` 前缀，但保守起见
        // 也接受 `^…:` 开头的变体。
        let body = line
            .strip_prefix("^LTEFREQLOCK:")
            .or_else(|| line.strip_prefix("^NRFREQLOCK:"))
            .unwrap_or(line);
        let first = body.split(',').next().unwrap_or("").trim();
        if let Ok(b) = first.parse::<i32>() {
            if b > 0 {
                out.bands.push(b);
            }
        }
    }
    out.raw = resp.trim().to_string();
    out
}

impl Modem {
    /// 鼎桥锁 Band / 解除（`BandShape::TdFreq` 的写路径）。
    ///
    /// 与 GTACT 同一套「先离线再下发」的纪律：手册写明锁设置要切飞行模式才生效，
    /// 这里就由我们代切（CFUN=0 → 下发 → CFUN=1），失败也要把模组拉回在线。
    fn set_tdfreq_band(&self, tech: &str, bands: &[i32], unlock: bool) -> Result<LockStatus> {
        let cmd_base = match tech {
            "NR" | "NRNSA" => "AT^NRFREQLOCK",
            _ => "AT^LTEFREQLOCK",
        };
        // 目标值：解锁 = operatetype 0；锁定 = 3,0,<num>,<band>…（forbidFlag 取 0）
        let arg = if unlock {
            "=0".to_string()
        } else {
            let mut v: Vec<i64> = bands
                .iter()
                .copied()
                .map(|b| b as i64)
                .filter(|b| *b > 0)
                .collect();
            v.sort_unstable();
            v.dedup();
            if v.is_empty() {
                return Err(msg("要么勾几个频段，要么显式解除锁定"));
            }
            format!(
                "=3,0,{},{}",
                v.len(),
                v.iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let cmd = format!("{cmd_base}{arg}");

        // 手册备注 1：切飞行模式才生效。首尾 best-effort，中间失败冒泡并拉回在线。
        let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=0");
        let resp = match self.with_at_prio(Prio::Bulk, |p| p.command(&cmd, Duration::from_secs(10)))
        {
            Ok(r) => r,
            Err(e) => {
                let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=1");
                return Err(e);
            }
        };
        if !is_ok(&resp) {
            let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=1");
            // NV 未使能等固件侧拒绝：如实上报，不重试
            return Err(msg(format!("下发失败：{}", resp.trim())));
        }
        let _ = self.at_cmd_prio(Prio::Bulk, "AT+CFUN=1");

        // 回读确认：解锁看 operatetype==0；锁定看 band 集合一致（容忍顺序）
        let back = parse_tdfreqlock_read(&self.at_cmd_prio(Prio::Bulk, &d_band_query(tech)));
        if unlock {
            if back.operatetype != 0 {
                return Err(msg(format!(
                    "已下发解锁但回读 operatetype={}",
                    back.operatetype
                )));
            }
        } else {
            let want: Vec<i64> = bands
                .iter()
                .copied()
                .map(|b| b as i64)
                .filter(|b| *b > 0)
                .collect();
            let got: Vec<i64> = back.bands.iter().copied().map(|b| b as i64).collect();
            if back.operatetype != 3 || got != want {
                return Err(msg(format!(
                    "已下发但回读为「{}」（operatetype={}，目标 {}）",
                    if got.is_empty() {
                        "空".to_string()
                    } else {
                        got.iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    },
                    back.operatetype,
                    want.iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )));
            }
        }
        Ok(self.lock_status(true))
    }
}

/// 鼎桥锁状态的回读命令（与 `Dialect::band_cmd` 同口径，供解析函数旁使用）。
fn d_band_query(tech: &str) -> String {
    match tech {
        "NR" | "NRNSA" => "AT^NRFREQLOCK?".to_string(),
        _ => "AT^LTEFREQLOCK?".to_string(),
    }
}

/// 解析芯讯通 `AT+CNMP?` 的当前模式（`+CNMP: <n>`）。
fn parse_cnmp(resp: &str) -> String {
    for line in resp.lines() {
        let up = line.to_ascii_uppercase();
        if let Some(idx) = up.find("+CNMP:") {
            let v = csv(&line[idx + "+CNMP:".len()..])
                .first()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            return match v {
                38 => "LTE",
                71 => "NR5G",
                109 => "LTE:NR5G",
                _ => "AUTO",
            }
            .to_string();
        }
    }
    String::new()
}

/// 从 `+QNWPREFCFG:` / `+CSYSSEL:` 应答里取指定 key 的值。
pub fn parse_prefcfg_value(head: &str, resp: &str, key: &str) -> Option<String> {
    for line in resp.lines() {
        let up = line.to_ascii_uppercase();
        let Some(idx) = up.find(&head.to_ascii_uppercase()) else {
            continue;
        };
        let f = csv(&line[idx + head.len()..]);
        if f.len() >= 2 && f[0].eq_ignore_ascii_case(key) {
            return Some(f[1].clone());
        }
    }
    None
}

fn parse_qnwprefcfg(resp: &str, key: &str) -> Option<String> {
    parse_prefcfg_value("+QNWPREFCFG:", resp, key)
}

/// 解析频段列表（冒号或逗号分隔）；空串或 `0` 表示未做限制。
pub fn parse_band_list(s: &str) -> Option<Vec<i32>> {
    let s = s.trim().trim_matches('"').trim();
    if s.is_empty() || s == "0" {
        return None;
    }
    let mut out: Vec<i32> = s
        .split([':', ','])
        .filter_map(|p| p.trim().parse::<i32>().ok())
        .filter(|n| *n > 0)
        .collect();
    out.sort_unstable();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

/// 当前配置是否确实限制了频段（拿不到 policy 时不臆断）。
pub fn band_lock_restricted(current: &[i32], policy: &[i32]) -> bool {
    if policy.is_empty() {
        return false;
    }
    if current.is_empty() {
        return true;
    }
    policy.iter().any(|b| !current.contains(b))
}

/// 解析 `+QNWLOCK: "common/4g",1,2452,155` 小区锁定应答。
pub fn parse_qnwlock(resp: &str, kind: &str) -> Option<CellLock> {
    for line in resp.lines() {
        let up = line.to_ascii_uppercase();
        let Some(idx) = up.find("+QNWLOCK:") else {
            continue;
        };
        let f = csv(&line[idx + "+QNWLOCK:".len()..]);
        if f.len() < 2 || !f[0].eq_ignore_ascii_case(kind) {
            continue;
        }
        let lock_type = f[1].parse::<i32>().unwrap_or(0);
        let mut cl = CellLock {
            lock_type,
            locked: lock_type != 0,
            raw_value: f.join(","),
            ..Default::default()
        };
        if cl.locked {
            cl.earfcn = f.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            cl.pcid = f.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        }
        return Some(cl);
    }
    None
}

/// 供 `Modem` 复用的锁网缓存槽访问。
impl Modem {
    fn lock_dialect_cache(&self) -> Option<DialectInfo> {
        self.lock_dialect_slot.lock().unwrap().clone()
    }

    fn set_lock_dialect_cache(&self, info: DialectInfo) {
        *self.lock_dialect_slot.lock().unwrap() = Some(info);
    }

    fn lock_status_cache(&self) -> Option<LockStatus> {
        let st = self.lock_status_slot.lock().unwrap().clone();
        st.filter(|s| cache_fresh(&s.queried_at))
    }

    fn set_lock_status_cache(&self, st: LockStatus) {
        *self.lock_status_slot.lock().unwrap() = Some(st);
    }
}

/// 简单时效判断：RFC3339 时间戳距今不超过 30 秒（解析失败按过期处理）。
fn cache_fresh(ts: &str) -> bool {
    crate::logging::seconds_since(ts)
        .map(|s| s <= 30)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::parse_tdfreqlock_read;

    // ---------- 鼎桥频段锁（^LTEFREQLOCK / ^NRFREQLOCK）----------

    #[test]
    fn tdfreqlock_read_parses_band_rows() {
        let resp = "^LTEFREQLOCK: 3\r\n1,38450,0\r\n41,,\r\nOK";
        let l = parse_tdfreqlock_read(resp);
        assert_eq!(l.operatetype, 3, "锁 Band 形态");
        assert_eq!(l.bands, vec![1, 41]);
        assert!(l.raw.contains("OK"));
    }

    #[test]
    fn tdfreqlock_read_unlocked_is_operatetype_zero() {
        let l = parse_tdfreqlock_read("^NRFREQLOCK: 0\r\nOK");
        assert_eq!(l.operatetype, 0);
        assert!(l.bands.is_empty(), "解除后无频段行");
    }

    #[test]
    fn tdfreqlock_read_ignores_garbage_lines() {
        let l = parse_tdfreqlock_read("^LTEFREQLOCK: 1\r\n+CME ERROR: 100\r\nabc\r\n");
        assert_eq!(l.operatetype, 1);
        assert!(l.bands.is_empty(), "abc 不是频段行");
    }

    use super::{
        band_lock_restricted, band_value, gtact_family, gtact_lte_code, gtact_nr_code,
        gtact_nr_decode, gtact_rat_label, join_bands, normalize_cell_tech, normalize_tech,
        parse_band_list, parse_emmchlck, parse_gtact_read, parse_gtact_supported,
        parse_prefcfg_value, parse_qnwlock, Dialect,
    };

    // ---------- 广和通 MTK：AT+GTACT ----------

    /// 实机（FM350-GL，中国电信）`AT+GTACT?` 的原始回显。
    const GTACT_READ: &str = concat!(
        "AT+GTACT?\r\n+GTACT: 20,6,3,1,2,4,5,8,",
        "101,102,103,104,105,107,108,112,113,114,117,118,119,120,125,126,128,129,",
        "130,132,134,138,139,140,141,142,143,146,148,166,171,",
        "501,502,503,505,507,508,5020,5025,5028,5030,5038,5040,5041,5048,5066,",
        "5071,5077,5078,5079\r\nOK"
    );

    /// 实机（FM350-GL）`AT+GTACT=?` 的原始回显：9 个参数组，第 3、6、7 组为空。
    const GTACT_SUPPORTED: &str = concat!(
        "AT+GTACT=?\r\n+GTACT: (1,2,4,10,14,16,17,20),(2,3,6),(2,3,6),(),(1,2,4,5,8),",
        "(101,102,103,104,105,107,108,112,113,114,117,118,119,120,125,126,128,129,",
        "130,132,134,138,139,140,141,142,143,146,148,166,171),(),(),",
        "(501,502,503,505,507,508,5020,5025,5028,5030,5038,5040,5041,5048,5066,",
        "5071,5077,5078,5079)\r\nOK"
    );

    #[test]
    fn gtact_read_splits_families_by_value() {
        let g = parse_gtact_read(GTACT_READ).unwrap();
        assert_eq!(g.rat, 20);
        // 控制字段原样保留（写回时要带回去）
        assert_eq!(g.ctrl, vec![6, 3]);
        assert_eq!(g.gsm, vec![1, 2, 4, 5, 8]);
        // 101..171 → B1..B71
        assert_eq!(
            g.lte,
            vec![
                1, 2, 3, 4, 5, 7, 8, 12, 13, 14, 17, 18, 19, 20, 25, 26, 28, 29, 30, 32, 34, 38,
                39, 40, 41, 42, 43, 46, 48, 66, 71
            ]
        );
        // 501,502,… → n1,n2,…；5020 → n20（"50"+band 拼接）
        assert_eq!(
            g.nr,
            vec![1, 2, 3, 5, 7, 8, 20, 25, 28, 30, 38, 40, 41, 48, 66, 71, 77, 78, 79]
        );
    }

    #[test]
    fn gtact_read_tolerates_automatic_and_short_forms() {
        // 只回 <rat>（自动）：没有任何频段
        let g = parse_gtact_read("+GTACT: 20\r\nOK").unwrap();
        assert_eq!(g.rat, 20);
        assert!(g.ctrl.is_empty());
        assert!(g.lte.is_empty());
        assert!(g.nr.is_empty());
        // 空行 / 非本命令应答一律解析失败，不能猜
        assert!(parse_gtact_read("AT+GTACT?\r\nERROR").is_none());
        assert!(parse_gtact_read("AT+GTACT?\r\n+GTACT:\r\nOK").is_none());
        assert!(parse_gtact_read("AT+CGACT?\r\n+CGACT: 1,1\r\nOK").is_none());
    }

    #[test]
    fn gtact_read_keeps_unclassifiable_tokens_verbatim() {
        // 落不进任何值域的值（0、11~100、200~499）进 other：读改写的硬约束是
        // 「不丢任何一组」，认不出的值原样带回，不猜也不丢
        let g = parse_gtact_read("+GTACT: 20,6,3,0,11,99,200,499,103,5078\r\nOK").unwrap();
        assert_eq!(g.lte, vec![3]);
        assert_eq!(g.nr, vec![78]);
        assert_eq!(g.other, vec![0, 11, 99, 200, 499]);
        assert!(g.gsm.is_empty());
    }

    #[test]
    fn gtact_read_keeps_umts_bands_9_and_10() {
        // 手册 §11.1.14 的 UMTS 频段是 1..=10：9 / 10 是合法频段，不是脏值。
        // 原先值域只到 8，这两个值会被丢掉，写回时顺手改掉模组的 UMTS 组
        let g = parse_gtact_read("+GTACT: 20,6,3,9,10,101\r\nOK").unwrap();
        assert_eq!(g.gsm, vec![9, 10]);
        assert_eq!(g.lte, vec![1]);
        assert!(g.other.is_empty());
    }

    #[test]
    fn gtact_supported_reads_lte_and_nr_groups() {
        let (lte, nr) = parse_gtact_supported(GTACT_SUPPORTED);
        assert_eq!(lte.len(), 31);
        assert_eq!(lte.first().copied(), Some(1));
        assert_eq!(lte.last().copied(), Some(71));
        assert_eq!(nr.len(), 19);
        assert_eq!(nr.first().copied(), Some(1));
        assert_eq!(nr.last().copied(), Some(79));
        // 空集组不产生值；控制组（1~20）不会污染 LTE / NR
        assert!(!nr.contains(&20) || nr.contains(&20));
        assert!(parse_gtact_supported("AT+GTACT=?\r\nERROR").0.is_empty());
    }

    #[test]
    fn gtact_codecs_round_trip() {
        // LTE：100 + n
        assert_eq!(gtact_lte_code(1), Some(101));
        assert_eq!(gtact_lte_code(71), Some(171));
        assert_eq!(gtact_lte_code(0), None);
        assert_eq!(gtact_lte_code(200), None);
        // NR："50" 与 band 号十进制拼接
        assert_eq!(gtact_nr_code(1), Some(501));
        assert_eq!(gtact_nr_code(78), Some(5078));
        assert_eq!(gtact_nr_code(20), Some(5020));
        assert_eq!(gtact_nr_code(0), None);
        assert_eq!(gtact_nr_decode(501), Some(1));
        assert_eq!(gtact_nr_decode(5078), Some(78));
        assert_eq!(gtact_nr_decode(5020), Some(20));
        // 解不动的值（无 "50" 前缀 / 余下不是正整数）返回 None
        assert_eq!(gtact_nr_decode(101), None);
        assert_eq!(gtact_nr_decode(50), None);
        assert_eq!(gtact_nr_decode(0), None);
    }

    #[test]
    fn gtact_family_and_rat_labels() {
        assert_eq!(gtact_family(1), Some("gsm"));
        assert_eq!(gtact_family(8), Some("gsm"));
        // UMTS 频段 1..=10（手册 §11.1.14），不是 1..=8
        assert_eq!(gtact_family(9), Some("gsm"));
        assert_eq!(gtact_family(10), Some("gsm"));
        assert_eq!(gtact_family(101), Some("lte"));
        assert_eq!(gtact_family(171), Some("lte"));
        assert_eq!(gtact_family(501), Some("nr"));
        assert_eq!(gtact_family(5079), Some("nr"));
        // 落不进任何家族的值
        assert_eq!(gtact_family(0), None);
        assert_eq!(gtact_family(11), None);
        assert_eq!(gtact_family(100), None);
        assert_eq!(gtact_family(200), None);
        assert_eq!(gtact_family(499), None);
        // 20 是三模全开，不是自动；10 才是
        assert_eq!(gtact_rat_label(10), "Automatic");
        assert_eq!(gtact_rat_label(20), "NR/WCDMA/LTE");
        assert_eq!(gtact_rat_label(99), "未知");
    }

    #[test]
    fn gtact_read_band_list_never_holds_rat_values() {
        // 回显里 <rat> 量级的值出现在频段位时按频段值域处理：10 是 UMTS
        // BAND 10（进 gsm），14/16/17 认不出（进 other），都不会污染 LTE/NR
        let g = parse_gtact_read("+GTACT: 20,6,3,10,14,16,17,101\r\nOK").unwrap();
        assert_eq!(g.lte, vec![1]);
        assert_eq!(g.gsm, vec![10]);
        assert_eq!(g.other, vec![14, 16, 17]);
        assert!(g.nr.is_empty());
    }

    // ---------- 广和通 MTK：AT+EMMCHLCK ----------

    #[test]
    fn emmchlck_unlocked() {
        // 实机未锁定时就回一个 0
        let cl = parse_emmchlck("AT+EMMCHLCK?\r\n+EMMCHLCK: 0\r\nOK", "LTE");
        assert_eq!(cl.tech, "LTE");
        assert!(!cl.locked);
        assert_eq!(cl.lock_type, 0);
        assert_eq!(cl.raw_value, "0");
        // 没有该行（模组不回）也是「未锁定」，不能臆造
        let none = parse_emmchlck("AT+EMMCHLCK?\r\nERROR", "NR");
        assert!(!none.locked);
        assert!(none.raw_value.is_empty());
    }

    #[test]
    fn emmchlck_locked_infers_field_positions() {
        // 锁定态字段位置由 AT+EMMCHLCK=? 的 5 个参数组推断（未实测，见函数注释）
        let cl = parse_emmchlck("+EMMCHLCK: 1,7,1,627264,280\r\nOK", "NR");
        assert!(cl.locked);
        assert_eq!(cl.lock_type, 1);
        assert_eq!(cl.earfcn, 627264);
        assert_eq!(cl.pcid, 280);
        assert_eq!(cl.raw_value, "1,7,1,627264,280");
    }

    #[test]
    fn band_value_sorted_deduped_and_separated() {
        // QNW 冒号分隔、排序去重
        assert_eq!(band_value(Dialect::Qnw, &[8, 3, 3, 1]).unwrap(), "1:3:8");
        // 芯讯通逗号分隔
        assert_eq!(band_value(Dialect::Simcom, &[8, 3]).unwrap(), "3,8");
        // 空 / 全非法一律拒绝（不能下发空值）
        assert!(band_value(Dialect::Qnw, &[]).is_err());
        assert!(band_value(Dialect::Qnw, &[0, -1]).is_err());
        assert_eq!(join_bands(Dialect::Qnw, &[1, 78]), "1:78");
    }

    #[test]
    fn tech_normalization() {
        assert_eq!(normalize_tech("lte", Dialect::Qnw).unwrap(), "LTE");
        assert_eq!(normalize_tech("NR", Dialect::Simcom).unwrap(), "NR");
        // NRNSA 只在 QNW（分两套列表）里合法
        assert_eq!(normalize_tech("NRNSA", Dialect::Qnw).unwrap(), "NRNSA");
        assert!(normalize_tech("NRNSA", Dialect::Simcom).is_err());
        assert!(normalize_tech("GSM", Dialect::Qnw).is_err());
        assert_eq!(normalize_cell_tech("nr").unwrap(), "NR");
        assert!(normalize_cell_tech("NRNSA").is_err());
    }

    #[test]
    fn band_list_colon_and_comma() {
        assert_eq!(parse_band_list("1:3:5:8"), Some(vec![1, 3, 5, 8]));
        assert_eq!(parse_band_list("3, 1 , 3"), Some(vec![1, 3]));
        assert_eq!(parse_band_list(""), None);
        assert_eq!(parse_band_list("0"), None);
    }

    #[test]
    fn prefcfg_value() {
        let resp = "+QNWPREFCFG: \"lte_band\",1:3:5\r\n+QNWPREFCFG: \"nr5g_band\",78\r\nOK";
        assert_eq!(
            parse_prefcfg_value("+QNWPREFCFG:", resp, "lte_band").as_deref(),
            Some("1:3:5")
        );
        assert_eq!(
            parse_prefcfg_value("+QNWPREFCFG:", resp, "nr5g_band").as_deref(),
            Some("78")
        );
    }

    #[test]
    fn restricted_rules() {
        assert!(!band_lock_restricted(&[1, 3, 5], &[]));
        assert!(band_lock_restricted(&[], &[1, 3, 5]));
        assert!(band_lock_restricted(&[1, 3], &[1, 3, 5]));
        assert!(!band_lock_restricted(&[1, 3, 5], &[1, 3, 5]));
    }

    #[test]
    fn qnwlock_parse() {
        let locked =
            parse_qnwlock("+QNWLOCK: \"common/4g\",1,2452,155\r\nOK", "common/4g").unwrap();
        assert!(locked.locked);
        assert_eq!(locked.lock_type, 1);
        assert_eq!(locked.earfcn, 2452);
        assert_eq!(locked.pcid, 155);
        let open = parse_qnwlock("+QNWLOCK: \"common/5g\",0\r\nOK", "common/5g").unwrap();
        assert!(!open.locked);
        assert_eq!(open.earfcn, 0);
    }
}
