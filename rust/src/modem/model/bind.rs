//! 数据通道绑定（`AT+EMBIND`）的读取与「主机侧有没有份」的判定。
//!
//! ## 为什么单独成模块
//!
//! 它既不属于 PDP（PDP 决定「有没有会话」），也不属于锁频（锁频决定
//! 「用哪条无线链路」），而是决定「**这条会话的数据最终送到谁手上**」。
//! 这是广和通 MTK 平台（FM350 / RW350 系列）双 SoC 结构（AP 域 + MD 域）
//! 独有的一层；把它混进拨号判据会让成功判据失真 —— PDP 明明激活了，
//! 主机却一个包都收不到。
//!
//! ## 三条决定性的逆向结论
//!
//! 1. `M-CCMNI` 与 `M-RNDIS` / `M-MBIM` 是**互斥落点，不是上下游**。
//!    数据要么去 AP 域的 `ccmni%d`，要么去主机侧的 USB 网卡，不会串联。
//! 2. `ccmni0..19` 是 **AP 域内核内建的 CCCI 网卡**，主机侧
//!    `/sys/class/net` 看不到，所以本程序永远不能把它当数据网卡。
//! 3. 绑定是**按 cid** 生效的（`AT+EMBIND=1,"<L2P>",<cid>`，`cid` 1-16），
//!    所以理论上 cid1 绑 `M-RNDIS` 给主机、cid2 绑 `M-CCMNI` 给模组自身
//!    可以并存 —— 但这需要运营商支持双 PDP，且对主机侧连通性没有收益。
//!
//! ## 本模块的边界
//!
//! 只做**体检**，默认**不介入拨号主流程**：自动改绑会打断 MD 侧 D2RM 的
//! bind / unbind / rebind 状态机，还可能撞上 `invalid rat`（绑定合法性
//! 与当前 RAT 相关）。因此这里只提供只读状态，写形式留给人工排障路径。

use serde::Serialize;

use super::modem::Modem;

/// 读形式。
pub const EMBIND_READ: &str = "AT+EMBIND?";
/// 测试形式，用于枚举本固件支持的 L2P 与 cid 范围。
pub const EMBIND_TEST: &str = "AT+EMBIND=?";

/// L2P 常量：AP 域 CCCI 网卡。
pub const L2P_CCMNI: &str = "M-CCMNI";
/// L2P 常量：USB RNDIS。
pub const L2P_RNDIS: &str = "M-RNDIS";
/// L2P 常量：USB MBIM。
pub const L2P_MBIM: &str = "M-MBIM";
/// L2P 常量：本地主机接口。
pub const L2P_LHIF: &str = "M-LHIF";

/// 能把数据送到**主机侧**的 L2P。
const HOST_L2P: &[&str] = &[L2P_RNDIS, L2P_MBIM];

/// 绑定形式（人工 / 排障路径使用，包内不自动下发）。
pub fn bind_l2p(l2p: &str, cid: u32) -> String {
    format!("AT+EMBIND=1,\"{l2p}\",{cid}")
}

/// 解绑形式（人工 / 排障路径使用，包内不自动下发）。
pub fn unbind_l2p(l2p: &str, cid: u32) -> String {
    format!("AT+EMBIND=0,\"{l2p}\",{cid}")
}

/// 一条 L2P 的绑定状态。
#[derive(Debug, Clone, Serialize)]
pub struct BindEntry {
    pub l2p: String,
    pub bound: bool,
}

/// `AT+EMBIND?` 的解析结果。
#[derive(Debug, Default, Serialize)]
pub struct BindState {
    /// 解析出的条目，顺序与模组回显一致。
    pub entries: Vec<BindEntry>,
    /// 原始回显行。解析不出条目时这是排障的唯一证据，必须保留。
    pub raw: Vec<String>,
}

impl BindState {
    /// 是否存在**已绑定**且能把数据送到主机的通道。
    pub fn host_bound(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.bound && HOST_L2P.contains(&e.l2p.as_str()))
    }

    /// `M-CCMNI` 是否处于绑定态（数据去了模组自己，主机拿不到）。
    pub fn ccmni_bound(&self) -> bool {
        self.entries.iter().any(|e| e.bound && e.l2p == L2P_CCMNI)
    }

    /// 体检结论，供状态页 / 日志直接展示。
    ///
    /// * `host` —— 主机侧有份，数据面方向正确；
    /// * `ccmni-only` —— 数据被送去模组自身，主机必然拿不到地址；
    /// * `none` —— 读到了条目但没有任何一条处于绑定态；
    /// * `unknown` —— 一条都没解析出来（命令不支持或回显形态未覆盖）。
    pub fn verdict(&self) -> &'static str {
        if self.entries.is_empty() {
            return "unknown";
        }
        if self.host_bound() {
            return "host";
        }
        if self.ccmni_bound() {
            return "ccmni-only";
        }
        "none"
    }

    /// 面向用户的解释文本，日志与状态页共用一份，避免各处各写一套话术。
    pub fn explain(&self) -> &'static str {
        match self.verdict() {
            "host" => "数据通道已绑定到主机侧（M-RNDIS / M-MBIM）",
            "ccmni-only" => {
                "数据通道被绑定到 M-CCMNI：数据去了模组自身 AP 域的 ccmni，主机侧拿不到地址"
            }
            "none" => "没有任何 L2P 处于绑定态，数据通道未建立",
            _ => "未能解析 AT+EMBIND 回显，数据通道方向未知",
        }
    }
}

