# modemd HTTP 接口契约（API）

服务默认监听 `0.0.0.0:9527`（`[server] listen`）。

## 通用约定

所有 `/api/*` 返回统一 JSON：

```json
{ "code": 0, "data": <any>, "msg": "ok" }
```

- `code == 0` 成功，非 0 失败（`data` 为 `null`，`msg` 为原因）。
- 业务错误默认仍返回 HTTP 200；鉴权失败返回 HTTP 401；未知接口返回 HTTP 404。
- 响应带 `Cache-Control: no-store`。

### 鉴权

- 由 `[ui] require_luci_login` 控制（默认开）。
- **免登录**：静态页、`/api/version`、`/api/session`、`/api/login`、`/api/logout`，以及来自
  本机（`127.0.0.1`）的请求。
- 其余 `/api/*` 需带 Cookie `modemd_sess=<会话ID>`，有效性经 ubus `session.get` 校验。
- ubus（rpcd）不可用时**放行并记 warn**，避免把整机用户挡在门外。

---

## 错误契约（handler 失败时也必须有响应）

所有接口（含失败路径）都返回同一个信封：

```json
{ "code": 0 | 非0, "data": <any>, "msg": "<人类可读的原因>" }
```

- `code == 0` 表示成功；非 0 表示失败，**HTTP 状态码通常仍是 200**（与前端既有约定一致）。
- 例外：鉴权失败用 **401**、未注册路径用 **404**，且都带信封。
- ⚠️ **实现约束**：`tiny_http::Request` 是一次性的。handler 一旦带着 `Err` 返回，
  `Request` 已被 drop，网关层再也写不了任何字节 —— 连接上会是空的，页面只看到
  「modemd 无响应」。因此 handler 内部**不得**用裸 `?` 传播错误，必须自己
  `response::fail(...)`（或 `response::to_value` / `json_body` 这两个先序列化再响应的助手）。
- 自测方法：对任意接口发一个必然失败的请求（如 `?modem_id=abc`），
  应拿到带 `msg` 的信封而不是空回复。

## 串口访问调度（优先级 + 限流）

一台模组只有一个 AT 口，而本项目有**十几个**并发访问者：后台信号采样、网络巡检、
流量积分、SIM 巡检、URC 回收，加上页面轮询（概览 3 秒一轮、拨号页 5 秒一轮）
和用户手动操作。全部挤一个串口，必须排队。规则在 `rust/src/modem/at/sched.rs`：

| 级别 | 谁在用 | 为什么是这个级别 |
| --- | --- | --- |
| `user` | `/api/at` 手动发指令 | **实时**：用户点一下就该立刻回，不能排在后台采样后面 |
| `signal` | 信号 / 注册态 / SIM 身份巡检 | 页面最常看的那几个数，卡了就是「数据不动」 |
| `data` | 拨号取 IP、载波聚合、网络状态、模组状态 | 功能正确性依赖 |
| `bulk` | 短信列表、锁网状态、小区扫描、参数读写、USSD、扫网… | 冷门且耗时，**尽量走缓存** |

同级内部按先来后到（ticket 序号），低优先级不会被饿死 —— 只要没有更高级的
等待者，它就轮到。

### 三道保护（各管一件事，互不重叠）

| 层 | 模块 | 管什么 | 默认 |
| --- | --- | --- | --- |
| **优先级排队** | `at/sched.rs` | 谁先用串口；同级 FIFO 不饿死 | `user` > `signal` > `data` > `bulk` |
| **窗口配额** | `at/sched.rs` | 每 10 秒最多几次「占用」（`user` 级不受限） | 120 次 / 10 秒 |
| **命令节奏** | `at/pacing.rs` | **每一条 AT 命令之间**硬性延时 | **30 ms** |
| **打开冷却** | `at/cooldown.rs` | 串口打不开时别狂重试 | 失败后 5 秒内不再 open |

#### 命令节奏（`at/pacing.rs`）—— 最容易被忽略的一层

早先只在「两次占用串口之间」限流，但**一次逻辑操作里往往连发好几条**：

- 读参数：`AT+CGDCONT?` → `AT+CGACT?` → `AT+COPS?`
- 读短信：`AT+CMGF=1` → `AT+CMGL="ALL"` → `AT+CMGF=0` → `AT+CMGL=4` → `AT+CMGF=1`

这几条中间原本**零间隔**，等于把一串 AT 一口气打给模块。串口是半双工，模组固件
要逐条解析，实测后果是应答被截断（只收到半个 `+CSQ: 22,99`）、偶发 `ERROR`、
严重时 AT 口假死到要重启模组。

现在延时下沉到 `AtPort::command_cancellable`：**每条命令发送前**都检查
「距上一条写完是否够 30 ms」，不够就睡够。所以不管一次操作里连发几条，
模组收到的命令之间都有 30 ms。

> 30 ms 不是拍的：同作者上游 `luci-app-fm350` 的 `MIN_CMD_GAP = 30ms` 是
> 同一 FM350 平台长时间实机跑出来的（20ms 零星丢 `OK`，30ms 稳定）。
>
> 语义是「**两次发令时刻之差**」：模组应答本身就慢的命令（`AT+CGDCONT?` 要
> 120 ms）自然满足间隔，不会被额外拖慢；只有命令排得很密时才真正生效。

**怎么判断它生没生效**（重要，别误读 `pacing.throttled = 0`）：

| 观察值 | 含义 |
| --- | --- |
| `cmd_gap_ms = 30` 且 `pacing.throttled = 0` | **正常**。30ms 是**下限**保护：这台模组每条命令（写+读应答）本来就要 60~80 ms，天然满足间隔，所以一次都没触发额外等待 |
| `pacing.throttled` 持续增长 | 真的出现了「命令排太密」—— 说明某个操作在连发快命令，这层正在替它减速（该去看看是谁） |
| `pacing.waited_ms` 突然很大 | 有操作连发了十几条快命令（比如翻很长的短信列表），累计等了上百毫秒 |

实机数据（FM350-GL，6 条并发手动 `AT`）：`user.served=6`、`pacing.throttled=0`
—— 6 条全部执行，没有一条需要额外等 30ms，因为每条 `AT` 往返就约 80ms。
**这恰恰是期望的行为**：保护在位，但不白白拖慢。

#### 打开冷却（`at/cooldown.rs`）

串口打不开通常持续几十秒（模组重启、USB 重新枚举、驱动没 bind），而 6~8 个后台
循环会同时来问。没有冷却就是「每 200 ms 一次 `open` 全失败」：日志刷屏、
USB 子系统被拖慢（实机见过模组重启后认到时间从 40 秒拉到 90 秒）、CPU 空转。

失败后 5 秒内**直接返回带倒计时的错误**（`AT 口打开失败，4 秒后重试`），
不碰串口。注意这不是「假装成功」—— 页面据此显示「模组未就绪」，
而不是「流量 0 / 信号 0」那种会被误读成「模组坏了」的默认值。

**冷门查询走 TTL 缓存**（`rust/src/modem/at/cache.rs`），命中就不发 AT：

| 数据 | TTL | 写后作废的入口 |
| --- | --- | --- |
| PDP / 选网参数（`/api/params`） | 15 秒 | `set_apn` / `delete_apn` / `set_pdp` / `set_netselect` |
| USB 网卡模式（`/api/usbnet`） | 30 秒 | `set_usbnet` |
| 组网模式偏好（`/api/mode`） | 30 秒 | `set_mode_pref` |

三条纪律（踩过坑）：**失败不缓存**（一次串口超时不能把错误钉住整个 TTL）、
**写后必作废**（改了 APN 还读旧值比慢更糟）、**要能强制刷新**
（`?refresh=1` 直接绕过缓存）。拨号流程取 APN 一律走实时版本
（`params_refresh`），因为要拿最新配置去拨。

想看当前排队情况：`GET /api/at/sched`（见下文）。工具页 AT 调试台的标题右侧
有个徽章实时显示「空闲 / 串口中 / 排队 N / 限流 N」。

## 接口

### GET /api/snapshot?modem_id=N
页面首屏快照：**一条 AT 都不发**，全部来自缓存（后台采样 / 同步接口 / 异步任务的最近一次结果）。

```json
{"code":0,"data":{
  "ts":"2026-10-19T07:22:38Z","modem_id":0,
  "modems":[{"id":0,"model":"FM350-GL","vendor":"fibocom",…}],
  "blocks":{"signal":{…},"network":{…},"cells":{…},"lock":{…}},
  "ages":{"signal":1933,"network":null,"cells":null,"lock":null,…},
  "stale":{"signal":false,"network":true,…},
  "dial":{"state":{"state":"idle","state_text":"未拨号",…}},
  "tasks":[{"id":"t1","kind":"signal.refresh","state":"running",…}]
},"msg":"ok"}
```

- `ages`：每块数据的年龄（毫秒）；`null` = 从没采过。`stale` 超过阈值（信号 30s / 网络 60s / 小区与锁网 30min）。
- 页面契约：**先渲染旧值 + 年龄**，需要新值就提交任务（见下），任务完成后重拉快照。

