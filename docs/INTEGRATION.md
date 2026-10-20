# 集成与架构说明

> 描述**当前代码**的分层、数据流与扩展点。与实现不一致时以代码为准，并请同步本文。
> 最后核对：2026-10-20（`main`）。

## 1. 仓库结构

| 目录 | 内容 |
| --- | --- |
| `rust/` | Rust 后端（`modemd` 库 + 二进制） |
| `files/` | 装进固件的运行时文件：init 脚本、netifd 协议、`mp-client.sh`、默认配置 |
| `luci/` | LuCI 应用与 proto 配置页 |
| `packaging/` | 打包辅助（control/postinst 等） |
| `docs/` | 本文档集（索引见 `docs/README.md`） |
| `scripts/` | 仓库校验脚本（如脱敏检查） |
| `Makefile` / `VERSION` | OpenWrt 包定义与唯一版本源 |

## 2. 后端分层

```
http/             HTTP + WebSocket（路由、鉴权、handlers、静态页、推送）
  └─ 调用 modem/ 与 netprov/，自身不碰串口、不改网络
runtime/          运行期：拨号协调器（dialer/，全仓唯一连接决策点）、
                  主机网卡台账（hostnic）、巡检（scheduler）、存储（store）、
                  NTP/时区、流量统计
  └─ dialer/native 内置 QMI/MBIM 链路（qmi_link / mbim_link）
netprov/          自动组网：纯策略（provision/route/v6）+ UCI 薄 IO（uci）+ 编排（mod）
modem/            模组域
  ├─ at/          AT 收发：串口后端、命令终结符、URC 过滤、节奏与优先级调度
  ├─ discovery/   发现：拓扑快照 → 端口计划 → 身份级探活 → 否定缓存
  ├─ model/       领域对象：Modem、拨号状态机、参数、小区、短信、锁网、任务…
  ├─ vendor/      厂商能力（一厂商一文件）+ 数据通道配方（datacall）
  └─ catalog/ports/quirks/bands/variant/atplan/…  数据表与匹配规则
config/           配置分节（server/serial/sms/sim/net/dial/modem/ui/wechat…）
ubus/             与 rpcd 的会话交互（transport/rpc/session 分离）
wechat/           企业微信推送（纯逻辑 + 传输 + 运行期状态）
```

**边界规则**：

- `http/` 不做业务判定，只做参数校验、调用与序列化；
- `netprov/` 不直接发 AT（地址/路由来自模组上报或设备响应，由调用方传入）；
- `runtime/dialer` 只管数据会话，不碰 netifd 配置；
- `modem/` 不依赖 `http/`，可被后台循环独立调用。

## 3. 一次「插上模组 → 能上网」的数据流

