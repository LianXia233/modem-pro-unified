# 上游审计与整合对照表（UPSTREAM_AUDIT）

> 审计对象（均为 `--depth 1` 克隆，记录当时 HEAD）：
>
> | 仓库 | HEAD | 提交时间 | 角色 |
> | --- | --- | --- | --- |
> | [FUjr/QModem](https://github.com/FUjr/QModem) | `2ef9227` | 2026-10-19 | 模组知识底座（清单 / 端口规则 / 方言脚本 / AT 命令库） |
> | [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro) | `397e014` | 2026-10-17 | Go 原版（Rust 版的行为基线 + 逐型号怪癖表） |
> | 本仓库 `modem-pro-unified` | `c959a97` | 2026-10-20 | 集成目标（Rust 单体后端 + LuCI + WebUI） |
>
> 本文只记录**用源码核对过**的事实：每一项都能在对应文件里找到出处；
> 没核到的写「未核实」，不写成「已支持」。

---

## 1. 三仓库规模

| | QModem | modem-pro（Go） | modem-pro-unified（Rust） |
| --- | --- | --- | --- |
| 文件数 | 808 | 738 | — |
| 后端源码 | shell 12,126 行（`vendor/*.sh`） | Go 约 9 万行（`module/*`，含 kernel 69k） | Rust 51,538 行 / 238 文件 |
| 前端 | LuCI（10+ 个 luci-app 包） | 单文件 WebUI | LuCI + WebUI（`rust/web/index.html` 8.9k 行 + `mini.html`） |
| 架构 | 每功能一个应用（C/shell）+ LuCI | Go 单体 + `module/` 分层 | Rust 单体 + `modem/{vendor,model,at,detect}` 分层 |

> QModem 的 `application/` 下有 18 个独立应用（`qmodem`、`modem_scan`、`qfirehose`、
> `qmodem_smsd`、`sms_forwarder*`、`voipd`、`qmodem_uplink_manager`、`tom_modem`…），
> 它们是**另一套架构**（每个功能一个守护进程 + rpcd 转发），不是「模组知识」。
> 本次整合的边界见 §6。

---

## 2. 模组清单覆盖

数据源：`QModem/application/qmodem/files/usr/share/qmodem/modem_support.json`。

| 段 | 条数 | 本仓库状态 |
| --- | --- | --- |
| `usb` | 90 | `rust/src/modem/modem_catalog.json` 91 条（**全覆盖**，另加 T99W640，PCIe-only） |
| `pcie` | 41 | 合并进 catalog 的 `pcie` / `pcie_at` 字段 |
| `device` | 9 | 合并进 catalog（型号 → 支持的形态） |

字段级对照（`modem_support.json` → `modem_catalog.json`）：

| QModem 字段 | 本仓库 | 说明 |
| --- | --- | --- |
| `manufacturer` | `vendor` | 12 家 |
| `platform` | `platform` | qualcomm / unisoc / mediatek / hisilicon / lte / lte12 |
| `modes` | `usb` | 数据协议列表，顺序即优先级 |
| — | `usb_at` / `pcie_at` | 逐模式切换指令（QModem 在 shell 里给，已提取成数据） |
| `manufacturer_id` | **`vid`（本轮新增）** | 4 位十六进制；5 条上游本来就没有（`simcom_sim82xx` ×3、`me3630-w`） |
| `lte_band` / `wcdma_band` / `nsa_band` / `sa_band` | `modem_bands.json`（92 条） | 已迁移 |
| `pdp_index` / `data_interface` / `settings` | **未迁移** | 见 §6.3 |

厂商分布（USB 段）：quectel 44 / fibocom 14 / simcom 7 / huawei 6 / meig 4 / sierra 4 /
foxconn 4 / thales 2 / gosuncn 2 / openluat 1 / telit 1 / neoway 1。

---

## 3. 数据资产迁移

| 资产 | 上游 | 本仓库 | 状态 |
| --- | --- | --- | --- |
| 型号清单 | QModem `modem_support.json` | `modem_catalog.json`（91） | 完成 |
| 频段能力 | 同上（4 个 `*_band` 字段） | `modem_bands.json`（92） | 完成 |
| USB 端口规则 | QModem `modem_port_rule.json`（18 条） | **`modem_ports.json` + `ports.rs`（本轮）** | 完成 |
| VID 映射 | QModem `manufacturer_id` | catalog 的 `vid` 字段 + `catalog::vendor_by_vid` | 完成 |
| 逐型号怪癖 | Go `module/modelquirk/table.go`（19 条） | **`modem_quirks.json` + `quirk.rs`（本轮）** | 部分：4 个字段已接入，见 §3.1 |
| 厂商 AT 预设 | QModem `at_commands_*.json`（10 组） | `vendor/*.rs::at_presets`（7~39 条/家） | 本仓库更细，无需回灌 |
| 厂商方言脚本 | QModem `vendor/*.sh`（12 家 / 12k 行） | `vendor/*.rs`（15 个模块 / 2.5k 行）+ `model/*`（17.6k 行） | 见 §4 |

### 3.1 怪癖表接入情况

`modem_quirks.json` 共 19 条，每条的 `note` 都保留上游实测依据（固件版本 + 原始应答）。
**只有下列字段接入了调用链**，其余字段在数据里保留、代码不读（避免死代码）：

| 字段 | 接入点 | 行为 |
| --- | --- | --- |
| `voltage_cmd` | `model/siminfo.rs::modem_status` | 型号特例优先（`AT!PCVOLT?` / `AT#CBC`），`AT+CBC` 兜底 |
| `iccid_cmd` | 同上 | `AT+ICCID` / `AT$MYCCID` 优先，三条标准写法兜底 |
| `temp_cmd` | `model/siminfo.rs::query_temps` | 插到厂商候选最前（FM160 的 `AT+GTLADC`），失败仍退回 |
| `note` | — | 只作证据，不参与逻辑 |
| `ignore_cclk_tz` | **未接入** | Rust 的 `parse_cclk` 从一开始就忽略 `±zz` 后缀（等于对所有型号都「不按后缀换算」）。上游语义相反：`false` 表示「后缀可信、要减」。直接接入会让所有未登记型号的时钟偏差从 0 变成 -8h，**必须先实机核对**，见 §7 |

---

## 4. 厂商与方言覆盖

本仓库 15 个厂商模块（`rust/src/modem/vendor/`），QModem 12 家脚本，Go 12 家模块：

| 厂商 | QModem 脚本 | Go 模块 | Rust 模块 | 差异说明 |
| --- | --- | --- | --- | --- |
| quectel 移远 | ✔ | ✔ | ✔ | Rust 有 QENG/QNWINFO/QCAINFO/QGDCNT/QTEMP/QNWPREFCFG |
| fibocom 广和通 | ✔ | ✔ | ✔ | 含 FM350 MTK 平台分支（RNDIS/温度/GTSENRDTEMP） |
| simcom 芯讯通 | ✔ | ✔ | ✔ | CNMP / CNWSEARCH / CPMUTEMP |
| meig 美格 | ✔ | ✔ | ✔ | — |
| tdtech 鼎桥 | （归在 huawei） | ✔ | ✔ | MT5700 的 `^` 私有族 |
| huawei 华为 | ✔ | ✔ | ✔ | 见 §4.1 |
| unisoc 展锐 | （归在厂商内） | ✔ | ✔ | SP* 族；`ATI` 不支持时的回落 |
| foxconn 富士康 | ✔ | ✔ | ✔ | NV 写入 |
| gosuncn 高新兴 | ✔ | ✔ | ✔ | `Z` 私有族 |
| neoway 有方 | ✔ | ✔ | ✔ | `$` 私有族 |
| openluat 合宙 | ✔ | ✔ | ✔ | `*` 私有族；`AT+ICCID` |
| sierra | ✔ | ✔ | ✔ | `!` 私有族 |
| telit | ✔ | ✔ | ✔ | `#` 私有族 |
| thales | ✔ | ✔ | ✔ | `^` 私有族 |
| unknown | — | ✔ | ✔ | 只给 3GPP 通用能力 |

### 4.1 已确认的方言缺口（本轮未补，理由写明）

按「AT 指令名」做差集（`QModem/vendor/*.sh` ∪ `Go/module` vs `rust/src`），
QModem 有而 Rust 没有的指令名 14 个、Go 有而 Rust 没有的 87 个。逐条甄别后：

- **属于「同一功能换了个写法」**（不算缺口）：`AT+CCID` 族（Rust 用候选列表）、
  `AT+CGPADDR` 变体（Rust 侧有三种固件写法的解析）等；
- **本轮已补**：`AT!PCVOLT?`、`AT#CBC`、`AT+ICCID`、`AT$MYCCID`、`AT+TEMP`、
  `AT+GTLADC`、`AT^TEMP`、`AT!TEMP?`、`AT!PCTEMP?`、`AT+MTSM`、`AT#TEMPSENS`
  （通过 `modem_quirks.json` 的电压/温度/ICCID 字段）；
- **仍是缺口**（无本机可验证设备，不硬写解析器）：
  - `AT^HVSST`（华为电压）、`AT^TEMP`（华为温度）—— Rust 的 `Huawei` 分支没有温度
    解析器，补指令而不补解析等于假支持；
  - `AT+EEMGINFO`（SIMCom 工程模式信息）；
  - `AT+GTCELLLOCK`（高新兴锁小区）；
  - `AT+SWITCH_SLOT`（双卡切换）—— 整条功能（SIM 槽位）本仓库未实现。

---

## 5. 识别体系（本轮重构）

### 5.1 旧流程的问题

`Modem::identify()` 只发 `ATI` → 关键字 → 取短名。三个后果：
ATI 回 ERROR 的固件（部分展锐）识别落空；型号撞车无从校验；认错了页面上看不出来。

### 5.2 新流程

```
             ┌────────────────────────────┐
  sysfs ────►│ usbdev::identity_for_tty() │  VID:PID + 接口号 + 拓扑
             └──────────────┬─────────────┘
                            │
  AT（只读）──► ATI / CGMM / CGMR ──► 文本行
                            │
                    ┌───────▼────────┐
                    │ modem::detect  │  多维度取证 + 冲突判定
                    └───────┬────────┘
                            │  Identify { vendor, model, matched_by, confidence,
                            │             evidence[], candidates[], conflicts[] }
                    ┌───────▼────────┐
                    │ Modem::identify│  → vendor / model / model_raw / ident
                    └───────┬────────┘
                            │
              ┌─────────────┴──────────────┐
              ▼                            ▼
      LuCI「识别依据」行            WebUI「识别依据」行（同一份 JSON）
```

### 5.3 证据优先级

| 依据 | 来源 | 置信度 | 条件 |
| --- | --- | --- | --- |
| `port_rule` | `modem_ports.json` 的 `VID:PID` | 90 | 且与 AT 型号一致 |
| `catalog_model` | AT 文本命中清单型号 | 85 | 设备自报 |
| `port_rule`（无 AT 型号） | 同上 | 80 | AT 回 ERROR 时仍能定型号 |
| `port_rule`（清单无此型号） | 同上 | 70 | 只认到规则里的型号 |
| `vendor_keyword` | ATI 关键字 | 55 | 只认到厂商 |
| `usb_vid` | 清单里 `VID → 唯一厂商` | 45 | 仅独占 VID |
| `no_match` | — | 0 | 未知设备，只留 3GPP 通用能力 |

### 5.4 冲突处理规则

1. 设备自报（AT 型号）**优先**于 USB VID 推断 —— 私有指令按厂商方言发，认错厂商的代价最大；
2. 但厂商会**按清单归属纠正**，避免「清单说 A、方言按 B 发」；
3. `huawei` 与 `tdtech` 视为同族（`MT5700M` 共线），不报冲突；
4. 任何冲突都会把置信度降 15，并原样写进 `conflicts[]`，前端直接显示；
5. 未知设备**不猜**：`model = None`，只保留 3GPP 通用能力。

### 5.5 型号匹配（本轮增强）

`catalog::lookup` 原来只做「整名包含」，`ATI` 只报 `MT5700M` 时匹配不到清单里的
`mt5700m-cn`。现在退到**型号主干**（剥掉 1~3 位区域后缀 `-cn` / `_gl`），
并要求主干 ≥5 位，避免 `s10` 这类短名误伤；同时提供 `candidates_for()` 列出
所有可能命中的型号（区域版本不确定时给候选，不给「已确认」）。

---

## 6. 未迁移项（明确边界）

### 6.1 QModem 的独立应用层

| 应用 | 作用 | 为什么没迁 |
| --- | --- | --- |
| `voipd` / `qmodem_sipd` | SIP 语音 | 需要 Asterisk/PJSIP 与音频通路，属另一条产品线 |
| `sms_forwarder` / `sms_forwarder_next` | 短信转发到邮件/Webhook | 本仓库已有 `wechat` 推送；短信转发属独立功能，未排期 |
| `qfirehose` | 高通固件刷写 | 高风险写操作，与「只读优先」原则冲突 |
| `qmodem_uplink_manager` | 多链路上行管理 | 与 netifd/mwan3 职责重叠 |
| `qmodem-seal` / `tom_modem` / `qmodem_monitor` | 密封校验 / 监控 | 与本仓库监控重叠 |
| LED 脚本、`ubus_at_daemon` | 指示灯、ubus AT 转发 | 本仓库有 HTTP/WS 通道，不需要 ubus AT 守护进程 |

### 6.2 内核驱动

QModem 的 `driver/`（quectel_MHI、quectel/simcom/fibocom QMI_WWAN、nss）是内核模块，
本仓库是用户态程序，驱动由固件/内核包提供。**不在整合范围**。

### 6.3 数据字段

| 字段 | 状态 | 说明 |
| --- | --- | --- |
| `pdp_index` | 未迁移 | 本仓库的 PDP 上下文由拨号参数 `cid` 显式给（`/api/dial/connect`），不需要「型号默认索引」 |
| `data_interface` | 未迁移 | 等价信息在 `modes` 里（usb/pcie 协议列表） |
| `settings.urc` | 未迁移 | 上游只登记了 RM520N 一条（`AT+CNMI=2,1,0,0,0`）。本仓库短信 URC 由 `at/urc.rs` 统一处理，未按型号下发 CNMI。**若某型号短信收不到，先查这里** |
| `settings.lock_cell.persists_reboot` | 未迁移 | 本仓库锁小区不持久化（重启丢失），与上游一致（同一字段为 `false`） |

---

## 7. 验证记录

### 7.1 自动化测试（本轮新增 31 个）

| 模块 | 用例数 | 覆盖 |
| --- | --- | --- |
| `modem::detect` | 9 | AT 型号 / 端口规则 / VID 独占 / 多厂商 VID / 冲突降级 / 华为族 / 未知设备 / 原始应答保留 |
| `modem::ports` | 5 | 18 条规则完整性、AT 接口白名单、语音口排除、大小写、空规则 |
| `modem::quirk` | 6 | 19 条齐全且都有 evidence、电压按型号、ICCID 私有指令、温度、时区标记、大小写与空串 |
| `modem::catalog` | 5（原有）+ 主干匹配 | 91 条、最长优先、未知不猜、切换指令来自 QModem |

`cargo test` 全量：**678 passed / 0 failed**。

### 7.2 实机验证

设备：FM350-GL（`0e8d:7127`），ImmortalWrt aarch64，modemd 1.0.0-8。

```
GET /api/modems →
  vendor=fibocom  model=fm350-gl  at_port=/dev/ttyUSB4
  identify:
    matched_by = catalog_model   confidence = 85
    usb = 0e8d:7127   interfaces = 1.0 … 1.9
    evidence = [
      "ATI 关键字 → fibocom",
      "AT 型号 fm350-gl 命中清单（fibocom / mediatek）",
      "USB 身份 0e8d:7127",
      "端口规则里没有这个 VID:PID",
      "VID 0e8d 在清单里只对应 fibocom"
    ]
    conflicts = []
```

识别结果与实机一致（`ATI` 原文 `Manufacturer: Fibocom Wireless Inc. / Model: FM350-GL`）。

### 7.3 未实机验证

- 端口规则命中路径（本机 PID 未登记）：只跑了单测（`2c7c:0801` → `rm520n-gl`）；
- 冲突路径：只跑了单测（构造 VID 与型号归属不一致）；
- `ignore_cclk_tz`：**未接入**，见 §3.1；
- 华为 / SIMCom / 高新兴 的温度、电压、工程信息指令：无设备，未验证。

---

## 8. 后续待办（按优先级）

1. ~~`ignore_cclk_tz`：实机核对~~ **已完成（1.0.0-10）**：FM350 实测 `+CCLK: 26/10/19,23:02:41+00`
   且与主机 UTC 同刻 —— 值是 UTC、后缀正确。据此确定接入方式：**默认按 3GPP 减 `±zz`**
   （`parse_cclk_tz`），怪癖表标记的违规固件（值是 UTC、后缀却报 +32）才跳过；
   本机 FM350 两种路径结果一致，行为零变化。
2. 华为温度/电压解析器（`AT^TEMP` / `AT^HVSST`），需一台 MH5000 系列实机；
3. `AT+SWITCH_SLOT` 双卡槽功能（整条功能缺失，不只是指令）；
4. 多模组同时接入的识别测试（当前只有单台设备）；
5. `settings.urc` 的 CNMI 下发策略：先确认哪些型号真的收不到短信 URC。