### GET /api/tasks、POST /api/tasks、GET /api/tasks/get、POST /api/tasks/cancel
异步任务：慢操作（扫小区 / 扫网 / AT / USSD / 发短信 / 锁频段 / 切 USB 模式 / 重启模组 /
刷新信号与网络）**提交后立即返回**，不占 HTTP 连接也不占串口队列。

```text
POST /api/tasks        {kind, modem_id, ...参数}  → {task:{id:"t1",state:"queued"}}
GET  /api/tasks/get?id=t1                         → {task:{state:"done", result:{…}, elapsed_ms:…}}
GET  /api/tasks?modem_id=0[&active=1]             → {tasks:[…], count}
POST /api/tasks/cancel {id:"t1"}                  → {id, cancelled}
```

- `state ∈ queued|running|done|failed|cancelled`；`progress` 是给页面看的一句话。
- **去重**：同 `kind + 模组 + 参数` 只允许一个在跑（扫网被点两次不会排两条）。
- **取消是协作式**：AT 读取循环每 100ms 检查一次（`command_cancellable`），扫网能被打断；
  不检查的闭包会跑完但结果标成 cancelled。
- `result` 与同名同步接口的 `data` 同形，前端可复用渲染。
- 任务种类：`signal.refresh` / `network.refresh` / `cells.scan` / `lock.status` / `at` /
  `ussd.send` / `sms.send` / `usbnet.set` / `lock.band` / `netscan` / `reboot`。

> 同步接口（`/api/cells`、`/api/netscan` 等）仍然保留：调试台与脚本用；
> **页面应当全部走任务**，见 `docs/PORTING.md` 的异步化口径。

### GET /api/version
免登录。返回版本与运行时信息。

```json
{"code":0,"data":{"version":"1.0.0-1","runtime":"rustc stable","platform":"linux/mips","luci_auth":true},"msg":"ok"}
```

### GET /api/modems[?discover=1]
模组列表。`discover=1` 前先做一次重扫。

```json
{"code":0,"data":[{"id":0,"at_port":"/dev/ttyUSB2","tty_list":["/dev/ttyUSB2"],"vendor":"quectel","vendor_name":"移远 Quectel","model":"RM500Q-GL","model_raw":"ATI | Quectel | RM500Q-GL | Revision: ... | OK","at_port_manual":false,"running":true}],"msg":"ok"}
```

（简化模型：一个 AT 口 = 一个模组，`tty_list` 就是当前口自己；`model_raw` 为识别时的 ATI 原文，未识别时不出现该字段。）

### GET /api/signal?modem_id=N
信号（`AT+CSQ` + 服务小区 / 扩展信号）。

```json
{"code":0,"data":{"rssi":6,"ber":99,"nr_rsrp":-116,"nr_rsrq":-14,"nr_sinr":-3},"msg":"ok"}
```

取值口径（`modem/model/signal.rs`）：

- `rssi` / `ber`：`AT+CSQ` 原值（`rssi` 是 0..31 索引，99/255 = 未上报）。
- `nr_rsrp` / `nr_rsrq` / `nr_sinr`：移远 / 广和通高通走 `AT+QENG="servingcell"`，
  取不到退回 `AT+CESQ`。**广和通 MTK 平台（FM350 / RW350）没有 QENG**，改读
  `AT+GTCCINFO?` 的服务小区行（同一份行表也喂 `GET /api/cells`）。
- 三值都是**阶梯索引换算**出来的物理量，统一取 3GPP 区间下界（索引 1 =
  [-140,-139) dBm）；越界 / 哨兵值（255）省略字段，不显示假数。
- **LTE 制式下 `AT+CESQ` 不带 SINR 格**：广和通 MTK 的 SINR 取自 GTCCINFO
  服务小区第 10 列 `<rssnr_value>`，该值本身就是 dB（-100 ~ 100）。

> 温度不在这里：广和通 MTK 平台的各路传感器温度在 `GET /api/modem/status` 的
> `temps` / `temp_key` / `temp_max`（`AT+GTSENRDTEMP=0`，量纲 0.001 ℃）。

### GET /api/network?modem_id=N
网络注册 / 运营商（`AT+COPS?`）。

```json
{"code":0,"data":{"operator":"CHN-UNICOM","act":"7","band":3,"band_label":"B3","tech":"FDD LTE","raw":"..."},"msg":"ok"}
```

`band_label` 是频段的**人可读名**（`n78` / `B3` / `B1`）。部分固件的 `<band>` 字段给的是
编码值而不是频段号，`modem/model/band.rs` 负责反解；NR 制式下 `<band>` 实测为空的机型
靠 ARFCN 反推。解不出时字段为 `null`，不猜。

### GET /api/network/status?modem_id=N
网络注册 + **当前 APN / 本机地址**（`AT+QNWINFO` + `AT+CREG?`/`AT+CGREG?` + `AT+CGCONTRDP`）。
周期推送的 `network` 事件就是这份数据。

```json
{"code":0,"data":{
  "modem_id":0,"queried_at":"2026-10-18T10:10:34Z",
  "net_tech":"FDD LTE","tech":"lte","tech_label":"LTE",
  "plmn":"46011","mcc":"460","mnc":"11","operator":"中国电信",
  "band":3,"band_label":"B3","band_text":"LTE BAND 3","duplex":"FDD","earfcn":1300,
  "raw_net_info":"+QNWINFO: \"FDD LTE\",\"46011\",\"LTE BAND 3\",1300",
  "apn":"ctnet.MNC011.MCC460.GPRS",
  "ipv4":"203.0.113.53","ipv6":"2001:db8::1",
  "cs_reg":{…},"ps_reg":{…},"reg_5g":{…},"reg_eps":{…},
  "serving":{…},"serving_count":1,"neighbour_count":3,"scanned_at":"2026-10-18T10:10:00Z",
  "ca":[],"ca_active":false,
  "roaming":false,"roam_allowed":true,"roam_scope":"none",
  "airplane":false,"cfun":1,"traffic":{…}
},"msg":"ok"}
```

| 字段 | 说明 |
| --- | --- |
| `duplex` | 从接入技术串拆出的 `FDD` / `TDD`；认不出就省略 |
| `reg_5g` / `reg_eps` | 5GS / EPS 注册状态；**只在 CS/PS 都报未注册时补查**（5G SA 下 CREG/CGREG 会长期停在 0,0） |
| `serving` / `serving_count` / `neighbour_count` / `scanned_at` | 服务小区与数量，取自**基站扫描缓存**（没扫过就没有 `scanned_at`，不触发扫描） |
| `ca` / `ca_active` | 分量载波（实时查询，见 `model/ca.rs`）。`ca_active` 只在**已激活**的辅载波存在时为真；模组不支持 CA 指令或本轮未上报时为 `[]` / `false` |
| `roam_allowed` / `roam_scope` | 「数据漫游」开关与范围（`none` / `domestic` / `global` / `unknown`） |
| `airplane` / `cfun` | 飞行模式与 `AT+CFUN?` 原值（1 = 全功能，4 = 飞行） |
| `raw_net_info` | QNWINFO 原文（去掉命令回显，多行用 ` | ` 连接），页面排查框直接显示 |

地址字段的归一化口径（`modem/model/addr.rs`，与 `AT+CGPADDR` 那条路**同一套判据**）：

- `ipv4`：四段 0~255、拒全零；解不出 → 字段省略。
- `ipv6`：标准冒号形式做**结构校验**（拦 `gg::1`、两个 `::`、组数不对、组过长），
  剥 zone id（`%eth2`）与方括号；冒号写法**原样返回，不重新压缩**。
- **16 段点分十进制**按 IPv6 解（广和通 / 移远部分固件把地址按字节展开）：
  `32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.1` → `2001:db8::1`。
- **占位与不可信地址一律不上报**：全零、`::1`、以及「只报接口标识」（前 8 字节全 0）。
  实测广和通 MTK 平台在 IPv6 未分配时就回点分的 `::1` 占位。

> **地址字段的下标不能写死**：3GPP 规定 `+CGCONTRDP` 的 `<ip_addr>` 在第 3 位，但实测
> 广和通 MTK 平台把它放到了第 5 位（第 3、4 位是空串）。`parse_active_context` 因此
> 从第 3 位起扫第一个含地址的字段，并把紧邻的下一字段也看一眼（有的固件把 v4 / v6
> 拆成相邻两列）；再往后是 DNS，不再当地址收。

#### 制式 / 运营商字段的取值口径

- **`AT+QNWINFO` 不可用时走 `AT+COPS?` 兜底**（`merge_network`）。广和通 MTK 机型
  （FM350 / RW350）对 `AT+QNWINFO` 与 `AT+CSPPLMN?` 都回 `ERROR`，制式 / PLMN 全空；
  此时才多发一条 3GPP 通用的 `AT+COPS?`。**该指令可用的厂商不增加这一次往返**。
- 兜底只能补 `plmn` / `mcc` / `mnc` / `tech` / `tech_label` / `operator`：
  `band` / `band_text` / `earfcn` / `net_tech` 没有通用替代，**保持为空，不造数据**。
