# 设备与协议支持矩阵

> 本表由 `rust/src/modem/modem_catalog.json`（91 个型号 / 12 家厂商）与
> `rust/src/modem/vendor/datacall.rs`（厂商 × 平台的数据通道配方）**生成并人工核对**，
> 最后更新：2026-10-21（拨号生命周期统一重构，见 `docs/DIAL_STATE_MACHINE.md`）。
>
> 图例：
> - **USB / PCIe**：清单登记的数据形态（`qmi/ecm/mbim/rndis/ncm/gobinet/rmnet/auto`）。
> - **ECM/RNDIS/NCM 数据通道**：非 QMI/MBIM 形态下模组侧真正执行的激活指令族；
>   `AT+CGACT` 表示只走 3GPP 通用路径。
> - **验证状态**：`单测` = 仅有自动化测试（含 golden 命令行断言）；
>   `实机` = 真机跑通拨号/断开的完整链路。**本表没有任何型号标为「实机」**，
>   原因见文末「验证边界」。
>
> 平台列的含义：`qualcomm` 高通、`unisoc` 展锐、`mediatek` 联发科、`hisilicon` 海思/巴龙、
> `lte` / `lte12` 高通 LTE 平台、`asrmicro` 翱捷。

## 1. 逐型号矩阵

| 型号 | 厂商 | 平台 | USB | PCIe | ECM/RNDIS/NCM 数据通道 | 验证状态 |
| --- | --- | --- | --- | --- | --- | --- |
| `fm150-ae` | fibocom | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/mbim | AT+GTRNDIS | 单测 |
| `fm160-cn` | fibocom | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/mbim | AT+GTRNDIS | 单测 |
| `fm160-eau` | fibocom | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/mbim | AT+GTRNDIS | 单测 |
| `fm160-na` | fibocom | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/mbim | AT+GTRNDIS | 单测 |
| `fm170-eau` | fibocom | qualcomm | qmi/ecm/mbim/rndis/ncm | qmi/mbim | AT+GTRNDIS | 单测 |
| `fm190w-gl` | fibocom | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi | AT+GTRNDIS | 单测 |
| `fm350-gl` | fibocom | mediatek | rndis | mbim | AT+CGACT（MTK 无 GTRNDIS） | 单测 |
| `fm350-gl-00` | fibocom | mediatek | rndis | — | AT+CGACT（MTK 无 GTRNDIS） | 单测 |
| `fm350r-gl` | fibocom | mediatek | rndis | mbim | AT+CGACT（MTK 无 GTRNDIS） | 单测 |
| `fm650-cn` | fibocom | unisoc | ecm/mbim/rndis/ncm | — | AT+GTRNDIS | 单测 |
| `l610-eu` | fibocom | unisoc | ecm/rndis/ncm | — | AT+GTRNDIS | 单测 |
| `nl668` | fibocom | lte | ecm/rndis/ncm | — | AT+GTRNDIS | 单测 |
| `nl678` | fibocom | lte | qmi/ecm/rndis | — | AT+GTRNDIS | 单测 |
| `rw350r-gl` | fibocom | mediatek | rndis | mbim | AT+CGACT（MTK 无 GTRNDIS） | 单测 |
| `t77w968` | foxconn | qualcomm | qmi/mbim | — | AT+CGACT | 单测 |
| `t99w175` | foxconn | qualcomm | mbim/rmnet | mbim | AT+CGACT | 单测 |
| `t99w368` | foxconn | qualcomm | mbim/rmnet | mbim | AT+CGACT | 单测 |
| `t99w373` | foxconn | qualcomm | mbim/rmnet | mbim | AT+CGACT | 单测 |
| `t99w640` | foxconn | qualcomm | — | mbim | —（无 USB 形态） | 单测 |
| `gm800` | gosuncn | qualcomm | qmi/ecm/mbim/rndis | mbim | AT+CGACT | 单测 |
| `me3630-w` | gosuncn | lte | ecm | — | AT+ZECMCALL | 单测 |
| `mh5000-31` | huawei | hisilicon | ecm/ncm | — | AT^NDISDUP | 单测 |
| `mh5000-82` | huawei | unisoc | ecm/ncm/rndis | — | AT^SETAUTODIAL | 单测 |
| `mh5000-82m` | huawei | unisoc | ecm/ncm/rndis | — | AT^SETAUTODIAL | 单测 |
| `mt5700m-cn` | huawei | hisilicon | ecm/ncm | — | AT^NDISDUP | 单测 |
| `mt5710_cn` | huawei | hisilicon | ecm/ncm | — | AT^NDISDUP | 单测 |
| `s10` | huawei | hisilicon | rndis | — | AT^NDISDUP | 单测 |
| `srm815` | meig | qualcomm | ecm/rndis/ncm | — | AT$QCRMCALL | 单测 |
| `srm821` | meig | unisoc | ecm/rndis/ncm | — | AT+CGACT | 单测 |
| `srm825` | meig | qualcomm | ecm/rndis/ncm | — | AT$QCRMCALL | 单测 |
| `srm825n` | meig | qualcomm | ecm/rndis/ncm | — | AT$QCRMCALL | 单测 |
| `n510m` | neoway | unisoc | ecm | — | AT$MYUSBNETACT | 单测 |
| `air724ug` | openluat | unisoc | ecm/rndis | — | AT+RNDISCALL | 单测 |
| `ag598eeu` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `ec200a` | quectel | lte | ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `ec20f` | quectel | lte | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `ec21` | quectel | lte | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `ec25` | quectel | lte | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `eg25` | quectel | lte | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `em05` | quectel | lte | qmi/ecm/mbim/rndis | — | AT+QNETDEVCTL | 单测 |
| `em05-cn` | quectel | lte | qmi/ecm/mbim/rndis | — | AT+QNETDEVCTL | 单测 |
| `em12` | quectel | lte | qmi/ecm/mbim/rndis | — | AT+QNETDEVCTL | 单测 |
| `em120k` | quectel | lte12 | qmi/ecm/mbim/rndis | — | AT+QNETDEVCTL | 单测 |
| `em160r-gl` | quectel | lte12 | qmi/ecm/mbim/rndis | — | AT+QNETDEVCTL | 单测 |
| `ep06` | quectel | lte | qmi/ecm/mbim/rndis | — | AT+QNETDEVCTL | 单测 |
| `mh8001-eu` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg200u-cn` | quectel | unisoc | ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rg500q-ea` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg501q-eu` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg520f-eb` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg520f-eu` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg520f-jo` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg520n-eb` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg520n-eu` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg530f-na` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg650v-eu` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rg801` | quectel | hisilicon | ecm/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm500q-ae` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm500q-cn` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm500q-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm500s-ce` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm500u-cn` | quectel | unisoc | ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm500u-cnv` | quectel | unisoc | ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm500u-ea` | quectel | unisoc | ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm502q-ae` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm502q-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm505q-ae` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm510q-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm510q-glha` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm520f-bl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm520f-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm520n-cn` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | — | AT+QNETDEVCTL | 单测 |
| `rm520n-eu` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm520n-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm521f-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm530n-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `rm551e-gl` | quectel | qualcomm | qmi/gobinet/ecm/mbim/rndis/ncm | qmi/gobinet/mbim | AT+QNETDEVCTL | 单测 |
| `em9190` | sierra | qualcomm | mbim/rmnet | mbim/rmnet | AT+CGACT | 单测 |
| `em9291` | sierra | qualcomm | mbim/rmnet | mbim/rmnet | AT+CGACT | 单测 |
| `mc7354` | sierra | lte | qmi | — | AT+CGACT | 单测 |
| `mc7355` | sierra | lte | qmi | — | AT+CGACT | 单测 |
| `a7605c` | simcom | lte | ecm/rndis/auto | — | AT+CGACT | 单测 |
| `a8200c-m2` | simcom | asrmicro | ecm/rndis | — | AT+CGACT | 单测 |
| `simcom_a8200_serias` | simcom | asrmicro | ecm/rndis | — | AT+CGACT | 单测 |
| `simcom_d8200g` | simcom | qualcomm | qmi/rndis | — | AT+CGACT | 单测 |
| `simcom_sim8200ea-m2` | simcom | qualcomm | qmi/rndis | — | AT+CGACT | 单测 |
| `simcom_sim8202g-m2` | simcom | qualcomm | qmi/rndis/mbim | — | AT+CGACT | 单测 |
| `simcom_sim8380g-m2` | simcom | qualcomm | qmi/rndis | qmi | AT+CGACT | 单测 |
| `fn990a40` | telit | qualcomm | mbim/rndis/qmi/ecm | mbim/qmi | AT#ICMAUTOCONN | 单测 |
| `mv32-w-a` | thales | qualcomm | mbim | — | AT+CGACT | 单测 |
| `mv32-w-b` | thales | qualcomm | mbim/rmnet | — | AT+CGACT | 单测 |

## 2. 厂商 × 平台 × 数据通道（激活 / 挂断）

| 厂商 | 平台 | 激活 | 挂断 | 上游依据 | 备注 |
| --- | --- | --- | --- | --- | --- |
| quectel | hisilicon | `AT+QNETDEVCTL=1,1,1` | `AT+QNETDEVCTL=<cid>,2,1` | QModem `modem_dial.sh:1298-1307` | 不支持指定 cid |
| quectel | unisoc | `AT+QNETDEVCTL=1,<cid>,1` | `AT+QNETDEVCTL=<cid>,2,1` | 同上 `:1308-1312` | |
| quectel | 其它/qualcomm | `AT+QNETDEVCTL=3,<cid>,1` | `AT+QNETDEVCTL=<cid>,2,1` | 同上 `:1313-1316` | connect_type=3 |
| fibocom | mediatek | `AT+CGACT=1,<cid>` | `AT+CGACT=0,<cid>` | 同上 `:1342-1344` | FM350/RW350 无 GTRNDIS |
| fibocom | qualcomm/unisoc/lte | `AT+GTRNDIS=1,<cid>` | `AT+GTRNDIS=0,<cid>` | 同上 `:1322-1340`、`:1352-1382` | |
| huawei | hisilicon | `AT^NDISDUP=1,<cid>` | `AT^NDISDUP=0,0` | 同上 `:1386-1400` | |
| huawei | unisoc | `AT^SETAUTODIAL=1,4` | `AT^SETAUTODIAL=0` → `AT+CFUN=0` → `AT+CFUN=1` | 同上 `:1533-1567` | 模组自拨 |
| tdtech | hisilicon | `AT^NDISDUP=1,<cid>` | `AT^NDISDUP=0,0` | 同上（MT5700 走 huawei 分支） | 失败回退 CGACT |
| meig | qualcomm | `AT$QCRMCALL=1,0,3,2,<cid>` | `AT$QCRMCALL=0,0,3,2,<cid>` | 同上 `:1443-1451` | |
| neoway | unisoc | `AT$MYUSBNETACT=0,1` | `AT$MYUSBNETACT=0,0` | 同上 `:1453-1461` | 断开后需 ~3s |
| telit | qualcomm | `AT#ICMAUTOCONN=1,<cid>` | `AT+CGACT=0,<cid>` | 同上 `:1466-1473` | 上游无断开指令 |
| gosuncn | lte | `AT+ZECMCALL=1` | `AT+ZECMCALL=0` | 同上 `:1475-1482` | |
| openluat | unisoc | `AT+RNDISCALL=1` | 不发（模组自持 PDP） | 同上 `:1497-1503`、`:1569-1574` | 只停主机接口 |
| simcom | 全部 | `AT+CGACT=1,<cid>` | `AT+CGACT=0,<cid>` | 同上 `:1417-1441` | 高通平台 CNMP 保活值不写死 |

