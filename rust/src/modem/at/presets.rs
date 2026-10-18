//! 常用 AT 指令清单（`/api/at/presets`）：3GPP 通用 + 各家私有。
//!
//! 指令清单按 3GPP 通用表 + 各家私有表合并（与各家
//! `module/<vendor>/atpreset.go`）：
//!   · 通用清单只放 3GPP 标准指令，厂商私有的都在各自的私有表里；
//!   · `presets_for` 合并时：私有表里同名（不分大小写）的项**覆盖**通用项，
//!     被抑制的通用项直接摘掉（目前只有展锐不认不带 cid 的 `AT+CGCONTRDP`）；
//!   · 厂商未识别时只给通用清单 —— 宁可少，也不要给用户看别家的私有指令。
//!
//! ⚠️ 表里**绝不出现**「未取证但一发就坏事」的指令（如展锐
//! `AT+SPCOMMLOCKBAND` 带参数执行、`AT+SPENGMD`）—— 用户点一下就会把模组弄挂。

use serde::Serialize;

use crate::modem::vendor::Vendor;

/// 一条预设指令。
#[derive(Debug, Clone, Serialize)]
pub struct AtPreset {
    /// 分组名（界面上是 `<optgroup>`）
    pub group: &'static str,
    /// 指令原文（选中后填进输入框，用户可再改）
    pub cmd: &'static str,
    /// 中文说明
    pub desc: &'static str,
    /// 是否是带占位符的模板（选中后必须先把 `<…>` 换掉）
    #[serde(skip_serializing_if = "is_false")]
    pub tpl: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

const fn of(group: &'static str, cmd: &'static str, desc: &'static str) -> AtPreset {
    AtPreset {
        group,
        cmd,
        desc,
        tpl: false,
    }
}

/// 3GPP 标准指令：任何规规矩矩的模组都该支持（TS 27.007 / TS 27.005）。
pub fn common_presets() -> Vec<AtPreset> {
    vec![
        of("查询", "AT+CGSN", "IMEI"),
        of("查询", "AT+CGMM", "模组型号"),
        of("查询", "AT+CGMR", "固件版本"),
        of("查询", "AT+CPIN?", "SIM 卡状态"),
        of("查询", "AT+CCID", "ICCID（卡号）"),
        of("查询", "AT+CIMI", "IMSI"),
        of("查询", "AT+CNUM", "本机号码（数据卡多为空）"),
        of("查询", "AT+CSQ", "信号强度 CSQ"),
        of("查询", "AT+CESQ", "扩展信号质量"),
        of("查询", "AT+CGATT?", "PS 域附着状态"),
        of("查询", "AT+CREG?", "CS 域注册状态"),
        of("查询", "AT+CGREG?", "PS 域注册状态"),
        of("查询", "AT+CEREG?", "EPS/LTE 注册状态"),
        of("查询", "AT+COPS?", "当前运营商"),
        of("查询", "AT+CGDCONT?", "PDP 上下文（APN 配置）"),
        of("查询", "AT+CGCONTRDP", "已激活的 PDP 承载（含本机 IP）"),
        of("查询", "AT+CGACT?", "PDP 激活状态"),
        of("查询", "AT+CGPADDR", "本机 IP 地址"),
        of("查询", "AT+CCLK?", "模组时钟"),
        of("查询", "AT+CFUN?", "功能模式"),
        of("查询", "AT+CMEE?", "错误码详细程度"),
        of("查询", "AT+CMGF?", "短信模式（0=PDU / 1=文本）"),
        of("查询", "AT+CPMS?", "短信存储用量"),
        of("查询", "AT+CSCS?", "当前字符集"),
        of("查询", "AT+CUSD=?", "USSD 支持情况"),
        of("设置（谨慎）", "AT+CMEE=2", "打开详细错误码"),
        of("设置（谨慎）", "AT+COPS=0", "自动选网"),
        of("设置（谨慎）", "AT+CFUN=1,1", "重启模组（会断网）"),
    ]
}

/// 厂商私有指令（**只有这一家会有**）；未识别厂商返回空。
///
/// 具体表在 `modem/vendor/<厂商>.rs`（见 `VendorSpec::at_presets`）——
/// 这里只做合并，不再维护任何一家的指令清单。
pub fn vendor_presets(vendor: Vendor) -> Vec<AtPreset> {
    vendor.spec().at_presets()
}

/// 展锐不支持的通用指令（实测：中兴 F50 / UMS9620 不认不带 cid 的 `AT+CGCONTRDP`）。
fn suppressed_common(vendor: Vendor) -> &'static [&'static str] {
    vendor.spec().suppressed_common()
}