- `tech` 由 `<AcT>` 按 3GPP TS 27.007 映射（`7`=LTE、`11`=NG-RAN、`2/4/5/6`=WCDMA、
  `0/1/3`=GSM）。⚠️ FM350 AT 手册 v2.2 那张表早于 NG-RAN 入表（它把 11/12 写成
  eMTC/NB-IoT），**别照它写**；实机注册在 NR n78 时 `<AcT>` 就是 `11`。
  已知限制：NR NSA 下模组常以 LTE 注册，`<AcT>` 报 `7`，字段照报、不猜。
- `operator` 三级取值：内置 MCC/MNC 表（`model/cells.rs`，四家中国运营商全名）→
  模组上报名（`AT+COPS?` format=0 才有）→ 原始 PLMN。

### GET /api/modules
支持模块清单（91 个型号，数据源自 QModem `support_list.md`）与逐型号 AT 指令面。

三种用法：

| 请求 | 返回 |
| --- | --- |
| `/api/modules` | `{count, usb_count, pcie_count, vendor_count, vendors, models}` |
| `/api/modules?model=RM500Q-GL` | `{plan}`：按型号离线推导（`in_catalog` 标出是否在清单里） |
| `/api/modules?modem_id=0` | `{plan}`：按在网模组的实测型号，`live=true` |

```json
{"code":0,"data":{
  "count":91,"usb_count":90,"pcie_count":41,"vendor_count":12,
  "vendors":[{"vendor":"quectel","name":"移远 Quectel","count":30}],
  "models":[{"vendor":"fibocom","vendor_name":"广和通 Fibocom","model":"fm350-gl",
             "platform":"mediatek","usb_modes":["rndis"],"pcie_modes":["mbim"],
             "usb_text":"RNDIS","pcie_text":"MBIM"}]
},"msg":"ok"}
```

`plan` 关键字段：`model / vendor / vendor_name / platform / platform_name /
platform_variant / platform_variant_note / in_catalog / live /
usb_modes / pcie_modes / usb_text / pcie_text / cmd_impl / cmd_impl_note /
lock_dialect / usb_dialect / mode_dialect / imei_dialect /
bands / bands_note / commands[] / gaps[] / gap_notes[]`。

`commands` 每条形如
`{key,label,cmd,candidates?,enable_cmd?,parser,format,on,source,note?}`，
来源分三类：3GPP 通用、厂商模块（`modem/vendor/<厂商>.rs`）、QModem 型号清单的
USB/PCIe 切换指令（按型号精确）。没实现的模式/能力会进 `gaps` + `gap_notes`，**不编造**；
型号不在清单里时 `in_catalog=false`，仍会给该厂商的 3GPP/厂商指令，但不给型号级切换指令。

### GET|POST /api/at
直发 AT。参数 `modem_id`、`cmd`（POST 可用 form 或单层 JSON；form 值按百分号解码）。

契约：
- 只允许**单条**指令：空 cmd → `cmd required`；含 `\r`/`\n` 或超过 128 字符 → `cmd must be a single line (<=128 chars)`；非 POST → `method not allowed, use POST`；
- 成功/失败都回 `data:{cmd, resp}` —— `resp` 是**应答行数组**（已去 `\r\n` 与空行），前端按行渲染；失败时可能是半截的。

```
POST /api/at     {"modem_id":0,"cmd":"AT+CGSN"}
```

```json
{"code":0,"data":{"cmd":"AT+CGSN","resp":["123456789012345","OK"]},"msg":"ok"}
```

发送失败（HTTP 状态仍是 200，业务 code -1）：

```json
{"code":-1,"data":{"cmd":"AT+CGSN","resp":[]},"msg":"串口 /dev/ttyUSB2 无应答（超时）"}
```

### POST /api/at/port
手动指定这台模组的 AT 命令口（跨重启记住，台账 `atport.json`，与 config.toml 同目录）。参数 `modem_id`、`port`。

- 键用**模组身份**（`imei:` 优先，其次 `model:`；未识别时用当前口自己兜底），**不是端口号** —— 端口号正是要换的东西；
- 目标口必须是本机真实存在的串口；与当前口不同时先探一次 AT —— 已识别模组探不过即拒，未识别模组放行（现状本来就是坏的，硬拒 = 永远修不回来）；
- `port` 传空串 = 恢复自动探测（在候选口里挑一个能答 AT 的）。

```json
{"code":0,"data":{"modem_id":0,"port":"/dev/ttyUSB3","changed":true,"key":"imei:867698040012349"},"msg":"已切到 /dev/ttyUSB3（已记住，重启后仍用它）"}
```

### GET /api/at/presets[?modem_id=N]
本模组的常用 AT 指令清单（按厂商渲染，私有表同名覆盖通用项）。取不到模组也回 **code 0** + 兜底清单（`data.modem_id=-1`、`vendor:"unknown"`、纯 3GPP 清单），界面照样能用，`msg` 说明原因。

```json
{"code":0,"data":{"modem_id":0,"vendor":"quectel","vendor_name":"移远 Quectel","presets":[{"group":"查询","cmd":"AT+CGSN","desc":"IMEI"},{"group":"设置（谨慎）","cmd":"AT+QNWPREFCFG=\"policy_band\",\"<lte_band>\"","desc":"LTE 频段（策略表）","tpl":true}]},"msg":"ok"}
```

`tpl:true` = 带 `<…>` 占位符的模板，选中后必须先替换再发。清单规模：通用 3GPP 28 条；移远 18 / 广和通 18 / 芯讯通 6 / 美格 7 / 鼎桥 10 / 展锐 38（同名覆盖、展锐另抑制通用 `AT+CGCONTRDP`）。广和通 MTK 平台（FM350 / RW350 系列）额外带三条：`AT+GTSENRDTEMP=0`（各路温度，量纲 0.001 ℃）、`AT+EMBIND?` / `AT+EMBIND=?`（USB 数据通道绑定体检；读形式在该平台回 `phone failure`，勿用 `AT+GTEMP?`）。

### GET /api/at/sched[?modem_id=N]
串口访问调度的实时状态（排障用）。口径见本文「串口访问调度」一节。

```json
{"code":0,"data":{"modem_id":0,
  "limits":{"gap_ms":0,"window_ms":10000,"per_window":40},
  "cmd_gap_ms":30,"pacing":{"throttled":18,"waited_ms":540},
  "open":{"fails":0,"skipped":0,"cooldown_ms":5000},
  "waiting":0,"busy":false,"max_wait_ms":312,
  "user":{"served":3,"wait_ms":12,"throttled":0},
  "signal":{"served":120,"wait_ms":860,"throttled":4},
  "data":{"served":30,"wait_ms":90,"throttled":0},
  "bulk":{"served":5,"wait_ms":1200,"throttled":2}},"msg":"ok"}
```

| 字段 | 含义 |
| --- | --- |
| `limits.gap_ms` | 两条命令之间的最小间隔 |
| `limits.per_window` / `window_ms` | 配额：每 `window_ms` 毫秒最多 `per_window` 次。**`user` 级不受配额限制**（配额防的是程序失控，不是限制人） |
| `limits.gap_ms` | 两次**占用**之间的兜底间隔（默认 0；命令级间隔见 `cmd_gap_ms`） |
| `cmd_gap_ms` | **每条 AT 命令之间**的硬性间隔（默认 30）。`null` = 此刻拿不到 AT 口（正在发命令），不是 0 |
| `pacing.throttled` / `pacing.waited_ms` | 被命令节奏拦下的次数 / 累计等待毫秒 |
| `open.fails` / `open.skipped` | 打开失败次数 / 因冷却被直接拒掉的次数 |
| `open.cooldown_ms` | 失败后的冷却时长（默认 5000） |
| `waiting` | 此刻排队等串口的线程数。**`18446744073709551615`（`usize::MAX`）= 调度器正卡在限流 sleep 里、拿不到快照**，不是真有那么多人在等 |
| `busy` | 串口当前是否被占着 |
| `max_wait_ms` | 历史最长单次等待 |
| `*.served` / `*.wait_ms` / `*.throttled` | 各优先级：被服务次数 / 累计等待毫秒 / 被闸门拦下次数 |

排障对照：

| 现象 | 说明 |
| --- | --- |
| `bulk.served` 暴涨、`signal.wait_ms` 很大 | 冷门命令在挤信号 → 该加缓存或降级（见 `at/cache.rs`） |
| `throttled` 持续增长 | 闸门在工作（**好事**，说明它挡住了灌爆）；若用户觉得慢，调 `Limits` |
| `waiting > 2` 且 `max_wait_ms` 很大 | 有慢命令占着串口（扫网 / USSD / 短信列表） |
| `busy: true` 长期不变 | 卡在某条命令上，去看日志里的超时 |

### GET /api/logs[?lines=N]
日志尾部，默认 200 行。

```json
{"code":0,"data":{"file":"/var/log/modemd.log","count":200,
  "lines":["2026-10-18T00:00:00Z INFO ...","..."]},"msg":"ok"}
```

> ⚠️ 字段是 `lines[]`（数组），不是早期的 `log`（整串）。
> 前端按 `lines.join("\n")` 渲染；`lines` 为空数组表示「还没写过日志」，
> 不等于出错 —— 出错时 `code != 0`。

### GET /api/session
当前是否已登录，以及登录闸门是否开着、LuCI 入口在哪。