> 平台未识别（清单里查不到型号）时，一律走 `AT+CGACT`，不发任何私有指令
> （`datacall.rs` 的 `plan()` 在 `platform.is_empty()` 时直接返回通用配方）。

## 3. 协议职责边界

| 形态 | 谁建立数据会话 | 谁拉网卡 | 谁配地址/路由 | 本项目实现 |
| --- | --- | --- | --- | --- |
| QMI | `modemd` 内置 QMI 拨号器（`runtime/dialer/native/qmi_link.rs`） | 内核 `qmi_wwan` | `netprov` + netifd（`modem_pro` 协议） | 已实现 |
| MBIM | `modemd` 内置 MBIM 拨号器（`native/mbim_link.rs`） | 内核 `cdc_mbim` | 同上 | 已实现 |
| ECM / RNDIS / NCM | 模组固件（本表第 2 节指令） | 模组 USB 功能 | 主机 DHCP；FM350 系列改静态（`netprov/provision.rs`） | 已实现 |
| GobiNet / RMNet | 同 ECM 路径（模组侧） | 厂商驱动 | DHCP | 按清单登记，未单独验证 |
| PPP | 未实现（上游 QModem 也没有 PPP 数据面，`ppp` 只是认证命令口径） | — | — | 明确不支持 |