/// 解析 `AT+EMBIND` 回显。
///
/// 本命令的回显形态是 `+EMBIND: %d` 后接若干 `,"<L2P>",%d` 续行，
/// 不同版本可能拼成一行或多行。这里做**宽容匹配**：把所有 `+EMBIND` 行与
/// 以 `,"M-` 开头的续行拼成一串，再按 `("<M-*>", <0|1>)` 成对取值。
/// 解析不出来时返回空 `entries`，调用方按 `unknown` 处理，绝不假设默认值。
pub fn parse_embind(resp: &str) -> BindState {
    let mut raw = Vec::new();
    let mut flat = String::new();
    for line in resp.lines() {
        let l = line.trim();
        if l.starts_with("+EMBIND") || l.starts_with(",\"M-") {
            raw.push(l.to_string());
            flat.push_str(l);
            flat.push(',');
        }
    }

    let toks: Vec<&str> = flat
        .split(',')
        .map(|s| s.trim().trim_matches('"'))
        .collect();
    let mut entries: Vec<BindEntry> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i].starts_with("M-") {
            if let Some(v) = toks.get(i + 1).and_then(|s| s.parse::<u32>().ok()) {
                let l2p = toks[i].to_string();
                if !entries.iter().any(|e| e.l2p == l2p) {
                    entries.push(BindEntry { l2p, bound: v != 0 });
                }
                i += 2;
                continue;
            }
        }
        i += 1;
    }

    BindState { entries, raw }
}

impl Modem {
    /// 读一次数据通道绑定状态（只读）。
    ///
    /// **必须过机型闸**：`AT+EMBIND` 是广和通 MTK 平台的私有指令，
    /// 发到其它厂商模组上只会白等一个超时。非目标机型直接返回空状态，
    /// 调用方会看到 `unknown`。
    ///
    /// 这是**低频**调用：每次状态刷新都问一遍会白白占用半双工 AT 通道。
    pub fn embind_status(&self) -> BindState {
        if !self.is_fibocom_mtk() {
            return BindState::default();
        }
        match self.query_first(&[EMBIND_READ]) {
            Some(resp) => parse_embind(&resp),
            None => BindState::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_line_form() {
        let st = parse_embind("+EMBIND: 1,\"M-RNDIS\",1,\"M-CCMNI\",0\r\nOK\r\n");
        assert_eq!(st.verdict(), "host");
        assert!(st.host_bound());
        assert!(!st.ccmni_bound());
        assert_eq!(st.entries.len(), 2);
    }

    #[test]
    fn parses_continuation_line_form() {
        // 固件里的格式串是 +EMBIND: %d 后接若干 ,"<L2P>",%d 续行
        let st = parse_embind(
            "+EMBIND: 4\r\n,\"M-CCMNI\",1\r\n,\"M-RNDIS\",0\r\n,\"M-MBIM\",0\r\nOK\r\n",
        );
        assert_eq!(st.verdict(), "ccmni-only");
        assert!(st.ccmni_bound());
        assert!(!st.host_bound());
        assert_eq!(st.entries.len(), 3);
    }

    #[test]
    fn unparsable_response_is_unknown_not_host() {
        let st = parse_embind("\r\nERROR\r\n");
        assert_eq!(st.verdict(), "unknown");
        assert!(!st.host_bound());
        assert!(st.entries.is_empty());
    }

    #[test]
    fn all_zero_is_none() {
        let st = parse_embind("+EMBIND: 2\r\n,\"M-CCMNI\",0\r\n,\"M-RNDIS\",0\r\nOK\r\n");
        assert_eq!(st.verdict(), "none");
    }

    #[test]
    fn duplicate_entries_are_collapsed() {
        let st = parse_embind("+EMBIND: 1,\"M-RNDIS\",1\r\n+EMBIND: 1,\"M-RNDIS\",0\r\nOK\r\n");
        assert_eq!(st.entries.len(), 1);
        assert!(st.host_bound());
    }

    #[test]
    fn explain_covers_every_verdict() {
        let cases = [
            ("+EMBIND: 1,\"M-RNDIS\",1\r\nOK\r\n", "host"),
            ("+EMBIND: 1,\"M-CCMNI\",1\r\nOK\r\n", "ccmni-only"),
            ("+EMBIND: 1,\"M-RNDIS\",0\r\nOK\r\n", "none"),
            ("\r\nERROR\r\n", "unknown"),
        ];
        for (resp, want) in cases {
            let st = parse_embind(resp);
            assert_eq!(st.verdict(), want, "{resp}");
            assert!(!st.explain().is_empty(), "{resp}");
        }
    }

    #[test]
    fn command_forms_match_firmware() {
        assert_eq!(EMBIND_READ, "AT+EMBIND?");
        assert_eq!(EMBIND_TEST, "AT+EMBIND=?");
        assert_eq!(bind_l2p(L2P_RNDIS, 1), "AT+EMBIND=1,\"M-RNDIS\",1");
        assert_eq!(unbind_l2p(L2P_CCMNI, 1), "AT+EMBIND=0,\"M-CCMNI\",1");
        assert_eq!(HOST_L2P.len(), 2);
    }
}