```json
{"code":0,"data":{"logged_in":false,"required":true,"login_url":"/cgi-bin/luci/"},"msg":"ok"}
```

> 未登录调其它接口时返回 HTTP 401 + `{"need_login":true,"login_url":"…"}`，
> 页面据此弹登录条并把「去 LuCI」按钮指到 `login_url`。

### POST /api/login
登录（转发 ubus `session.login`）。参数 `username`（默认 `root`）、`password`。
成功下发 `Set-Cookie: modemd_sess=<sid>; Path=/; HttpOnly; SameSite=Lax`。

```json
{"code":0,"data":{"sid":"..."},"msg":"ok"}
```

### POST /api/logout
清除 cookie（`Max-Age=0`）。

```json
{"code":0,"data":null,"msg":"ok"}
```

### POST /api/params/apn
写 APN（回读确认 + 自动重新激活 + 顺手激活 IMS），返回新的参数总览，步骤写在 `msg` 里。
请求体 `{modem_id, cid, pdp_type, apn}`。

### POST /api/params/apn/delete
删除一条 APN 定义（`{modem_id, cid}`，回读确认）。

### POST /api/params/pdp
激活 / 停用 PDP（`{modem_id, cid, active, reconnect}`；`reconnect` = 先断后连）。

### POST /api/params/netselect
选网方式（`{modem_id, mode, plmn}`，`auto` / `manual`），手动需 5~6 位数字 PLMN。

### POST /api/mode/set
下发组网模式偏好（`{modem_id, value}`），写完回读确认。

### GET /api/usbnet?modem_id=N
USB 网卡模式（当前值 + 可选档位 + 每档的切换指令 + 不可执行原因）。

```json
{"code":0,"data":{
  "value":"3","name":"RNDIS","supported":true,"need_reboot":true,
  "modes":[{"value":"0","name":"RMNET"},{"value":"1","name":"ECM"},…],
  "mode_cmds":{"0":"AT+QCFG=\"usbnet\",0","1":"AT+QCFG=\"usbnet\",1"},
  "blocked_modes":{"3":"广和通 RNDIS 不允许在这里执行（切过去会整机掉 USB）"},
  "cmd":"AT+QCFG=\"usbnet\"",
  "note":"修改后需重启模组生效","raw":"+QCFG: \"usbnet\",3"
},"msg":"ok"}
```

- 取值一律**字符串**（与页面 `<option value>` 直接对齐，不会出现 `"3" !== 3`）。
- `value=""` = 本机固件没有查询指令（例如广和通），此时页面把下拉置灰、
  只列出该厂商支持的档位，并把原因写在 `note`。
- `blocked_modes` 里的是「只展示、不允许执行」的档位：后端会拒（`/api/usbnet/set`），
  页面提前置灰并写明原因（不能从列表里删掉 —— 当前就在那一档时要能显示出来）。

### POST /api/usbnet/set
设置 USB 网卡模式（`{modem_id, value}`，0~5）；写完需重启模组生效。

### GET /api/dial?modem_id=N[&force=1]
「拨号上网」页一次拉全：拨号状态 + USB 类型识别 + 类型表 + 开关三件套。
`force=1` = 重新识别 USB 类型（不走 30 秒缓存）。

```json
{"code":0,"data":{
  "modem_id":0,
  "state":{"modem_id":0,"state":"connected","state_text":"已连接","cid":1,
           "apn":"3gnet","pdp_type":"IPV4V6","ipv4":"10.0.0.1","ipv6":"2001:db8:1:2:3:4:5:6",
           "kind":"ecm","type_name":"ECM","host_setup":"…","host_iface":"eth0","host_ips":[],
           "connected_at":"2026-10-18T06:00:00Z","elapsed_sec":42,"manual_stop":false,
           "steps":[{"name":"激活 PDP（AT+CGACT）","ok":true,"info":"cid=1 已激活","at":"…"}]},
  "usb":{"modem_id":0,"value":"1","name":"ECM","kind":"ecm","source":"at","confident":true,
         "query_cmd":"AT+QCFG=\"usbnet\"","dial":{"value":"1","name":"ECM","kind":"ecm","at_dial":true,"…":"…"},
         "descriptor":{…},"host_iface_hint":{"iface":"eth2","driver":"cdc_ether",…},
         "bus":"usb","catalog_model":"fm350-gl","catalog_usb":["rndis"],"catalog_pcie":["mbim"],
         "probed_at":"…"},
  "class_notes":[],"types":[…],
  "auto_wan":{"at":"2026-10-18T12:00:00+08:00","modem_id":0,"kind":"qmi","mode":"modem_pro","ifaces":["modem0"],"applied":["建接口 modem0","modem0.proto=modem_pro","firewall.wan.network += modem0"],"skipped":[],"ok":true},
  "wan_ipv6":true,"wan_ipv6_mode":"dhcpv6","wan_ipv6_modes":[…],"dial_enabled":true,
  "host_dialer":{"backend":"native-qmi","vendor":"quectel","plan":"native quectel qmi (/dev/cdc-wdm0, apn=ctnet, v6=true, pdn=0)",
                 "handle4":1,"handle6":2,"mtu":1500,"modem_ipv4":"10.0.0.2","gateway":"10.0.0.1",
                 "dns":["…"],"modem_ipv6":"2001:db8:…/64","started_at":"…","note":""},
  "wan_mode":"modem_pro","wan_mode_label":"modem_pro（默认）","wan_modes":[…]
},"msg":"ok"}
```

- `state`：`idle`/`dialing`/`connected`/`failed` + `state_text`；`steps` 按顺序记下
  「写入 APN → 激活 PDP → 激活 IMS → 读取本机地址 → 主机侧网卡候选」每一步的成败
  （QMI/MBIM 只有一步「交办给内置拨号器（托管中）」）。
- `usb`：三条路识别（① 固件 `AT+QCFG="usbnet"`；② USB 描述符类码；③ 主机侧网卡驱动），
  `source` 标明走了哪条；`confident=false` 时 `note` 说明为什么不确定。
- `auto_wan`：自动组网最近一次结果（后台 netprov 循环 15 秒一轮，见 `rust/src/netprov/`）。
- `host_dialer`：内置拨号后端（`modemd` 自己建数据呼叫，无外部二进制）。
  - `backend=native-qmi`：`handle4/handle6`、`mtu`、`modem_ipv4`、`gateway`、`dns`、
    `modem_ipv6`、`qmap`（`mux_id` / `qmap_version` / `ep_type` / `rx_urb_size`）；
  - `backend=native-mbim`：session / MTU / 模组侧地址 / `nw_error`；
  - `vendor` 是厂商短名（`quectel` / `fibocom` / `meig` / `simcom` / …）；
  - `control` / `control_kind` 是控制口路径与驱动类型（`cdc-wdm` / `gobinet` / `mhi`）；
  - 没在跑就是 `null`。后端**只做数据呼叫**，地址仍由 netifd 的 `modem_pro` 取。
- `wan_ipv6_mode`：生效的模组侧 IPv6 取址方式（`dhcpv6` / `ra` / `static` / `off`，来自
  `[net] wan_ipv6_mode`；`wan_ipv6=false` 时恒为 `off`）。`wan_ipv6_modes` 是四档的元信息。
  从未跑过 / 被跳过时是 `null` 或 `skipped` 非空；`applied` 是逐条人话记录（页面可显示
  「刚才动了什么」）。`ok=false` 表示出错（此时 `applied` 可能是半套）。

### POST /api/dial/connect
拨号（`{modem_id, cid, apn?, pdp_type?, reconnect?, force?}`；`cid` 缺省 0 = 用默认数据 cid 1）。
按 USB 类型**只走一种方式**：

- **ECM/RNDIS/NCM 等** → 写 APN（留空沿用模组配置）→ 激活 PDP → 激活 IMS →
  读本机地址（`AT+CGPADDR`）→ 找主机侧网卡；
- **QMI/MBIM** → **不发 `AT+CGACT=1`**（会把内置拨号唯一的数据承载占掉，实测
  `QMUXError 0xe` / `wwan0 link_state 0`），只把 APN 落到拨号状态里，由下一轮
  组网循环（≤15 秒）让 `modemd` 建立数据呼叫（QMI `WDS Start` / MBIM `CONNECT`，
  QMAP 模组还会先发 `WDA Set Data Format` + `Bind Mux`，见 `runtime/dialer/`）。

  `msg` = `ok, 已交办内置拨号器 cid=N`（不是「已连上」：会话建起来到拿到地址
  还要几秒，页面随后由 `dial` 推送刷新）。MBIM 且**没有任何 APN 可用**时仍报错
  —— 空 APN 的 `CONNECT` 会被设备拒绝，不如当场说清楚。

成功 `msg` = `ok, 已拨号 cid=N`；**失败也把 `DialState` 放进 `data`**（页面显示卡在哪一步），
错误原因在 `msg`。

### POST /api/dial/disconnect
断开数据连接（`{modem_id, cid?}`；`cid` ≤ 0 时用当前拨号 cid，再兜底默认 cid）。
ECM/RNDIS/NCM 等 → `AT+CGACT=0,<cid>` + 回读确认；QMI/MBIM → 先拆掉 `modemd`
内置的 QMI/MBIM 会话（发 `Stop` / 去激活并释放 client id）再重置状态。
成功后 `manual_stop=true`（自动组网不再替用户拨回来）。`msg` = `ok, 已断开 cid=N`。