/// 按厂商拼出完整清单（私有表同名覆盖通用项）。
pub fn presets_for(vendor: Vendor) -> Vec<AtPreset> {
    let vendor_only = vendor_presets(vendor);
    let suppressed = suppressed_common(vendor);
    if vendor_only.is_empty() && suppressed.is_empty() {
        return common_presets();
    }
    let override_keys: Vec<String> = vendor_only.iter().map(|p| p.cmd.to_uppercase()).collect();
    let mut out: Vec<AtPreset> = common_presets()
        .into_iter()
        .filter(|p| {
            let k = p.cmd.to_uppercase();
            !override_keys.contains(&k) && !suppressed.contains(&k.as_str())
        })
        .collect();
    out.extend(vendor_only);
    out
}

#[cfg(test)]
mod tests {
    use super::{presets_for, vendor_presets, Vendor};

    fn cmds(list: &[super::AtPreset]) -> Vec<String> {
        list.iter().map(|p| p.cmd.to_string()).collect()
    }

    #[test]
    fn unknown_vendor_gets_common_only() {
        let got = presets_for(Vendor::Unknown);
        assert_eq!(got.len(), super::common_presets().len());
        assert!(cmds(&got).contains(&"AT+CGSN".to_string()));
        // 未识别时不掺任何厂商私有指令
        assert!(!cmds(&got).iter().any(|c| c.contains("QNWPREFCFG")));
    }

    #[test]
    fn vendor_only_override_common() {
        // 移远没有与通用清单同名的私有项 → 通用全保留 + 私有追加
        let got = presets_for(Vendor::Quectel);
        let c = cmds(&got);
        assert!(c.contains(&"AT+CGSN".to_string()));
        assert!(c.contains(&"AT+QGDCNT?".to_string()));
        assert!(c.len() > super::common_presets().len());
    }

    #[test]
    fn unisoc_suppresses_common_cgcontrdp() {
        let got = presets_for(Vendor::Unisoc);
        let c = cmds(&got);
        // 不带 cid 的 AT+CGCONTRDP 被摘掉，只留私有表里带 cid 的两条
        assert!(!c.contains(&"AT+CGCONTRDP".to_string()));
        assert!(c.contains(&"AT+CGCONTRDP=1".to_string()));
        assert!(c.contains(&"AT+CGCONTRDP=?".to_string()));
        // 展锐表里有模板项
        assert!(got.iter().any(|p| p.tpl));
    }

    #[test]
    fn tpl_flag_serializes_omit() {
        let v = vendor_presets(Vendor::Unisoc);
        let tpl_item = v.iter().find(|p| p.tpl).unwrap();
        let json = serde_json::to_string(tpl_item).unwrap();
        assert!(json.contains("\"tpl\":true"));
        let plain = v.iter().find(|p| !p.tpl).unwrap();
        let json = serde_json::to_string(plain).unwrap();
        assert!(!json.contains("tpl"));
    }

    #[test]
    fn all_vendor_tables_non_empty() {
        for v in [
            Vendor::Quectel,
            Vendor::Fibocom,
            Vendor::Simcom,
            Vendor::Meig,
            Vendor::TDTech,
            Vendor::Unisoc,
        ] {
            assert!(!vendor_presets(v).is_empty(), "{:?} 表不能为空", v);
        }
    }
}