```
① 巡检（默认 3s）
   manager::discover
     ├─ auto_bind_option：端口规则 option_driver=1 且无 tty → 写 option1/new_id
     ├─ auto_bind_pcie：未绑定驱动的 PCIe 模组 → mhi_pci_generic / mtk_t7xx
     ├─ reap_gone：设备文件消失的模组摘掉（否则重插再也发现不了）
     ├─ discovery::snapshot  读 sysfs：usb_dev / iface / driver / vid:pid
     ├─ discovery::plan      分组 + 白名单 + PCM 剔除 + 排序（纯函数）
     └─ scanner::discover_new 身份级探活（AT → CGMM/CGMI/ATI 有内容行）
   每台物理设备只登记一个 Modem（命令口首选 + 回退口）

② 识别（Modem::identify，只读）
   ATI（失败回退 CGMM/CGMR）+ USB 身份 → detect::identify
   → 厂商 / 型号 / 置信度 / 证据 / 冲突（见 detect.rs 的优先级表）

③ AT 访问
   所有命令走 AtPort + Scheduler（User > Signal > Data > Bulk 优先级 + 25ms 节奏）
   串口打不开有冷却闸；URC 由 LineGate 过滤，不污染命令应答

④ 拨号（/api/dial/connect，或任务 dial.connect）
   dial_inner
     ├─ 取连接代次（Modem::dial_epoch，bump 后作为本次意图）
     ├─ 校验 cid / APN / PDP 类型
     ├─ usb_type：AT 方言 → USB 描述符 → 主机驱动（30s 缓存）
     ├─ vendor::datacall 取配方（厂商 × 平台）
     ├─ QMI/MBIM：交办拨号协调器（modem 侧绝不 AT+CGACT=1），state=dialing
     └─ ECM/RNDIS/NCM：按配方发私有激活指令，失败回退 CGACT
   每次「宣布成功」前都比对代次：用户中途点断开 / 新拨号接手 ⇒ 本次结果作废
   状态机 phase：session → iface_pending → route_pending → online

⑤ 组网（netprov，15s 一轮，启动先跑一轮再进节拍）
   选本模组 USB 设备下的网卡 → 写 UCI（network.modem<id>）→ ifup
   地址：DHCP / FM350 静态（模组上报地址）/ v6 兄弟接口
   地址、路由、DNS 落地后回写拨号状态

⑤b 拨号协调器 reconcile（同一轮，无条件跑，不受 auto_wan / uci 影响）
   这两个开关管「建不建接口」，协调器管「谁拨号、什么时候重拨」
   每台模组二选一，**决策只在这里做一次**：
     ├─ 原生（QMI/MBIM）
     │   is_running → native_takes（ensure_dial_kind + 厂商策略 supports）
     │   → APN 三级回退（拨号状态 → last_dial_apn → 模组已配置上下文）
     │   → pick_device（同 UCI 口径选网卡，多模组不串）
     │   → plan_native：控制口 sysfs 反查（usbmisc/usb/GobiQMI/mhi 四级，
     │     绝不猜 /dev/cdc-wdm0）+ qmap_mode 三路径 + QMAP 参数
     │   会话探活：QMI Get Packet Service Status / MBIM ActivationState
     │   建会话前记代次、建好后比对（过期立即拆）；成功后回调
     │   Modem::note_native_up 写 connected（这是早先缺失的一环）
     └─ AT（ECM/RNDIS/NCM/unknown）
         auto_connect_allowed（唯一判定：dial_enabled / auto_dial /
         manual_stop / connected / dialing+busy / 退避）→ 通过才派工作线程
         跑 Modem::dial（不阻塞组网循环），结果按状态分类记退避
   统一退避表（每模组一条）：AT 30/60/120/240/300s，Native 5/10/20/40/60s；
   同一错误持续则继续退避，错误变化才清零；用户重开开关时清空（立即重试）

⑥ 断开（/api/dial/disconnect）
   先停原生会话 → 模组侧按配方挂断（hangup 首步 bump 代次，作废在途拨号）
   → 清该网卡旧默认路由与地址 → manual_stop=true（协调器不再自动重拨）
```

## 4. 数据表与匹配规则

| 文件 | 内容 | 维护方式 |
| --- | --- | --- |
| `modem/modem_catalog.json` | 91 型号：厂商/平台/USB 与 PCIe 形态/切模式指令/VID | 与 QModem 支持清单对齐；新增型号按同结构追加 |
| `modem/modem_ports.json` | 18 条 `VID:PID` 端口规则（接口白名单、语音口、option 驱动） | 与 QModem `modem_port_rule.json` 逐条对齐 |
| `modem/modem_quirks.json` | 逐型号怪癖（解析差异、超时、只读限制） | 每条必须有实测依据 |
| `modem/modem_bands.json` | 型号 → 支持频段 | 与 catalog 型号集合一致（有测试守住） |
| `modem/variant.rs` | 厂商 × 平台差异（同一指令不同参数） | 只写有依据的分支 |

匹配口径：型号去分隔符大写后**长 token 优先**；≤4 字符要求词边界；ATI 文本先原样再连写匹配。
身份与能力分离：**认到型号 ≠ 所有协议可用**，能力一律查 `catalog` 的 `usb`/`pcie` 列表。

## 5. 扩展指南

### 5.1 新增一个型号