### POST /api/dial/enable
拨号总开关（`{on}`，**必须显式给值**）：写回 `[net] dial_enabled` 并即时生效。
关掉后所有拨号方式（含手动拨号）一律拒绝，且下一轮组网循环会拆掉 QMI/MBIM 会话。

### POST /api/dial/mode
WAN 模式（`{mode}` = `modem_pro` / `dhcp` / `both`，含别名）：写回 `[net] wan_mode`，
返回 `{wan_mode, wan_mode_label, wan_modes}`。真正改接口的是组网循环（≤15 秒一轮，见 netprov）。

### POST /api/dial/ipv6
IPv6 开关（`{on}`，**必须显式给值**）：写回 `[net] wan_ipv6` 并即时生效，
四种拨号方式都按它来（QMI/MBIM 不建 v6 呼叫、ECM/NCM/RNDIS 不建 `modem<id>_6` 等）。

**取址方式**由 `[net] wan_ipv6_mode` 决定（`dhcpv6` 默认 / `ra` / `static` / `off`，
见 `PROGRESS.md` §30；350 系列对齐 `luci-app-fm350`）：`dhcpv6` 交给 odhcp6c 并
`extendprefix=1` 把 `/64` 委派给 LAN；`ra` 用 `proto=none` + `accept_ra=2` + 主动发 RS；
`static` 才写 `/128`（无前缀残值会被过滤并退回 `dhcpv6`）。本接口不改这一项，改配置文件即可。

### GET /api/wechat
企业微信推送的配置与登录态。**不回传 `bot_token`**（只回 `has_token` 布尔值）。

```json
{"code":0,"data":{
  "enabled":false,"logged_in":false,
  "stage":"idle","stage_cn":"未登录","stage_msg":"还没登录：先扫码登录…",
  "qr_png":"","qr_left":0,"verify_required":false,
  "state":"未登录","to_user_id":"","effective_target":null,
  "last_sender":"","session_ready":false,"heard_message":false,"has_context":false,
  "listen_on":true,"last_incoming":"","last_incoming_at":null,
  "template":"📩 新短信{parts}\n模组：{modem}\n号码：{number}\n时间：{time}\n内容：{content}",
  "listen":true,"allowed_senders":"","bot_id":"","bot_user_id":"",
  "token_mask":"","has_token":false,"transport_ok":true,
  "last_send":null,"sent_ok":0,"sent_fail":0,
  "default_template":"📩 新短信{parts}…"
},"msg":"ok"}
```

- `stage`：`idle` / `wait` / `ready` / `expired`（页面按它分支：等扫码时画二维码，
  登录后换配置面板）；`stage_cn` 是给人看的中文。
- `qr_png`：二维码的 **SVG data URI**（后端离线画，不联网也不依赖前端库）；
  `qr_left` 是剩余秒数。
- `last_sender` / `last_incoming` / `last_incoming_at`：最近给机器人发过消息的人与时间；
  `session_ready` = 已登录 **且** 真的收到过对方消息（没收到时主动发会被服务端拒）。
- `sent_ok` / `sent_fail` / `last_send`：推送计数与最近一次发送记录（含失败原因）。
- `token_mask`：只露首尾 4 位的 token，**不回传完整凭据**。
- `effective_target`：实际推送目标。`to_user_id` 显式指定时就是它；为空时是
  **学到的**「最近给机器人发过消息的人」（`listen` 开着才会学）；
  两者都空 ⇒ `null`，此时不会推送。
- `transport_ok`：设备上有没有 `curl`。`false` 时推送必然失败
  （`https://` 需要 TLS，而 `std` 没有），页面上该说清楚。

⚠️ 短信访问密码已于 2026-10-19 取消；本接口只受**外层登录闸门**约束
（`[ui] require_luci_login`）。能改推送目标就能把短信转发到任意微信，
所以这个开关的意义仍然在「谁能登录路由器」。

### POST /api/wechat/login
拿登录二维码（`{}`，无参数）。返回 `{state:"waiting_scan", qr_url, expires_at}`。

已有**未过期**的二维码时直接复用，不重复拿（浪费一次公网往返）。
设备上没有 `curl` 时报 `code=-1`。

### POST /api/wechat/verify
轮询扫码结果（`{}`，无参数）。

- 还没扫 / 扫了没确认 ⇒ `{state:"waiting_scan"}`（**不是错误**，页面继续轮询）；
- 没有等待中的二维码 ⇒ `code=-1`（提示先点「扫码登录」）；
- 过期 ⇒ `code=-1`；
- 成功 ⇒ `{state:"logged_in", bot_id, bot_user_id}`，并把三份凭据
  **写回 config.toml**（不写的话重启又回到未登录 —— `bot_user_id` 以前
  就是因为结构体没这个字段被静默丢掉的，见 `PROGRESS.md` §24.1）。

### POST /api/wechat/set
改配置（表单）。可改 `enabled` / `listen` / `to_user_id` / `template` /
`allowed_senders`；未知键**忽略**（页面多带一个字段不该让整个保存失败）。

内存覆盖即时生效，同时行级原地写回 `config.toml`（保留注释）。
返回体与 `GET /api/wechat` 同构。`template` 传空串 = 回落默认模板。

### POST /api/wechat/test
发一条测试消息（`{}`）。未登录 / 没有推送目标时报 `code=-1` 并说明缺什么。
⚠️ 同步跑 curl 到公网，超时 15 秒，会占住一个 HTTP 工作线程。

### POST /api/wechat/logout
登出：清内存态 + 清空 config.toml 里的 `bot_token` / `bot_id` / `bot_user_id`。
返回 `{state:"logged_out"}`。

---

### POST /api/airplane/set
开关飞行模式（`{modem_id, enabled}`）；不支持 `AT+CFUN?` 的机型会被拒绝。

### POST /api/roam/set
「数据漫游」开关（`{on}`）：写回 `[net] allow_roaming` 并即时生效，返回漫游视图
（含 `allow_roaming` / `effective` / `reason`）。

### POST /api/modem/clock
把模组时钟校准到主机 UTC（`AT+CCLK="yy/MM/dd,HH:mm:ss+00"`；固件不收时区尾巴时
自动回退为不带时区重发）。返回体与 `GET /api/modem/status` 同构，
`module_time` / `clock_skew_s` 即校完回读的值。

### POST /api/modem/reset
重启模组（`AT+CFUN=1,1`），约 30 秒后 AT 口恢复。指令已写进串口即视为成功
（重启本来就会断开 AT 口，等不到 OK 是常态，`confirmed=false` + `note` 说明）。

```json
{"code":0,"data":{"modem_id":0,"cmd":"AT+CFUN=1,1","confirmed":false,
 "note":"指令已下发但未收到确认（重启会断开 AT 口，属预期）"},"msg":"ok"}
```

### GET /api/lock/status
锁网状态（`modem_id` 查询参数）。返回四块：

```json
{"code":0,"data":{
  "modem_id":0,
  "mode_pref":"AUTO","dialect":"qnw","dialect_name":"高通 QNW",
  "separate_nsa":false,
  "band_lte":{"tech":"LTE","bands":[3],"capable":[1,3,41],"locked":true,
              "declared":[1,2,3,4,5,7,8,12,13,18,19,20,25,26,28,38,40,41,42],
              "declared_note":"Modem-Pro 频段表（出厂口径）：支持 19 个 LTE 频段，可与上方「设备能力」对照"},
  "band_nr":{"tech":"NR","bands":[],"capable":[78],"locked":false,
             "declared":[1,3,7,8,20,28,38,40,77,78],"declared_note":"…"},
  "bands":{"model":"fm350-gl","vendor":"fibocom","platform":"mediatek",
           "wcdma":[1,2,5,8],"lte":[...],"nr_nsa":[...],"nr_sa":[...]},
  "band_groups":[
    {"key":"wcdma","title":"WCDMA（3G）","label":"WCDMA","tech":"","lockable":false,
     "bands":[1,2,5,8],"declared":[1,2,5,8],"note":"只展示：…"},
    {"key":"lte","title":"LTE（4G）","label":"LTE","tech":"LTE","lockable":true,
     "bands":[1,2,3,...],"declared":[1,2,3,...]},
    {"key":"nr","title":"NR（5G）","label":"NR","tech":"NR","lockable":true,
     "bands":[1,3,7,8,20,28,38,40,77,78],"declared":[...],
     "note":"NSA / SA 在设备侧是同一套频段（只有一条锁频段指令）"}
  ]
},"msg":"ok"}
```