**生命周期统一（2026-10-21 起）**：四种形态的「谁决定连 / 何时重连 / 失败退避 / 状态归属」
统一由 `runtime/dialer` 的 `DialerSupervisor::reconcile` 负责；表里的差异只保留在
**数据会话由谁建立**这一层（QMI/MBIM = 主机侧协议；ECM/RNDIS/NCM = 模组侧 AT 配方）。
`DialState` 由后端回调写入（`note_native_up/down`、`dial_inner`、`hangup`），
不存在第二个状态来源。

## 4. 验证边界（务必读）

- **自动化**：`cargo test --locked` 共 **746 项**（2026-10-21）。
  其中拨号生命周期相关的新增用例覆盖：两种后端的统一退避序列、退避在「错误变化 /
  成功」时的清零规则、自动连接的全部拒绝理由、连接代次对陈旧提交的拦截、
  原生后端状态写入与掉线回退、外部断链（重启/飞行模式）对在途拨号的作废。
  厂商配方字面量、平台回退、cid 替换、激活/挂断对称、端口规则与物理分组另有既有用例。
- **未做真机验证**：本次会话没有可用的 5G 模组与运营商网络，因此：
  - 表中所有型号的「实机」列均为空；
  - FM350-GL / RM520N-GL 等机型的 RNDIS/ECM 路径只做了指令级与状态机级验证；
  - 需要在真机上补做的最小清单见 `docs/UPSTREAM_AUDIT.md` §5。