1. `modem_catalog.json` 追加一条（字段照现有条目：`vendor/model/platform/usb/pcie/usb_at/pcie_at/vid`）；
2. 若有频段数据，`modem_bands.json` 同步追加（型号集合必须一致，测试会校验）；
3. 若端口布局特殊，`modem_ports.json` 加 `VID:PID` 规则（`include` / `voice_pcm_interface` / `option_driver`）；
4. 若解析/超时有差异，`modem_quirks.json` 加一条并注明实测依据；
5. `cargo test --locked`（`datacall`、`catalog`、频段一致性测试会跑）。

### 5.2 新增一家厂商

1. `modem/vendor/<name>.rs` 实现 `VendorSpec`：`id/label/detect` 必填，其余按需覆盖；
2. `Vendor` 枚举、`DETECT_ORDER`、`spec()`、`from_key()`/`as_str()`/`label()` 各加一项；
3. 有私有数据通道指令时实现 `data_channel()`（激活与挂断必须成对）；
4. 有私有 AT 面时覆盖 `at_plan_commands()`；
5. `vendor/mod.rs` 的 `detects_each_vendor` 与 `datacall` 的表驱动测试补一条。

### 5.3 新增一条数据通道指令

1. 在对应厂商模块的 `data_channel()` 里按 `ctx.platform` 分支；
2. 激活与挂断**必须成对**（`activate_and_deactivate_are_symmetric` 会检查）；
3. 标注依据（QModem 脚本行号或手册章节）与超时；
4. 在 `datacall::tests::vendor_table_matches_qmodem_dial_script` 里加一行字面量断言。

### 5.4 新增接口

1. `http/router.rs` 的 `ROUTES` 加 `(方法, 路径)`，并在分发 `match` 里接 handler；
2. `http/handlers/<name>.rs` 用 `response::ok/fail` 统一信封；
3. 更新 `docs/API.md`（接口全表由路由导出，必须同步）；
4. 需要登录时确认 `auth::is_public_path` 没把它放进放行名单。

## 6. 配置分节

| 分节 | 关键项 |
| --- | --- |
| `[server]` | `listen`、`port`、`log_file`、`log_level`、`scan_interval`（默认 3s） |
| `[serial]` | `baud`、`at_probe_timeout`、`cmd_timeout`、`usb_autobind`、`pcie_autobind`、`auto_rescue` |
| `[net]` | `auto_wan`、`wan_mode`、`wan_dns`、`wan_ipv6`、`wan_ipv6_mode`、`host_dial_pdn` |
| `[dial]` | 拨号总开关、自动拨号、默认 cid/APN 行为 |
| `[sim]` / `[sms]` | SIM 热插拔、短信存储区与密码闸门 |
| `[ui]` | `require_luci_login`（默认开） |
| `[quota]` / `[ntp]` / `[wechat]` | 流量配额、校时、企业微信推送 |

缺省值由 `config/` 各分节 `normalize()` 补齐；配置读不到时以**收紧后的默认值**启动
（登录闸门、短信闸门不会因为配置损坏而敞开）。

## 7. 兼容性红线

- HTTP 路径/方法/字段、WebSocket 消息、`code` 语义；
- 配置文件键与默认值、conffiles 保留行为；
- netifd 协议名 `modem_pro` 与 `mp-client.sh` 的调用约定；
- 初始化/升级不得覆盖用户配置；
- API 的 `modem_id` 语义：**给了但不存在必须报错**，不回落（避免写操作打到别的模组）。

## 8. 与上游的关系

- 知识底座（型号清单、端口规则、厂商 AT 方言）来自 QModem，按 MPL-2.0 使用并在文件头注明；
- 发现/串口策略/状态机参考 Go 版 modem-pro（MIT），但数据面改为内置 Rust 拨号器；
- 逐项迁移结论、取舍理由与未完成项见 `docs/UPSTREAM_AUDIT.md`；
- 逐型号支持矩阵与验证状态见 `docs/SUPPORT_MATRIX.md`。