| 字段 | 含义 |
| --- | --- |
| `band_lte` / `band_nr` / `band_nr_nsa` | 单技术域状态：`bands` = 设备当前值，`capable` = 运行期读到的**设备能力**（固件不报时为空 + `capable_note`），`declared` = **清单出厂口径**（QModem 频段表），两者不一致正是排查信息 |
| `declared_note` | 清单口径说明：型号没登记 / 上游没写频段 / 该制式没记录 / 支持 N 个 |
| `bands` | 本型号四组支持频段（`wcdma/lte/nr_nsa/nr_sa`）；`null` = 不在频段表 |
| `band_groups` | 锁频段页的可勾选分组（清单 ∪ 设备能力 ∪ 当前锁定值，按型号自动排版）。`tech` 是下发时用的技术域，空 = 本组只读；`lockable=false` 的组（WCDMA）只展示 |
| `separate_nsa` | 本机方言的 NR 是否分 NSA / SA 两套列表：`true` 时 `band_groups` 出 `nr_nsa` + `nr_sa` 两组，否则只出一组 `nr`（设备侧只有一条锁频段指令） |

> 分组频率列表**必须**含清单全集：少了它，用户取消勾选并下发后，那个频段会从页面消失、勾不回来。

### POST /api/lock/band
锁频段 / 解除频段锁定。请求体 `{modem_id, tech, bands, unlock?}`：
`tech` = `LTE` / `NR`（QNW 方言另有 `NRNSA`）；`bands` 为频段数组（如 `[78]` 或 `[3,8]`）；
`unlock=true` 时忽略 `bands`，表示该技术域不限制。返回最新锁网状态（同 `/api/lock/status`）。

> QNW 方言（移远/广和通高通平台）**不能写 0 解除**——那会清空频段、直接脱网；
> 解锁是读 `policy_band` 后**全量写回**。芯讯通方言的解锁写法未经验证，会明确拒绝。

**广和通 MTK 方言**（`dialect` = `fibocom`，FM350 / RW350，指令 `AT+GTACT`）与上面
两套**结构不同**，因此行为也不同：

- **一条命令同时写 GSM / LTE / NR 三组**，所以改一个技术域必须读改写——另外两组
  （含没有 UI 入口的控制字段与 GSM 组）原样带回去。实测锁 NR→n78 时 LTE 的 31 个
  频段原样不动。
- **解锁不是写 0**：`AT+GTACT` 没有「不限制」的哨兵值，写 0 或留空等于清空该组频段。
  解锁 = 写回 `AT+GTACT=?` 的支持全集；读不到支持全集就拒绝执行。
- **锁定前会校验频段**：不在 `AT+GTACT=?` 支持列表里的一律拒绝，并在错误信息里列出
  支持全集。
- **锁频前先离线**（`CFUN=0` → 下发 → `CFUN=1`），这是该指令的要求；下发失败时
  会把模组拉回在线，不会把它扔在飞行模式里。

频段编码（解码后 `bands` 里放的是**band 号**，B3 而不是 103、n78 而不是 5078）：

| 技术域 | `AT+GTACT` 编码 | 例 |
| --- | --- | --- |
| LTE | `100 + n` | B1 → `101`，B71 → `171` |
| NR | `"50"` 与 band 号十进制拼接 | n1 → `501`，n20 → `5020`，n78 → `5078` |
| GSM | 原值（锁网契约无 GSM 域，仅写回时保留） | — |

> `AT+GTACT?` 回显是**位置参数**，但 `AT+GTACT=?` 显示有 9 个参数组、其中 3 组是
> 空集，在回显里不占位——所以频段按**值域**分类取，不按下标（按下标必然错位）。

### POST /api/lock/cell
锁小区 / 锁频点 / 解除。请求体 `{modem_id, tech, lock_type, earfcn, pcid?, scs?}`：
`lock_type` = 0（解除）/ 1（锁频点+PCI，必须给 `pcid`）/ 2（仅锁频点）；
`scs` 仅 NR 用。返回最新锁网状态。

> 广和通 MTK 方言（`AT+EMMCHLCK`）**拒绝锁定**：实机 `AT+EMMCHLCK=?` 给出 5 个
> 参数组 `(0-1),(0,2,7),(0,1),(0-46589),(0-511)`，末两组能认出是 EARFCN / PCI
> 区间，但中间两组枚举含义无手册可查、锁定态回显格式也未实测。**解锁
> （`lock_type=0`）可用**（写法就是 `AT+EMMCHLCK=0`）。读状态不受影响：未锁定时
> 回 `+EMMCHLCK: 0`，锁定态的字段位置由上述参数组推断并在 `raw_value` 里保留整行
> 供人工核对。

### POST /api/lock/mode
组网模式偏好（旧接口，等价于 `/api/mode/set`）。请求体 `{modem_id, mode}`。

### POST /api/lock/unlock
解除全部锁定（各技术域频段回 policy + 小区锁定置 0）。请求体 `{modem_id}`，
返回最新锁网状态。芯讯通方言未验证解锁写法，会明确拒绝。

### POST /api/sms/send
发短信。请求体 `{modem_id, phone, content}`：
中文 / emoji 自动切 UCS2（`AT+CSCS="UCS2"` + `AT+CSMP=17,167,0,8`），超长自动分段。

**广和通 MTK 平台（FM350 / RW350 系列）整条序列换成 PDU 模式**——该平台 TEXT 模式下
中文与长短信都不可靠，故不是在文本序列上打补丁。序列 `AT+CMGF=0` → `AT+CMGS=<TPDU 字节数>` →
等 `>` → PDU + Ctrl-Z → 发完切回 `AT+CMGF=1`（短信模式是全局状态，不复位会影响后续读短信）。
切分按**真实容量**算：GSM7 单条 140 字节（160 septet）、UCS2 单条 140 字节（70 字符），
UDH 各让 7 字节。UDH 开启时不自动发 `UDHI`。分段发送中途失败时报「第 N 条被模组拒绝
（已成功 M/T 条）」，只报一句「发送失败」的话用户不知道该不该重发整条。

```json
{"code":0,"data":{"modem_id":0,"phone":"13800138000","parts":1,"ucs2":true,"note":null},"msg":"ok"}
```

### POST /api/sms/delete
删一条（`{modem_id, index}`）或全部（`{modem_id, all:true}`）。

```json
{"code":0,"data":{"modem_id":0,"all":false,"removed":1},"msg":"ok"}
```

### POST /api/sms/storage
切短信存储区（GET 是读用量）。请求体 `{modem_id, mem}`（`SM` / `ME` / `MT`），
下发 `AT+CPMS` 后回读确认，并写回 `[sms] sms_memory`。

### GET /api/quota?modem_id=N
流量配额状态。查询会顺手采样一次流量、累计到本周期，并按配置执行/维持超限动作。

```json
{"code":0,"data":{"modem_id":0,"supported":true,"enabled":true,"limit_bytes":1073741824,
 "limit_text":"1.00 GB","used_bytes":524288000,"used_text":"500.0 MB","used_up":104857600,
 "used_down":419430400,"remain_bytes":549453824,"remain_text":"524.0 MB","percent":48.8,
 "over_limit":false,"blocked":false,"suspended":false,"estimated":false,"action":"block",
 "count_up":true,"count_down":true,"reset_day":1,"period_start":"2026-10-01T00:00:00Z",
 "period_end":"2026-11-01T00:00:00Z","blocked_at":null,"note":null},"msg":"ok"}
```

`supported=false` 表示模组不提供流量计数器（配额无法启用）；`estimated=true` 表示用量由
瞬时速率积分估算（广和通只报速率）。

### POST /api/quota/set
保存配额设置并按新口径立即评估。请求体：`{modem_id, enabled, count_up, count_down, action,
reset_day, limit_bytes | limit_mb | limit_gb, used_gb?}`。上行/下行都不计入时返回失败。
返回同 `/api/quota`。

### POST /api/quota/reset
本周期用量清零并开始新周期（同时恢复数据连接）。请求体 `{modem_id}`，返回同 `/api/quota`。

### POST /api/quota/unblock
本周期不再按上限断网，并恢复数据连接。请求体 `{modem_id}`，返回同 `/api/quota`。

### POST /api/traffic/reset
清零模组的累计流量计数器（`AT+QGDCNT=0`；速率式机型只重置本地积分基准）。返回流量视图：

```json
{"code":0,"data":{"supported":true,"estimated":false,"cmd":"AT+QGDCNT?","sent":0,"recv":0,
 "up_bps":0,"down_bps":0,"up_text":"0 bps","down_text":"0 bps","interval_s":0.0},"msg":"ok"}
```

### GET /api/ussd?modem_id=N
读最近一次 USSD 应答。`result` 为 null 表示还没收到过应答。

```json
{"code":0,"data":{"modem_id":0,"result":{"status":0,"text":"余额为 12.5 元","at":"2026-10-18T02:50:00Z"}},"msg":"ok"}
```

`status`：0 无需后续操作 / 1 需要继续 / 2 会话被网络结束 / 4 不支持 / 5 超时。

### POST /api/ussd
下发 USSD 码。请求体 `{modem_id, code}`；`code` 只允许数字与 `* # +`，长度 1~64。

```json
{"code":0,"data":null,"msg":"已下发，等待模组应答（异步推送）"}
```

⚠️ 应答是**异步 URC**（`+CUSD`）：本接口只代表「已下发」，真正的回复通过 WebSocket
`ussd` 消息推送（或稍后 GET 一次）。部分固件把 `+CUSD` 放在 `OK` 之前一起回来，那种情况
服务端会当场推送。失败示例（USSD 是电路域业务，5G SA 数据卡 / 物联网卡多半没有 CS 注册）：

```json
{"code":-1,"data":null,"msg":"模组拒绝 AT+CUSD=1,\"*100#\",15：USSD 是电路域(CS)业务，当前 CS 未注册或卡不支持（可用「概览→网络」里的 CS 域状态确认）"}
```

### GET /api/netscan?modem_id=N&tech=lte|nr|both&minutes=3~10
主动扫网（慢接口，最长 `minutes` 分钟，期间独占该模组 AT 口）。

```json
{"code":0,"data":{"modem_id":0,"tech":"LTE","scanned_at":"2026-10-18T02:50:00Z",
 "duration_ms":18230,"wait_ms":180000,"stopped":false,
 "operators":[{"status":2,"long":"CHN-UNICOM","short":"UNICOM","numeric":"46001","act":"7","tech":"lte"}],
 "operator_cmd":"AT+COPS=?","cells":[],"cell_cmd":"","cell_note":"本固件没有提供主动扫小区结果（该能力仅部分固件支持）",
 "steps":["设置运营商名为长名格式（AT+COPS=3,0）","可见运营商搜索完成（AT+COPS=?）"],"raw":["..."]},"msg":"ok"}
```

### POST /api/netscan/stop
叫停正在进行的扫网（只置标志、立即返回）。

```json
{"code":0,"data":{"modem_id":0,"stopped":true},"msg":"ok"}
```

### GET /api/imei?modem_id=N
IMEI 管理状态（含备份信息）。`supported=false` 表示本机厂商没有可用的写入指令
（本程序不对认不出的机型发未经验证的厂商私有指令）。

写入指令按厂商分派：移远 / 美格等高通系 `AT+EGMR=1,7,<imei>`，
**广和通 MTK 平台（FM350 / RW350 系列）`AT+EGMREXT=1,7,<imei>`**，其同族只读形式
`AT+EGMREXT=0,7` 已实机验证可用（`AT+EGMREXT?` 回 `+CME ERROR: 100`，查询写法不可用）。
`luhn_ok` 是 Luhn 校验结果，**仅作非阻断提示**（部分厂商 IMEI 不严格遵循 Luhn，不阻断写入）。

```json
{"code":0,"data":{"modem_id":0,"supported":true,"cmd":"AT+EGMREXT=1,7,","read_cmd":"AT+CGSN",
 "imei":"867698040012349","luhn_ok":true,"original":null,"original_saved_at":null,"writes":0,
 "can_restore":false,"restored_at":null,
 "note":"只接受 15 位数字；写前用同族只读命令 AT+EGMREXT=0,7 快照备份，写后回读确认（本平台需重启模组后才生效）"},"msg":"ok"}
```

### POST /api/imei/set
改写模组 IMEI（仅移远 / 美格等高通系与广和通 MTK 平台）。请求体 `{modem_id, imei}`（15 位数字）。
第一次改写前会先把当前值备份到 `imei.json`（与 `config.toml` 同目录），之后才能一键恢复；
备份用**与写入指令同族的只读命令**（广和通 MTK 走 `AT+EGMREXT=0,7`，其余走 `AT+CGSN`），
模组明确应答空值同样算备份成功（空 IMEI 模组正是最需要改写的维修场景），两条路都读不到
才拒绝改写。写完回读确认：**不一致时的口径按平台分流**——

- 广和通 MTK：`AT+EGMREXT` 写入需**重启模组后**才在射频 / 网络侧生效，回读到空值 /
  旧值都可能是「还没生效」，此时不判失败，响应带 `warning` 提示重启后复查；
- 其它厂商：回读不一致即报错。

返回同 `/api/imei`，并可能额外带 `warning`（仅本次响应携带，`GET /api/imei` 不带）。

> 广和通 MTK 平台的 `AT+EGMREXT=1,7,<imei>` **未被广和通手册（v2.2 / v2.10）收录**，
> 属社区与实机验证命令。本程序已有三重保护（15 位数字校验、改写前同族快照备份、
> 下发后回读确认），故破例收录；`AT+GTIMSILOCK?` 为写保护查询，`AT+GTCCID`/`AT+GTSNWR=1,<imei>` 为另一套写法，
> 实机未验证前不发。

```json
{"code":0,"data":{"modem_id":0,"supported":true,"cmd":"AT+EGMREXT=1,7,","read_cmd":"AT+CGSN",
 "imei":"867698040012349","luhn_ok":true,"original":"123456789012345","original_saved_at":"2026-10-18T05:44:43Z",
 "writes":1,"can_restore":true,"restored_at":null,"note":"…",
 "warning":"写入命令已下发且模组未报错，但回读为「空」—— 广和通 MTK 平台的 AT+EGMREXT 需重启模组后才生效，请重启后复查"},"msg":"ok"}
```

### POST /api/imei/restore
写回备份的原始 IMEI（`{modem_id}`）。这台模组从未被本程序改写过（或备份原值为空、
模组出厂即无 IMEI）时拒绝执行。返回同 `/api/imei`（`can_restore` 变回 `false`，
`restored_at` 记下时刻）；广和通 MTK 平台恢复后同样需重启模组才生效，会带 `warning`。

### GET /api/host/nic?modem_id=N
主机侧网卡配置视图（FM350 这类"模组侧拨号成功但主机上不了网"的机型用）。
面板在前端已下线，接口保留：给的是一份"配置 + 可复制命令"。

```json
{"code":0,"data":{"modem_id":0,"model":"FM350-GL","required":true,"reason":"FM350 系列…",
 "os":"linux","elevated":true,"platform":"openwrt","openwrt":true,
 "interfaces":[{"name":"eth1","up":true,"addrs":["169.254.3.7/16"],"strong":true,
                "note":"只有 169.254.x.x（没拿到 DHCP —— 这个机型要手工配一次）"}],
 "plan":{"iface":"eth1","ip":"10.42.0.2","mask":"255.255.255.0","gateway":"10.42.0.16","dns":[]},
 "saved":false,"saved_plans":{},"suggested":{"iface":"eth1","ip":"10.42.0.2","mask":"255.255.255.0","gateway":"10.42.0.16","dns":[]},
 "modem_ip":"10.42.0.16","actions":[…三组动作…],"persist":[…UCI 命令…],"checked_at":"…"},"msg":"ok"}
```

- `required`：只有 **350 系列（FM350 / RW350）**为 true（判据认 ATI 原文里的型号串）。别的机型
  主机侧地址由模组 DHCP 或内置拨号给，加了只会让人以为"每次都要手工配"。
- `plan`：建议值按模组上报的本机地址推（同网段取 `.2`、网关指向模组），**仅供参考**，
  真正的网段只有模组那份配置知道。
- `persist`：仅 OpenWrt 给（`uci` 写 `/etc/config/network`），全部 `manual=true`
  —— 只显示不执行（运行期的 `ip addr/route` 会被 netifd 的 reload 抹掉）。

### POST /api/host/nic/apply
生成一组命令（`{modem_id, iface, ip, mask, gateway, dns[], mode, execute}`）。
`mode` 三选一：`renew`（`udhcpc -i`，最多等 90 秒）/ `static`（flush→add→up→默认路由，
OpenWrt 上不写 `resolv.conf`）/ `dhcp`（恢复自动获取）。`execute=false`（默认）只生成命令；
`true` 才由本程序下发（走 `prog + args`，**不拼 shell**），进程没权限时自动退回"只生成"。
配置不合法时 `commands` 为空、只给 `note`（半套命令比没有更糟）。

```json
{"code":0,"data":{"view":{…同 GET…},"apply":{"label":"应用静态配置","executed":false,
 "commands":[{"label":"配静态地址","text":"ip addr add 10.42.0.2/24 dev eth1","admin_only":true}],
 "results":[]}},"msg":"ok"}
```

### POST /api/host/nic/save
保存一张网卡的手工配置（`{modem_id, iface, ip, mask, gateway, dns[]}`）到
配置文件同目录的 `hostnic.json`（重启不丢）。掩码归一成点分、DNS 去空去重；
校验不通过直接报错、不落盘。返回同 `GET` 的视图，`msg` 形如 `ok, 已保存 eth1 的配置`。

### GET /api/schedule
定时器页面一次拉全：任务列表 + 可选动作元信息 + 目标模组下拉。任务按**本机时区**判定
（`TZ` / `/etc/TZ` 的 POSIX 写法；认不出按 UTC 并记日志），到点后 15 分钟内允许补做，
同一"日期+时间"只执行一次（去重记录随 `schedule.json` 落盘，重启不丢）。

```json
{"code":0,"data":{"now":"2026-10-18T00:15:36Z","tasks":[],"modems":[
  {"key":"port:/dev/ttyUSB2","label":"#0 /dev/ttyUSB2 RM520N-GL","id":0}],
 "kinds":[{"kind":"band_lock","label":"锁频段","needs_locker":true,"has_tech":true,
   "needs_bands":true,"needs_cell":false,"needs_mode":false}],
 "max_tasks":50,"supported":true},"msg":"ok"}
```

`tasks` 里每条是任务本体 + 展示字段（`target_label` / `target_ok` /
`start_actions[].text` / `next_runs[]`（`"09-30 08:00 周三"`）/ `last{phase,at,ok,msg}` /
`start_unsupported` / `end_unsupported`——后者非空表示该侧第一个动作此机型不支持，
列表状态胶囊据此显示「机型不支持」）。任务字段：

```json
{"id":"t1792282536d7212d","name":"白天锁n78","enabled":true,
 "modem_key":"port:/dev/ttyUSB2","port":"/dev/ttyUSB2","imei":"123456789012345",
 "days":[],"start_time":"08:00",
 "start_actions":[{"kind":"band_lock","tech":"NR","bands":"78"}],
 "end_time":"22:00","end_actions":[{"kind":"band_unlock","tech":"NR"}],
 "slots":{"start":"2026-10-30 08:00"},"last":null,
 "created_at":"2026-10-18T00:15:36Z","updated_at":"2026-10-18T00:15:36Z"}
```

动作 `kind` 共 8 种：`band_lock`（tech+bands）/ `band_unlock`（tech）/
`cell_lock`（tech+earfcn+pcid+lock_type（1=频点+PCI 2=仅频点）+persist）/ `cell_unlock`（tech）/
`unlock_all` / `mode_pref`（mode：7 档语义值 `AUTO/WCDMA/LTE/NR5G/WCDMA:LTE/WCDMA:NR5G/LTE:NR5G`
或本机原码）/ `airplane_on` / `airplane_off`。`days` 是生效星期（0=周日…6=周六，空=每天）。

### POST /api/schedule/set
新增（`id` 为空则生成）/ 更新一条任务。请求体就是任务字段，另可带 `modem_id`
（目标模组留空时回退到它）、兼容旧版 `start_action` / `end_action` 单动作写法。
保存前校验并归一化：`8:00` → `08:00`、tech 别名（`5g`→`NR`）、频段排序去重、
模式大写；结束时间留空时结束动作一并清掉；任务名缺省自动生成。
目标模组补全 Port / IMEI（仅展示与换端口迁移用）。校验失败返回 `{code:-1,msg:原因}`；
成功返回与 `GET /api/schedule` 相同的视图，并广播 `schedule` 推送。

```json
{"code":0,"data":{"now":"…","tasks":[…],"modems":[…],"kinds":[…],"max_tasks":50,"supported":true},"msg":"ok"}
```

### POST /api/schedule/delete
删除一条任务（`{"id":"..."}`）。不存在时返回 `{code:-1,msg:"任务不存在（可能已被删除）"}`；
成功返回视图并广播。

### POST /api/schedule/toggle
启用 / 停用一条任务（`{"id":"...","enabled":false}`）。停用的任务到点不执行。
不存在同样返回「任务不存在」；成功返回视图并广播。

### POST /api/schedule/run
立即执行一次开始动作（调试用，`{"id":"..."}`），并把当前 15 分钟窗口内的开始槽位
标记为已跑（避免这次手动执行后循环又自动补一遍）。任务不存在 / 目标离线 /
串口未开时返回 `{code:-1,msg:原因}`；执行了但模组拒绝时按 `fail_data` 返回
（`code=-1` + 视图 + 错误文本，界面直接显示便于对账）；成功 `msg:"已执行"`。

### GET /api/ntp
NTP 校时状态（全局能力，不属于任何模组）。插件自带 SNTP 客户端（UDP 123）：
启动 3 秒后首试，失败按「3 秒×10 → 1 分钟×10 → 20 分钟常驻」逐级退避；
同步成功只校准**插件自己的时间源**（日志 / 定时器 / 配额全用，不改系统时钟），
一直失败保持偏移 0，即回退读设备时间。

```json
{"code":0,"data":{"enabled":true,"synced":true,"source":"ntp","server":"ntp.tencent.com",
 "offset_secs":0,"delay_ms":12,"last_ok_at":"2026-10-19T05:12:00Z","last_try_at":"…",
 "phase":"steady","phase_label":"常态（每 20 分钟）","attempts":0,"next_in_secs":1180,
 "last_error":"","ok_count":3,"fail_count":1,
 "servers":["ntp.tencent.com","time.cloudflare.com","ntp.aliyun.com"],
 "timezone":"Asia/Shanghai","now":"2026-10-19T05:12:33Z","now_local":"2026-10-19 05:12:33 +08:00"},
 "msg":"ok"}
```

`source`：`ntp`（已校准）/ `system`（回退设备时间）；`now_local` 为本地时区
（`[ntp] timezone` 优先于 `TZ` / `/etc/TZ`）的当前时间。每次尝试 / 阶段切换都会
广播 `ntp` WS 消息（`data` 同形）。

### POST /api/ntp/sync
请求立即同步一次（不等下一个节拍）。只置强制标志立即返回（不在这里等服务器应答，
免得三台挨个超时把请求挂住十几秒）；后台循环最迟 1 秒内执行，结果经 `ntp` 推送到达。

```json
{"code":0,"data":{…同 GET /api/ntp…},"msg":"已请求立即同步（1 秒内执行，结果会推送到页面）"}
```

### POST /api/ntp/set
设置时区（`{"timezone":"Asia/Shanghai"}`，也认 POSIX 写法 `CST-8`）。解析成功就
全局生效（所有本地时间立刻切换，不用重启），并原地写回 `config.toml` 的
`[ntp] timezone`（保留注释）；写不出的写法返回 `{code:-1,msg:原因}` 且**不动**
现有时区。服务器列表与总开关不提供运行期修改（改 `config.toml` 后重启生效）。

```json
{"code":0,"data":{"timezone":"Asia/Shanghai","offset_secs":28800,"offset_label":"UTC+8"},
 "msg":"时区已设为 Asia/Shanghai（UTC+8，立即生效）"}
```

---

## WebSocket 动作协议（客户端 → 服务端）

握手（`GET /ws`，屏端 `GET /display/ws?key=<DISPLAY_KEY>`）之后客户端可以发 JSON 指令：

```json
{"action":"mode_set","modem_id":0,"value":"LTE"}
```

字段：`action`（必填）/ `modem_id`（缺省 0）/ `index`（短信**存储位置**，字符串）/
`value`（`mode_set` 的档位）/ `number` + `text`（`sms_send`）。未给的按零值；
JSON 解析失败或 action 不认识**静默忽略**（不做任何应答，也不发 AT）。

| action | 参数 | 行为 | 完成后推送 |
| --- | --- | --- | --- |
| `snapshot` | — | 再来一份模组列表快照 | `modems` |
| `ping` | — | 回 `{"type":"pong"}` | — |
| `sms_read` | `index` | `AT+CMGR=<index>`（读一次即已读）→ 重读短信 | `sms` + `smscount` |
| `sms_delete` | `index` | `AT+CMGD=<index>` → 重读短信（未读角标一起回落） | `sms` + `smscount` |
| `mode_get` | — | 读一次组网模式 | `mode` |
| `mode_set` | `value` | 下发 + 回读确认 | `mode`（`msg` 是「已切换到 …」或「设置失败：…」） |
| `sms_send` | `number`,`text` | 发一条短信 | `sendsms` |
| `net_get` | — | 读一次网络状态 | `network` |

`mode` 消息的 `data`（屏端 320px 屏的取舍：多给一个**短名**）：

```json
{"value":"LTE","name":"仅 4G LTE","short":"仅4G","msg":"已切换到 仅 4G LTE",
 "choices":[{"value":"AUTO","name":"自动","writable":true},{"value":"LTE","name":"仅 4G LTE","writable":true}],
 "screen":[{"value":"LTE","name":"4G","writable":true},{"value":"NR5G","name":"5G","writable":false},
           {"value":"LTE:NR5G","name":"4G+5G","writable":false}],
 "read_only":false}
```

`short` 是右上角那行用的短名（自动 / 仅3G / 仅4G / 仅5G / 3G+4G / 3G+5G / 4G+5G）；
`screen` 固定三个按钮，本机档位表里没有的档 `writable=false`（置灰，别让用户点了必定失败）；
`read_only=true` 表示本机只能读（按钮全灰）。空串与 `false` 字段不出现。

`sendsms` 消息的 `data`：`{"number":"10086","text":"CXHF","ok":true,"msg":"已发送，等运营商回复"}`。

> ⚠️ 除 `snapshot` / `ping` 外全部在独立线程执行：发短信要等提示符、切模式要等回读确认，
> 都要占串口几秒，卡在 WS 读循环里会让这一路的心跳与推送全停。

---

## 静态页

`/`、`/index.html`、`/mini.html`、`/layouts/*.css`：内置调试台（编译期嵌入二进制）。
其余非 `/api/*` 路径返回 404。

---

## 待补齐（迁移清单）

PCIe/MHI 复位与绑驱动在真机上的表现等。
新增接口沿用上面的 `{code,data,msg}` 契约。

> URC 的**巡检加速**与**屏端推送**均已接线（`spawn_urc` + `model/urc.rs`，
> 见 `PROGRESS.md` §21 / §22）：`+GTSIM` 把 SIM 巡检加速到下一拍，注册态 URC 把网络
> 状态重查加速到下一拍，`+CUSD` 应答自动推 `ussd`，`+CMTI` 触发一次短信重读后推
> `sms` / `smscount`。那不影响本文件的任何契约（不新增接口、不改字段），
> 只是让数据过期得更快、新内容不用手动刷页面。
