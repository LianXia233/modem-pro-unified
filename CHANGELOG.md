# 变更记录

本项目采用 [语义化版本](https://semver.org/lang/zh-CN/)；版本号单一来源为仓库根 `VERSION`。

## [未发布]

### 1.0.0-7：实时速率与曲线 + 拨号链路状态 + 时区下拉 + AT 串口三层保护

本版两块工作：**A. 让页面上的数字来自真实网卡**（速率/流量/曲线/拨号状态/时区），
**B. 别把 AT 一起打给模组**（优先级排队 + 30ms 命令节奏 + 打开冷却）。

#### 🟢 A1. 速率与流量改从 sysfs 采样（`runtime/ifstat.rs`，新增模块）

**根因**：速率/累计原本取自模组 AT 计数器（移远 `AT+QGDCNT?` / 广和通
`AT+GTSTATIS?`）。实机 FM350-GL 上 `AT+GTSTATIS` 直接回 `ERROR`，于是
`supported:false`、速率与累计**恒为 0**，页面四个指标全空 —— 而
`/sys/class/net/eth2/statistics/` 里明明有真实数据（实测 rx 238MB / tx 65MB）。

- 独立线程**固定 1000ms** 采样，**不依赖前端是否打开**（关页面照采）；
- 用 `now - prev` 的实际时间差算速率（调度延迟不会把数值算虚高）；
- 计数器回退（接口重建 / 32 位回绕）时重设基线，不产生巨大负值；
- 历史用**有界环形缓冲**（60 点），跑几个月也不涨内存；
- 累计分三档且**不混用**：接口生命周期内（sysfs 绝对值）/ 本进程累计
  （逐拍增量）/ 可持久化（本期未做，避免每秒写闪存）。

**接口识别不写死网卡名**：`netprov::pick_device` 从模组自己的 USB 设备推导
（QMAP mux 口 → 强候选 → 同 USB 设备下的口，排除 eth0/eth1 板载口），
推导不出来再按驱动全局找（`rndis_host` / `qmi_wwan` / `cdc_mbim`）。

**三处同源**：`/api/traffic`、`/api/network/status.traffic`、WS 推送
`push_traffic` 现在读同一份 `IfStat`，LuCI 与内置 WebUI 不会再算出两个数。

#### 🟢 A2. 实时曲线（`spark.js` + WebUI 内联 SVG）

- LuCI 概览页新增「实时速率曲线」卡片：上下行两条曲线、60 秒窗口、
  纵轴按实际速率自动取整（1/2/5×10ⁿ）、面积填充；
- 内置 WebUI 同款（内联一份精简实现，单文件页面不为 60 行 SVG 再拉请求）；
- **不引第三方图表库**；配色避开红绿（那是涨跌语义），上行琥珀、下行蓝；
- 前端多久拉一次只影响「多久前移一格」，历史点在后端环形缓冲里，不会丢数据。

#### 🟢 A3. 拨号链路状态（`model/dial.rs`）

**根因**：状态机只看「本程序有没有拨过号」。实机 `eth2` 是 netifd 按 UCI
（`modem0` proto=static）拉起来的，地址 `10.42.0.16` 真实可用、默认路由也走它，
但本程序没拨过号 → 状态停在 `idle` → 页面显示「未拨号」。

- 新增 `disconnected` / `disconnecting` / `unknown` 三个状态，与 `idle` 区分开；
- 判据改成**看实际地址**：`ip -4 -o addr show dev <iface>`（带 5 秒缓存防 fork 风暴），
  刻意**不看** `operstate == up` —— 接口 UP 只说明链路层在，DHCP 失败照样没 IP；
- 新增 `link_source` 字段标出链路是谁拉起来的（`modemd` / `netifd`），排障一眼分清；
- `DialState.link_source` 为 `netifd` 时说明「网通着，但不是本程序拨的」。

#### 🟢 A4. 时区改下拉（新增 `runtime/tz.rs` + `handlers/timezone.rs`）

**实机关键发现**：固件**没有 zoneinfo 数据库**（`/usr/share/zoneinfo` 不存在），
系统时区靠 `/etc/config/system` 的 `timezone`（POSIX TZ，如 `CST-8`）+ `zonename`
两个字段，外加 `/etc/TZ`。所以不能照 Linux 发行版那样 `ln -sf /usr/share/zoneinfo/...`。

- `GET/POST /api/timezone`：读当前值 + 12 个常用时区预置；写后**回读校验**；
- 系统时区不在预置列表里时**保留原值并置顶显示**，绝不自动覆盖；
- 写成功后同步 modemd 自己的 `[ntp] timezone`，避免「系统 +9、日志 +8」；
- 与 `/api/ntp/set` 的分工写进接口文档（一个管路由器，一个管 modemd 日志）。

#### 🟢 A5. 网络变化事件不再出现 `undefined`

用户报：日志里 `undefined · rsrq 从 undefined 变更为 undefined`。
两层原因：① 前端读错字段名（`time/old/val/diff` vs `at/from/to/delta`）；
② 后端若把「未上报」当值参与比较，就会拿空值当新旧值。

- 后端 `valid_signal` 已过滤（RSRQ=0 视为未上报）；补 3 条回归测试钉住
  「无效值不进比较」「首次只建基线」「值没变不报事件」；
- 前端字段名对齐（上一版已修，本版补测试防回归）；
- 记录一个**有意的取舍**：SINR=0 被当作有效值（物理上 0 dB 成立），
  而 RSRQ=0 视为未上报（模组普遍如此）—— 写进测试注释，不是遗漏。

---

### B. AT 串口三层保护（优先级排队 / 30ms 命令节奏 / 打开冷却）

用户原话：「不要把所有 AT 命令一起打给模块，需要硬性延时 20-30ms」。
根因不是「没限流」，而是**限流放错了层次**：早先只在「两次占用串口之间」等，
而一次逻辑操作里往往连发 3~5 条（读参数 3 条、读短信 5 条），这几条中间零间隔。

#### 🔴 命令节奏下沉到「每一条」（`at/pacing.rs`，新增模块）

- `AtPort::command_cancellable` 里**发送前**检查「距上一条写完是否够 30 ms」，
  不够就睡够。所以不管一次操作里连发几条，模组收到的命令之间都有 30 ms。
  实机后果对照：应答被截断（只收到半个 `+CSQ: 22,99`）、偶发 `ERROR`、
  严重时 AT 口假死到要重启模组。
- **30 ms 取自上游实机验证值**：同作者 `luci-app-fm350` 的
  `rust/src/at/mod.rs` 里 `MIN_CMD_GAP = 30ms`（20 ms 零星丢 `OK`，30 ms 稳定），
  本项目同平台直接沿用。
- 语义是「两次**发令时刻**之差」：应答本身就慢的命令天然满足，不会被额外拖慢。

#### 🟢 优先级排队（`at/sched.rs`，新增模块）

原来的做法是一把裸 `Mutex`，谁先抢到谁先发。现在按业务重要性插队：

| 级别 | 谁在用 |
| --- | --- |
| `user` | `/api/at` 手动发指令、用户点刷新 —— **实时**，不让后台压住 |
| `signal` | 信号 / 注册态 / SIM 身份巡检 |
| `data` | 拨号取 IP、载波聚合、网络状态、模组状态 |
| `bulk` | 短信列表、锁网状态、小区扫描、参数读写、USSD、扫网 |

同级 FIFO 不饿死。**`user` 级不受窗口配额限制** —— 配额防的是程序失控，
不是限制人。

#### 🟢 打开失败冷却（`at/cooldown.rs`，新增模块）

串口打不开往往持续几十秒（模组重启、USB 重新枚举），而 6~8 个后台循环会同时来问。
没有冷却就是「每 200 ms 一次 `open` 全失败」：日志刷屏、USB 子系统被拖慢、CPU 空转。
现在失败后 5 秒内**直接返回带倒计时的错误**（`AT 口打开失败，4 秒后重试`），
不碰串口。这不是「假装成功」—— 页面据此显示「模组未就绪」而不是「流量 0」。

#### 🟢 冷门查询 TTL 缓存（`at/cache.rs`，新增模块）

| 数据 | TTL | 写后作废入口 |
| --- | --- | --- |
| PDP / 选网参数 | 15 秒 | `set_apn` / `delete_apn` / `set_pdp` / `set_netselect` |
| USB 网卡模式 | 30 秒 | `set_usbnet` |
| 组网模式偏好 | 30 秒 | `set_mode_pref` |

三条纪律：**失败不缓存**（一次串口超时不能把错误钉住整个 TTL）、
**写后必作废**（改了 APN 还读旧值比慢更糟）、**能强制刷新**（`?refresh=1`）。
拨号流程取 APN 一律走实时版（`params_refresh`），因为要拿最新配置去拨。

同时把 `params()` 的三条命令合并进**一次**端口占用（早先每条排一次队，
光排队间隔就比命令本身久）。

#### 🔴 实机暴露并修掉的两个真问题

1. **配额设得太紧，把串口堵死了**。原 `40 次 / 10 秒`（4 次/秒），而实机自然需求
   是 4~6 次/秒。结果 5 分钟内：`signal` 累计等 54 秒、`data` 等 64 秒、
   **用户手动 AT 等了 2.7 秒**，最长一次 **7.7 秒**。配额改为 `120 次 / 10 秒`，
   且 `user` 级不受限。改完复测：所有 `wait_ms` 归 0，
   `/api/modem/status` 从 **8 秒/次** 降到 **0.8 秒/次**。
2. **串口被上一轮遗留的临时实例占着**（`md3`，PPid=1 没被清理），
   导致新实例一直「发现模组 0 个」。已在验证脚本里改用 pid 文件精确停止 +
   停后确认 `/proc/*/fd` 不再持有 `ttyUSB*`。

#### 🟢 新增接口 `GET /api/at/sched`

暴露调度与节奏状态（各优先级服务次数 / 等待毫秒 / 被限流次数 / 打开失败次数 /
`cmd_gap_ms`），工具页 AT 调试台标题右侧有实时徽章。

#### 🟢 前端

- 工具页 AT 调试台加「串口调度」徽章：`空闲 / 串口中 / 排队 N / 限流 N`，
  悬浮看各优先级服务次数与阈值。
- `api.js` URL 编码加固（`encodeURIComponent` 不编码 `!'()*`，
  而 ucode 代理正则不放行）。

#### 验证

- `cargo test` **658 通过 / 0 失败**（本轮新增 14 个：节奏 6、调度 3、冷却 5）；
  `clippy --all-targets` **0 警告**。
- **实机**（ImmortalWrt SNAPSHOT / 6.18.55 / aarch64 / FM350-GL）：
  - `/api/at/sched` 返回 `cmd_gap_ms=30`、`limits.per_window=120`、
    `open.cooldown_ms=5000`；
  - 6 条并发手动 `AT`：`user.served=6`、`pacing.throttled=0`
    （每条 `AT` 往返约 80ms，天然满足 30ms 下限，保护在位但不白拖）；
  - 改配额前 `max_wait_ms=7745` → 改后 `max_wait_ms≤160`；
  - 生产服务验证后已恢复（`running`、FM350-GL 识别、默认路由未变）。

#### 未做实机验证

- **`pacing.throttled` 的增长场景**：需要一条「应答 <30ms 的快命令」才能触发，
  FM350-GL 上所有命令都慢于 30ms，所以实机只验证到「不触发」。逻辑正确性由
  单测 `consecutive_commands_are_all_paced`（5 条命令必有 4 个间隔）保证。
- 多模组并存时的调度（本机只有一台）。

### 1.0.0-6：全链路数据契约对齐 + 鉴权闸门修复 + 载波聚合实时采集

本轮以「页面能不能真的读到后端数据」为验收标准做了一次端到端审查：
启动真实 `modemd` 实例，逐个请求 **79 个** LuCI 实际调用的接口，核对返回信封与字段名。

#### 🔴 安全（两处，均属「配置看起来生效、实际没有」）

- **`require_luci_login` 从未真正生效（P0）**
  - 根因：普通 HTTP 请求由 `gateway.rs` 转给内部 tiny_http（`127.0.0.1:0`），
    `req.remote_addr()` 在那条链路上**恒为 127.0.0.1**；而 `auth.rs` 正是用它判断
    「本机请求免登录」。结果：默认 `listen = "0.0.0.0:9527"` 下，局域网内任何人都能
    无密码调用全部接口（改 IMEI、发短信、切飞行模式、改配置）。
  - 修法：网关在转发时**覆盖式**注入 `X-Real-IP`（先删除客户端自带的同名头，防伪造），
    鉴权闸门改读它。实测：非回环未登录 → 401；伪造 `X-Real-IP: 127.0.0.1` → 仍 401；
    ucode 代理（真回环）→ 200。
- **`admin/modemd_api/proxy` 缺 ACL 依赖（P0）**
  - 此前只有 `auth`，没有 `depends.acl` —— 任何已登录 LuCI 的用户（哪怕没被授予
    `luci-app-modemd`）都能调用这个直通后端的代理。已补 `depends: {acl: [...]}`。

#### 🔴 后端契约（失败时不再「什么都不回」）

- **handler 返回 `Err` 时连接上零字节（P0）**
  - `tiny_http::Request` 是**一次性**的：handler 带着 `Err` 返回时 `Request` 已被 drop，
    网关层再也写不了响应。页面只看到「modemd 无响应」，连一句原因都拿不到。
  - 逐个消灭了 10 个 handler 里的 30+ 处裸 `?`：`sim` / `signal` / `network_status` /
    `lock_status` / `wechat`(11 处) / `schedule_set|run|delete|toggle`。
    新增 `response::to_value` / `json_body` 两个「先序列化、再响应」的助手。
- **`/api/tasks` 的重复 `build_spec` 与 11 处 `need(&modem)?`**
  - 改成「先取模组、再组 spec」各一次，失败一律 `response::fail`。

#### 🟢 页面数据（此前恒为空 / 恒为 `-` 的根因）

- **后台采样循环对 LuCI 用户完全空转（P0，本轮最关键的「页面读不到数据」根因）**
  - 所有采样循环（signal / network / traffic / change）都以 `ws.active_count() > 0`
    为触发条件；而 **LuCI 前端只走 HTTP 轮询**（跨端口跨源，ucode 代理也转不了长连接），
    根本不连 WebSocket。于是 LuCI 用户一次采样都触发不了，`/api/snapshot` 的 `blocks`
    永远为空、`ages` 永远 `null`、`stale` 永远 `true`。
  - 新增「HTTP 客户端活跃度」：`AppState::last_http_seen`（AtomicI64）+ `viewer_active(60s)`。
    `/api/snapshot`、`/api/usbnet` 被调用即记一次时间戳。两类前端口径统一，
    「没人在看就别发 AT」的初衷仍然成立（关页面 60 秒后自动停）。
- **新增 `modem` 缓存块 + `modem.refresh` 任务**：概览页「SIM 卡信息」「模组硬件参数」
  两张卡片此前读的是 `ModemView`（只有 id/at_port/model/running），9 项全是 `-`。
  现在 `/api/modem/status` 的结果归档进 `blocks.modem`，前端改读它。
  后台按 5 分钟节流刷新，缓存为空时立刻补一次（首屏不必干等）。
- **`ModemStatus` 补 `imei`**：概览页要显示 IMEI，而它原先只在 `/api/imei`（改写域）里。
  用一条 `AT+CGSN` 只读补上，不碰备份台账与 Luhn 校验那套语义。
- **`dial` / `usbnet` 两个缓存块此前从未被写入**：`StateCache::KINDS` 声明了却没人填，
  快照里是「永远 null、永远过期」的占位块。现在 snapshot 顺手归档 `dial`（本地推断，
  不发 AT）、`/api/usbnet` 查询后归档。

#### 🟢 载波聚合：从「结构占位」到实时采集

- **新增专用模块 `model/ca.rs`**：`ca` / `ca_active` 此前恒为 `[]` / `false`（API.md 已
  自认是占位）。现按厂商分派指令并解析：
  - 广和通 `AT+GTCAINFO?` —— **逐字段照抄官方手册《FIBOCOM FM350 AT Commands v2.2》
    §11.1.16**（含 LTE/NR 两套字段序、`SCCn` 的三个前导字段、`scell_state` 语义、
    带宽编码表）；频段编码是「`50` + 频段号」的十进制拼接（`50512` → n512），
    不是减法（早期实现按 `- 5000` 算会得出 45512，被单测抓出）。
  - 移远 / 美格 `AT+QCAINFO` —— 本仓库无官方手册，**只认文本模式**（`"PCC"`/`"SCC"`
    与 `"LTE BAND n"`/`"NR5G BAND n<n>"`），列序一概不猜。
  - Telit `#CAINFOEXT` 明确返回 `None`（字段布局未移植），**不按错的列序编数据**。
  - 7 个单测覆盖：激活/未激活 SCC、NR 编码、只有主载波、乱码、未知指令。

#### 🟢 字段契约（前端读错名字，导致固定显示 `-` / `undefined`）

| 页面 | 前端原来读 | 后端实际返回 |
| --- | --- | --- |
| 概览 / 工具（波动记录） | `time/old/val/diff` | `at/from/to/delta` |
| 工具（运行日志） | `log` | `lines[]` |
| 短信（存储区） | `cur/counts` | `current/mem1_used/mem1_total` |
| 工具（USB 设备） | `vid_pid/driver` | `vid`+`pid`/`drivers[]` |
| 工具（PCIe） | `device.slot/vendor/device` | `devices[].bdf/vid/pid` |

另修：`POST /api/pcie/reset` 前端从不传 `bdf`（后端判空必失败）；PCIe 复位按钮现在
从 `devices[0].bdf` 取值。

#### 🟢 慢操作改走任务（避免撞 ucode 代理 20 秒超时）

新增任务种类：`modem.refresh` / `dial.connect` / `dial.disconnect` / `imei.set` /
`imei.restore` / `sms.storage.set`。前端对应改为 `task.run(...)` + 进度提示：
锁频段（重启射频）、拨号/断开、IMEI 改写/恢复、短信存储区切换。
同步接口保留（不破坏既有契约），只是页面不再走它们。

另修 `lock.js`：某分组一个频段都没勾时会下发 `bands: []`，被后端拒 → 整批
`Promise.all` 一起 reject、已成功的分组被误报失败。现在空分组直接跳过。

#### 🟢 数据真实性

- **`version.rs` 的 `rustc_version()` 恒返回 `"stable"`** —— 假值。新增 `build.rs` 在
  编译期执行 `rustc --version` 取真实版本，拿不到就写 `unknown`，不再拿假值充数。
  （首版实现把 `"rustc "` 前缀也带上了，端到端验证时看到 `rustc rustc 1.93.0`，已修。）
- **`modem_by_query` 的 `modem_id` 解析失败静默回落 0** —— 会把「锁 3 号模组的频段」
  悄悄执行成「锁 0 号」。现在给了但解析不了就返回 `None`（找不到模组）。
- **`POST /api/roam/set` 漏传 `on` 默认开启漫游** —— 国际漫游下等于悄悄放行流量。
  改为必须显式给值，给了但认不出也报错。
- **`/api/netscan` 把 6 位 NR-ARFCN 误判成 PLMN**（`627264` → MCC 627 / MNC 264，
  凭空编出一份不存在的运营商，真正的频点反倒留在 0）。改为按长度分派：
  5 位优先当 PLMN，6 位优先当频点。
- **两张互相矛盾的运营商表**：`cells.rs` 把 `46004/46008` 判成「中国广电」，
  `siminfo.rs` 判成「中国移动」—— 同一张卡两个页面显示两家运营商。
  现在只有一张表（`siminfo::operator_from_plmn`），`cells::operator_name` 是转发壳。
- **`select_key_temps` 取的是列表前三项**，与注释写的「按名称挑机身/5G基带/CPU」
  完全不是一回事（广和通按 sensor id 排序，前三项常是无关通路）。改为按名匹配，
  一个都对不上才退回前三项。
- **速率式模组的累计流量恒为 0**（广和通只报瞬时速率）→ 新增本地积分
  `RateTotal`，标 `estimated`，并在注释里如实写明「只在采样期间推进」的局限。
- `roam_applies` / `applies` 恒为 `true` → 改为「判据齐全才算适用」
  （`scope ∈ {domestic, global}`）。

#### 🟢 运行时健壮性

- **模组拔出后永不回收（P0）**：`manager.rs` 的 `discover()` 只增不删，后果是
  ① 幽灵模组永久堆积、串口 fd 泄漏；② 它的口一直留在 `known` 集合里，
  **同一台设备重插后再也发现不了**（只能重启 modemd）。
  新增 `reap_gone()`：只在「设备文件不存在」时回收（不用「打不开」当判据，避免把
  正忙/权限受限的好模组误删），按 `discover` 的顺序先回收再统计 `known`；
  回收的 id 由 `take_reaped()` 交给巡检循环清 `StateCache`。
- **`netprov/uci.rs` 外部命令无超时（P0）**：`uci` / `ifstatus` / `network reload`
  用 `Command::output()` 无限阻塞。卡住时组网线程永久挂起，连尾部的**拨号器对账**
  也不跑了 —— 掉线无法自愈且日志干净。
  新增 `run_with_timeout`（看护线程 + `recv_timeout` + 超时 kill），
  4 处调用全部覆盖，不引第三方依赖。

#### 🟢 启动速度（实机发现）

- **`discover()` 挡住 HTTP 启动，首次部署时页面 40 秒不可用（实测）**
  - `lib.rs` 里 `mgr.discover(true)` 同步跑在 `http::serve` **之前**。实机 FM350-GL 有
    7 个 `/dev/ttyUSB*`，只有 ttyUSB4 是 AT 口，其余每个都要等探测超时 ——
    从进程启动到 `/api/version` 可用整整 **40 秒**，这期间 LuCI 全部页面都是
    「modemd 无响应」。
  - 改为：HTTP 先起，首次发现放后台线程。实测**就绪耗时 0 秒**，模组列表几秒后自己填上
    （页面本来就在轮询 `/api/snapshot`）。
  - 顺带给 `discover()` 加了互斥锁：启动发现、周期巡检、页面 `?discover=1` 三方并发时，
    原先会各自看到对方还没插入的口，导致**同一台设备登记两次**（抢同一个 tty、应答串扰）。

#### 🟢 打包与部署

- `Makefile` 的 `DEPENDS` 从 `+luci-base +odhcp6c` 补全为：
  `rpcd` / `uhttpd-mod-ubus` / `uhttpd-mod-ucode` / `ucode-mod-fs` / `ucode-mod-math` /
  `netifd` / `wwan` / `usbutils` / `kmod-usb-net-{qmi-wwan,cdc-mbim,rndis,cdc-ether}` /
  `kmod-usb-serial-option`。少任何一个都不会编译报错，而是装到设备上才「打不开」。
- `init.d`：删掉永久空转的 `procd_add_reload_trigger "modemd"`（它监视的是不存在的
  UCI 配置 `/etc/config/modemd`；`config.toml` 的变更已由 `procd_set_param file` 覆盖）；
  启动前补 `mkdir -p /var/log`（精简固件上日志会静默丢）。
- `config.toml`：`[net]` 段补上代码一直在读、但默认配置里缺席的
  `dial_enabled` / `allow_roaming`；`reset_before_dial` 的两行注释改为如实标注
  「目前未实现」（此前注释写「默认开」，会误导排障）。
- `api.js`：`encodeURIComponent` 不编码 `!'()*`，而 ucode 代理的 `PATH_RE` 不放行它们。
  现补一层，避免将来新增带文本的查询参数时踩坑。

#### 🟢 清理

- `changes::int_param` 上移到 `http::request::strict_i64_param`（原先
  `signal_history` 要跨 handler 模块调它，属职责错位），原位置保留转发壳。
- 修掉两处指向已删除文件 `http/smsauth.rs` 的过期注释。
- `band.rs` 的两个 ARFCN 反推函数与 `cells::band_from_earfcn` 并存，已在注释里写明
  职责差异（字符串标签 vs 整数号）与「当前生产路径走哪一份」，避免被误当重复实现删掉。

#### 验证

- `cargo build` / `cargo test`：**616 通过 / 0 失败**（新增 20 个单测）。
- `cargo clippy --all-targets`：**0 警告**。本次改动的 40 个文件 `rustfmt --check` 全部合规。
- 端到端：真实启动 `modemd`（无模组），**79 个** LuCI 调用接口全部返回完整
  `{code,data,msg}` 信封；未注册路径 404 + 信封；鉴权三种场景（回环/非回环/伪造头）符合预期。
- 全部 LuCI JS（15 个文件）`node --check` 通过。
- **实机验证（ImmortalWrt SNAPSHOT / 内核 6.18.55 / aarch64_cortex-a53 + FM350-GL）**：
  - 交叉编译 aarch64-musl（`rust-lld` 作 linker）后在设备上实跑新二进制；
  - `/api/version` 返回 `platform: linux/aarch64`、`runtime: rustc 1.93.0`（真实版本）；
  - `blocks.modem` 拿到真实 SIM 身份：`iccid 89860xxxxxxxxxxxxxxx`、`imsi 46011xxxxxxxxxx`、
    **`imei 86933xxxxxxxxxx`**（本轮新增字段，来自 `AT+CGSN`）；
  - `roam_applies: false`（`roam_scope: "none"`）—— 修复前这里恒为 `true`；
  - `temp_key` 挑出 `ltepa_ntc` / `md_5g` / `soc_max` —— 修复前是不管名字的「列表前三项」；
  - `AT+GTCAINFO?` 在当前（未注册）状态下只回 `OK`、无 `PCC/SCC` 行 —— 解析器正确返回
    「未上报」，没有编造数据；
  - 鉴权闸门：非回环未登录 401、伪造 `X-Real-IP` 仍 401、回环放行；
  - 自动组网**没有**改写用户 UCI（`/etc/config/network` 时间戳未变，`modem0` 是用户手工配置）；
  - 全程未影响生产服务的网络（默认路由走有线 WAN，蜂窝当前未承载流量）。
- **仍未做实机验证**：CA 的解析分支在实机上只走到「空闲态无上报」这一条，**未拿到真实
  `PCC/SCC1` 报文**（需要拨号进入连接态 + 网络配置辅载波）；模组热插拔回收未实机复现。
  详见 `docs/PROGRESS.md` 的「本轮遗留」。

### 1.0.0-5：LuCI 完整对齐内置 WebUI + 二级菜单无下拉 + 滑动 TAB 栏 + 原生 SVG 信号与白色毛玻璃

- **二级菜单收敛与杜绝 404**：
  - 菜单统一收敛为 LuCI「Mobile Network」下的单一「5G 终端」二级菜单入口（`admin/modem/modemd`），别名指向 `overview`。
  - 保留全部 8 大子页面端点（`overview`, `dial`, `cells`, `lock`, `sms`, `tools`, `modules`, `service`）在 `menu.d` 中注册，但**省略 `title` 属性**：
    - **侧栏无下拉**：依据 LuCI `dispatcher.uc` 契约（只有带 `title` 的节点才进入导航树），无 `title` 的路由不会被渲染到侧边栏/顶栏菜单树中，彻底消除了侧栏下拉式菜单堆叠；
    - **直连绝不 404**：所有子页面 URL 均具备完整路由支撑，直接访问 `/admin/modemd/dial` 或从书签/历史记录进入均 100% 正常响应。
- **页内横向左右滑动 TAB 导航栏（Sliding TAB Bar）**：
  - 在 `fmt.js` 中实现统一的横向左右滑动 TAB 组件，支持鼠标滚轮横向滚动、左右微调浮动箭头、移动端触控滑动和当前激活项自动居中聚焦。
  - 每个 TAB 绑定真实的 LuCI 页面超链接（`L.url('admin/modemd/' + tab.id)`），切换 TAB 时真实切换子页面，解决“点击什么都是首页”的问题。
- **原生动态 SVG 图标与信号量程刻度条**：
  - 新增专用原生 SVG 图标库 `icons.js`。
  - 修复 LuCI `window.E` 对 `innerHTML` 的底层赋值处理，确保 SVG 图标与刻度条在 DOM 中原生解析渲染。
  - 渲染真实的 5 档蜂窝天线信号柱（按极好/良好/普通/较弱/无信号分级着色）以及 RSRP、SINR、RSRQ、RSSI 4 宫格物理量程平滑 SVG 刻度槽（`signalBar`）。
- **WebUI 跳转按钮去重**：
  - 移除概览页 Header 卡片内重复的跳转按钮，整页仅在横向滑动 TAB 栏最右侧保留单一权威的「`WebUI ↗`」跳转药丸按钮。
- **功能与数据全量对齐**：
  - 8 大业务模块及 10 项调试工具（AT 控制台、USSD、搜网、热插拔、IMEI 修改等）全面对齐内置 WebUI 数据源与操作行为。
- **实机全量验证**：
  - 目标设备 `192.168.10.1` 部署与 Playwright 自动化核验 100% 通过（8 大子页面直连 `is_404 = False`，无下拉子菜单，无控制台报错）。
  - 后端 Rust 单元测试（`cargo test --lib`）554 项测试全数通过。

### 1.0.0-4：LuCI 子菜单修复 + 魔改主题适配

- **恢复 8 个子菜单节点**（设备中心/拨号上网/基站扫描/锁网/短信/调试工具/支持模块/
  服务控制）：单入口 + hash Tab 依赖主题的视图解析，在魔改主题下不可靠；
  原生多子页形态在标准 LuCI 语义下稳定，菜单完整、功能对齐 WebUI。
- **概览页加「打开 WebUI ↗」**：与 :9527 原生 WebUI 共存期的新窗口跳转
  （端口从 config.toml 解析），页面本身不加载 WebUI 资源。
- **删除 main.js**（单入口 Tab 骨架，随形态调整退役）。
- **修复 ucode 代理**：`String.prototype.format` 不是 ucode API，改 `sprintf()`——
  之前代理必 500，LuCI 所有数据请求失败（`ApiError` 的直接根因之一）。
- **postinst 自愈**：升级后清 `/tmp/luci-indexcache*`、`/tmp/luci-menu.json`、
  `/tmp/luci-modulecache`，并 touch 全部插件 JS/menu/acl 文件 —— 修复
  「升级后菜单树/页面还是旧的」。
- 实测（aurora 主题）：8 页全部渲染、菜单完整、快照数据正常、proxy 500 清零。

### WebUI/LuCI 共存口径定稿（1.0.0-3）

- **LuCI 完整对齐 WebUI**：8 个 Tab（设备中心 / 拨号上网 / 基站扫描 / 锁网 / 短信 /
  调试工具 / 支持模块 / 服务控制）全部原生 LuCI JS 渲染，**不加载任何 WebUI 资源**
  （无 iframe、不引外部页面）。
- **共存期跳转按钮**：Tab 栏旁「打开 WebUI ↗」（新窗口，端口从 config.toml 解析），
  供过渡期对照；后续 LuCI 验证成熟后 WebUI 可退役。
- **删除 iframe 时代残留**：`modemd_auth.uc` 会话中转端点与 `menu.d` 对应节点移除
  （不再有嵌入页面需要会话 ID）；菜单只剩「5G 终端」入口与 `/api` 代理端点。
- 全仓无 Lua / CBI 残留（`find *.lua` 为空），LuCI 侧为纯 JS + ucode。

### LuCI 菜单 Tab 化 + 代理错误信息可排查（1.0.0-2）

- **页内 Tab 菜单**：8 个子页面合并为单一入口「5G 终端」，页内 Tab 切换
  （设备中心 / 拨号上网 / 基站扫描 / 锁网 / 短信 / 调试工具 / 支持模块 / 服务控制）。
  设备固件的 ucode dispatcher 没有 `type:tabs` 渲染，因此由 `view/modemd/main.js`
  做 Tab 骨架、**按需 `L.require` 动态加载各视图** —— 切 Tab 才加载对应 JS 与数据；
  Tab 状态记在 `location.hash`（`#md=dial`），刷新/分享链接可复现；
  切换时清空共享轮询的旧订阅（`store.reset()`），避免对已脱离 DOM 的节点 redraw。
- **代理错误信息可排查**：`api.js` 先取文本、按 content-type 判断再解析。
  此前 LuCI 会话过期（403 登录页 HTML）、端点缺失（404）或 ucode error500 时
  直接 `res.json()`，用户只看到
  `无法连接 LuCI 代理：Unexpected token '<', "..." is not valid JSON`；
  现在分别给出明确提示（会话过期 / 菜单未生效 / LuCI 代理异常 + HTTP 状态）。

### CI 增加 mediatek/filogic（aarch64_cortex-a53）APK 通道

实测：`apk --print-arch` 为 `aarch64_cortex-a53` 的固机装不上 armv8 打出的
`aarch64_generic` 包（`error: uninstallable / arch: aarch64_generic`），而临时改
`/etc/apk/arch` 会让已装的 cortex-a53 包全部变 uninstallable。

所以把 `TARGET` 从 workflow env 下放到 job 级 matrix，新增 `apk-filogic` 通道。
两档 APK 内容一致（同一份静态 musl 二进制、无内核模块），只是包架构标签不同；
按设备 `apk --print-arch` 选：`aarch64_cortex-a53` → filogic，否则 → mainline。

### LuCI/后端异步化架构：任务系统 + 缓存快照

慢操作从 HTTP 请求线程里整体挪走，「打开页面」不再等于「发一串 AT」：

- **异步任务**（`http/tasks.rs` + `POST/GET /api/tasks*`）：扫小区、扫网、AT、USSD、
  发短信、锁频段、切 USB 模式、重启模组统一走「提交 → 立即返回 task_id → 轮询」；
  同 `kind+模组+参数` 去重（狂点按钮安全）、协作式取消（AT 读取每 100ms 检查）、
  完成 10 分钟后回收。
- **只读状态缓存**（`http/cache.rs`）：信号/网络/小区/锁网的最近一次结果按
  `(种类, 模组)` 归档；同步接口、后台采样循环、异步任务完成三个写入点共用。
- **`GET /api/snapshot`**：一个请求拿齐概览要显示的东西（旧值 + 年龄 + 在跑任务），
  页面首屏零 AT；需要新值提交任务，完成后重拉快照。
- **LuCI 重构为原生 JS+Ucode（不再 iframe 嵌 WebUI）**：新增代理控制器
  `modemd_api.uc`（同源转发 + 端口自适应）与 8 个原生视图
  （设备中心 / 拨号 / 基站 / 锁网 / 短信 / 调试工具 / 支持模块 / 服务控制）；
  共享模块 `modemd/{api,task,store,fmt}.js`——store 是可见性感知的单例轮询
  （后台暂停、切回立即刷新），task 封装「提交+退避轮询+取消」。
- 设备实测：`signal.refresh` 任务 100ms 完成、快照 `ages.signal` 归档；
  未知任务种类与取消不存在的任务都有正常 JSON 回复。

### WebUI 与后端契约对齐 + 取消短信访问密码

#### 取消短信访问密码（用户要求）

- 删除 `http/smsauth.rs` 与 `/api/sms/{auth,unlock,lock,password}` 四个接口；
- 短信读 / 发 / 删 / 存储区、短信推送、微信机器人都只受**外层登录闸门**
  （`[ui] require_luci_login`）约束；
- WebSocket 不再接受 `sms_token` 查询参数， `sms` 推送与 `smscount` 一样走普通广播；
- 配置里的 `[ui] sms_password` / `sms_password_required` 不再读取（旧配置可保留，无副作用）；
- 前端删除锁屏卡、解锁/锁定按钮、「访问密码」子页签与令牌存储。

#### 前后端字段对齐（WebUI 拿得到、拿得对）

| 接口 | 修了什么 |
| --- | --- |
| `GET /api/network/status` | 补 `duplex` / `reg_5g` / `reg_eps`（CS/PS 都未注册时才补查）/ `serving` / `serving_count` / `neighbour_count` / `scanned_at` / `ca` / `ca_active` / `roam_allowed` / `roam_scope` / `airplane` / `cfun` / `raw_net_info`；`scanned_at` 不再把「没扫过」写成「刚刚」 |
| `GET /api/cells` | 补 `ca_cmds`（按厂商给真实 CA 查询指令）/ `ca_note`；扫描后补运营商 / 网络接入信息 / 注册状态三行 |
| `GET /api/usbnet` | 视图重写：`value/name/modes[]/mode_cmds/blocked_modes/need_reboot/cmd/note`，取值统一字符串；读不到当前值时不再显示「未知（）」 |
| `GET /api/dial` | `usb.host_iface_hint` 字段名对齐（原 `host_iface`），补 `catalog_model/catalog_usb/catalog_pcie` |
| `GET /api/session` | 补 `required` / `login_url`；未登录 401 响应体补 `need_login` + `login_url` |
| `GET /api/modems` | 补 `cell_scan_at`（最近一次基站扫描时间，没扫过就没有） |
| `GET /api/wechat` | 视图补齐：`stage/stage_cn/stage_msg/qr_png/qr_left/verify_required/last_sender/session_ready/heard_message/has_context/listen_on/last_incoming(+at)/last_send/sent_ok/sent_fail/token_mask/default_template` |

#### 修掉的真 Bug

- **短信列表为空被当成读失败**：`AT+CMGL=4` 回 `OK` 但零条时，后端报
  `PDU 模式下未解出任何短信`，页面永远弹「读取失败」。现在空列表返回 `[]`（code 0），
  只有固件明确回 ERROR 才算失败。
- **扫码登录没有二维码**：新增 `qrcode` 依赖（仅核心，无 image/svg 特性），
  后端把二维码内容离线画成 **SVG data URI**，页面 `<img>` 直接显示。
- `cell_scan_at` / `scanned_at` 的「空结果填现在」问题（会让页面把没扫过显示成刚扫过）。
- 前端：USB 网卡读不到当前值时下拉置灰并显示原因；主机侧网卡在拨号状态为空时
  回落到 USB 识别结果；补内联 favicon（消除唯一的 404 控制台错误）。

### 移植 QModem 频段表：型号 → 支持频段（92 条）

`/api/modules` 的 `bands` 之前是空的（页面显示「未提供」），锁频段页的
`band_groups` / `declared` 也没有后端赋值 —— 前端这两块实际是死的。本轮补齐：

- **内嵌频段表** `rust/src/modem/modem_bands.json`（92 条记录 / 89 条有频段 / 68 条支持 NR），
  逐字取自 QModem `modem_support.json`，按 MPL-2.0 使用（文件头注明）。
- **`modem/bands.rs`**：四组频段（WCDMA / LTE / NR-NSA / NR-SA）、型号匹配
  （去分隔符 + 最长优先 + ≤4 字符词边界 + 别名同权）、三态说明
  （没登记 / 上游没写 / 该制式没记录），以及按型号自动排版的 `groups()`。
- **`/api/modules?model=` / `?modem_id=`**：`plan.bands` 给出四组频段，
  `plan.bands_note` 说明来源与个数（`?modem_id=` 的匹配键带上 ATI 原文，别名也认）。
- **`/api/lock/status`**：`band_lte` / `band_nr` / `band_nr_nsa` 新增 `declared` +
  `declared_note`（出厂口径，与运行期 `capable` 并存对照）；新增 `bands` 与
  `band_groups`（清单 ∪ 设备能力 ∪ 当前锁定值；分两套 NR 列表的方言才出 NSA / SA 两组）。
- **前端**：修正「没有频段记录」提示里的旧路径（`module/kernel/…` → `rust/src/modem/…`）。

### 全量移植：逐型号 AT 指令面 + 厂商 × 平台变体表

- **AT 指令面**（`rust/src/modem/atplan.rs`）：按上游 `modem_at_plan.go` 的字段契约
  （`key/label/cmd/candidates/enable/parser/format/on/source/note`）输出 20+ 项能力：
  身份、服务小区、邻区、信号、注册、运营商、CA、温度、电压、流量、APN、PDP 地址、
  AMBR、ICCID、IMSI、时钟、短信读写、扫网、锁频段、组网模式、USB/PCIe 模式、IMEI、USSD、飞行模式。
- **各厂商单独模块**：新增 `VendorSpec::at_plan_commands()`，默认给基础 3GPP；
  14 家厂商在各自 `modem/vendor/<厂商>.rs` 里声明邻区/CA/ICCID/IMEI/锁频段/USB 等差异
  （如移远 `QCAINFO` + `AT+EGMR`、广和通 MTK `AT+EGMREXT`、华为 `AT^PHYNUM`、
  鼎桥 `AT^MONNC`、芯讯通 `AT+CNBP?`、Sierra `AT!USBCOMP?`…），加厂商只动一个文件。
- **厂商 × 平台变体表**（`rust/src/modem/variant.rs`）：移植上游 22 条 `PlatformVariants`
  与家族/平台档判定（含 lte 旧款、`EM12` 词边界、MT5700→鼎桥归口），输出
  `platform_variant` / `platform_variant_note` / `lock|usb|mode|imei_dialect`。
- `/api/modules?model=` / `?modem_id=` 全部改走 `atplan`，`commands` 只含有实现依据的条目，
  未接入的进 `gaps`（带原因）。

### 对齐上游：支持清单从 7 个骨架型号扩到 91 个（QModem）

`GET /api/modules` 之前是重构留下的骨架（7 条、裸数组），而前端「支持模块」页按
`{count, vendors, models}` + 逐型号 `plan` 渲染 —— 页面实际是坏的。本轮按上游
（Go 版 `module/catalog/modem_catalog.json` + `apiModules`）补齐：

- 内嵌 QModem `docs/support_list.md` 全量 91 个型号（12 家厂商、41 个有 PCIe），
  数据与解析按 MPL-2.0 使用（见 `rust/src/modem/catalog.rs` 文件头）。
- `/api/modules` 返回 `{count, usb_count, pcie_count, vendor_count, vendors, models}`；
  `?model=` / `?modem_id=` 返回 `{plan}`，含 `in_catalog / usb_modes / pcie_modes /
  usb_text / pcie_text / platform / commands / gaps`。
- `commands` 只列**有出处的**模式切换 AT（QModem 指令脚本里的 `AT+QCFG="usbnet",N` 等），
  没登记的模式进 `gaps` 并附原因，**不编造**。
- 型号识别做「最长关键字优先 + 去分隔符归一」：`FM350-GL` 命中 `fm350-gl`（USB RNDIS /
  PCIe MBIM），不会误命中更短的键；不在清单里明确 `in_catalog=false`。

### 重大变更：删除 `third_party/`，QMI/MBIM 拨号全部内化

四家厂商的 C 拨号器（quectel-CM / fibocom-dial / meig-cm / simcom-cm）与 8 个厂商内核模块
整目录移除，仓库从 8.0M 降到 2.5M；`modemd` 现在自己完成 QMI / MBIM 数据呼叫，
`[net] host_dial_bin` 默认 `"native"`，不再安装任何 `/usr/lib/modemd/*` 二进制。

- **补齐 QMAP**：`runtime/dialer/qmi.rs` 新增 `WDA Set Data Format`（TLV 0x10/0x11/0x12/0x13/
  0x15/0x16/0x17）与 `WDS Bind Mux Data Port`（TLV 0x10/0x11/0x13），逐条对齐 quectel-CM
  的 `WdaSetDataFormat()` / `WdsSetQMUXBindMuxDataPort()`（`QUECTEL_UL_DATA_AGG` 未开，
  故不发 0x19/0x1B/0x1C）。mux id：单承载 `0x81`，多承载 `PDN + 0x80`；端点类型 USB=2 / PCIe=3；
  聚合窗口读 `qmap_size`，缺省 16 KiB。
- **控制口扩到三种驱动**：`find_control()` 依次反查 `/dev/cdc-wdmN`、`/dev/GobiQMIx`、
  `/dev/mhi_QMIx`，都按 sysfs 对齐设备，对不上就报错而不是猜 `cdc-wdm0`。
  GobiNet 跳过 WDA（驱动自己设数据格式，与 `GobiNetCM.c` 同口径）。
- **MBIM 不再受厂商限制**：`vendors::supports()` 对 MBIM 恒真（Basic Connect 是标准协议），
  Fibocom / SIMCom 的 MBIM 模组不会再因为「厂商 C 源码里没有 `mbim-cm.c`」被拒。
- **删除死代码**：`runtime/dialer/proc.rs`（321 行外部进程托管）、`DialerPlan` / `plan_dialer` /
  `DialerSupervisor::status`、`VendorSpec::dialer_bin()` 及 4 处实现、`files/lib/netifd/proto/quectel.sh`
  （quectel-CM 的 netifd 包装）、`Makefile` 的 `cc_dialer` / 四个 `MODEM_PRO_*` 菜单 / 安装规则、
  CI 的 `third_party` 拷贝与四个 `CONFIG_MODEM_PRO_*`、`.gitattributes` 与脱敏脚本的
  `third_party` 例外。
- **配置兼容**：旧值 `host_dial_bin = "quectel-CM"` 等不会报错，只告警一次并按内置实现跑；
  `proto=quectel` 的接口不再可用，改用 `proto=modem_pro`。
- **QMAP 收尾**：`Set Data Format` 应答里的 `rx_urb_size`（TLV `0x16`）现在会回读并
  覆盖请求值（与驱动 `rx_urb_size` 对齐，和 quectel-CM 同口径）。
- **无硬件也能跑会话**：`qmi_link` 支持注入可读写句柄，新增假设备（socketpair）端到端用例，
  逐帧校验 `CTL Sync → 3×ClientID → WDA SetDataFormat → BindMux(v4/v6) → Start(v4/v6) → Runtime`
  的顺序、client id 归属与 TLV 内容。
- **验证**：`cargo test` **573 全绿**、`cargo clippy --all-targets` 与 `cargo build` 零告警。
- ⚠️ **真机未验证**：测试机 FM350/RW350 是 RNDIS，没有控制口。QMI（含 QMAP）、MBIM、
  PCIe/MHI 三种数据面仍需实机跑通（验证矩阵见 `docs/PROGRESS.md` §41.4）。

### 清理：移除误入库的 `downloads/fluxdown`

`downloads/fluxdown/FluxDown-0.5.7-linux-x64/`（一个与项目无关的 FluxDown 桌面应用二进制
集）是在「优化 Actions 编译速度」那次提交里被顺手 `git add` 进来的，已从仓库删除，并把
`/downloads/` 加进 `.gitignore` 防止再犯。（历史里仍留有该二进制；如需彻底抹除需重写历史。）

### 修复：主线 APK 通道编 fibocom-dial 报「隐式声明 / 裸 return」

上一轮 CI 24.10 IPK 通道已绿（产出 `modem-pro_1.0.0-r1_aarch64_generic.ipk`），
只剩**主线 APK（snapshots）** 通道挂在编 `fibocom-dial`：

```
third_party/fibocom-dial/src/main.c:927:  error: 'return' with no value, in function returning non-void [-Wreturn-mismatch]
third_party/fibocom-dial/src/main.c:1447: error: implicit declaration of function 'requestGetSIMCardNumber'
third_party/fibocom-dial/src/main.c:1449/1450: error: implicit declaration of function 'requestSimBindSubscription_*'
third_party/fibocom-dial/src/QMIThread.c:2344: error: implicit declaration of function 'get_private_gateway'
```

- **根因**：这份广和通老代码**先调用后定义**（无函数声明）**且**有非 void 函数里的裸
  `return ;`。老 gcc（24.10 通道的 gcc-13）只当警告，但 **主线 snapshots 的 gcc-14 已把
  `implicit-function-declaration` 与 `return-mismatch` 默认当错误**（GCC 14 的 C99+ 模式
  默认报错变更），于是挂了。
- **修复**：顶层 `Makefile` 给 fibocom-dial 的额外 CFLAGS 加 `-std=gnu89`——gnu89 模式下这
  两类都不再是错误，**一个开关同时解决两类**。该开关**只能单给 fibocom**：quectel 用了
  C99 的 for 内声明，加 gnu89 反而报错；meig / simcom 在 gcc-14 语义下无需。
- **为何不加 `-Wno-error=return-mismatch`**：gcc-14 把该诊断拆成了 `-Wreturn-mismatch`，这个
  选项名在 gcc-13（24.10）里**不存在**，传进去会被 gcc-13 判为参数错误、反而把已绿的
  24.10 通道编挂。
- **本地验证**：装 gcc-14 复现并确认 `-std=gnu89` 单独即可编过；再用 SDK 的 gcc-13 强制
  开启 gcc-14 那套默认报错逐家核对四拨号器——仅 fibocom 需要该开关，且 fibocom 加 gnu89 后
  在 gcc-13 上完整编译并链接成 aarch64 静态二进制，24.10 通道不回归。
- **文档**：`third_party/fibocom-dial/SOURCE.md` 按上述实测修正「要加的开关」表。

### 修复：云编译把 luci-base 当构建依赖，连带去编 luci 组件而缺头文件

上一轮 CI 两个通道都挂在「编译 modem-pro」：日志里先编 `feeds/luci/modules/luci-base`
（及其 `contrib/package/csstidy`），接着编 `ucode-mod-html` 报
`ucode/module.h: No such file or directory`、`rpcd-mod-luci` 报缺 `netlink/msg.h`、
`lucihttp` 报缺 `lua.h`。

- **根因**：`modem-pro` 的 `DEPENDS:=+luci-base +odhcp6c` 本是**运行时**依赖，但 OpenWrt
  执行 `make package/modem-pro/compile` 时也会先把这些依赖编出来；而流水线用
  `feeds install -p luci luci-base` 装的是 **luci feed 的真实 luci-base**，于是连带拉出整串
  luci 组件去编译。这些组件要 base feed 的头文件（`lua.h` / `ucode/module.h` /
  `netlink/msg.h`），而 **SDK 里没有 base feed**，必然失败。
- **修复**：feeds 只拉本地 `modem_pro` / `ci_stub` 两个 feed（**不拉 `packages` / `luci`**），
  `luci-base` 与 `odhcp6c` 均改由 `ci_stub` 的 stub 提供（只声明包名、不编任何东西、
  不发布），`make package/modem-pro/compile` 因此不再连带去编任何 luci 组件。
- **stub 细节**：stub 的 Makefile 必须 `include $(INCLUDE_DIR)/package.mk`（否则
  `make -r DUMP=1` 无 dumpinfo 目标、feeds 索引里查不到包名，扫描静默失败），
  并补全 `PKG_VERSION` / `PKG_RELEASE`。
- **本地复现验证**：24.10.8 SDK 上按新流程完整跑通，产物
  `modem-pro_1.0.0-r1_aarch64_generic.ipk`，control 声明 `Depends: libc, luci-base, odhcp6c`；
  全程只编 toolchain / 两个 stub / modem-pro，未再出现任何 luci 组件。

### 编译提速：Rust 后端改宿主工具链直编（跳过 rust 源码编译）

原云编译最大耗时是 OpenWrt 从源码编译 `feeds/packages` 的 rust/host 工具链
（rustc + LLVM，冷启动 1~2 小时）。现改为**宿主 rustup 工具链直接交叉编译**：

- **顶层 Makefile**：新增 `MODEM_PRO_HOST_RUST` 开关（默认 `0` 保持原行为）。
  - `0`：走 `feeds/packages/lang/rust/rust-package.mk` 的 `Build/Compile/Cargo`（本地适用，不变）；
  - `1`：宿主 `cargo build --release --locked --target aarch64-unknown-linux-musl`，
    链接交给工具链自带 `rust-lld`（宿主 cc/ld 处理不了 aarch64 目标文件），
    产出**静态 musl 二进制**（无运行时 libc 依赖，同架构固件通用），完全不依赖
    `feeds/packages/lang/rust`；
  - 版本号注入改走 `CARGO_PKG_VARS`（上游配方自带行首 `+`，外层再叠会展开出
    多余的 `+VAR=...` 被 shell 当命令执行）；
  - 新增 `MODEM_PRO_CARGO_TARGET_DIR`，可指向稳定目录以复用编译产物。
  - 开关取值有**两种等价写法**（见 `docs/BUILD.md`）：命令行/环境变量
    `MODEM_PRO_HOST_RUST=1`，或 `.config` 符号 `CONFIG_MODEM_PRO_HOST_RUST=y`。
  - **关键顺序坑（务请注意）**：OpenWrt 的**包元数据扫描在 `feeds update/install`
    期间就跑**（子 make 在每个包目录下 `make -r DUMP=1`），它读 `.config` 决定各包的
    `Build-Depends`，结果落进 `tmp/.packageinfo` 并被后续 make 复用，**改 `.config`
    也不会触发重扫**。因此开关必须在 feeds 扫描**之前**就可见，否则扫描会把
    `rust/host` 记成 `modem-pro` 的构建依赖，之后无论怎么设都照样去源码编 rustc + LLVM。
- **云编译流水线**（`.github/workflows/build.yml`）：
  - 新增「安装宿主 Rust 工具链」步骤（`rustup` minimal profile +
    `aarch64-unknown-linux-musl` target，版本由 `RUST_TOOLCHAIN` 固定）；
  - 缓存由 `dl/` + `staging_dir/host*` 改为 `~/.cargo/registry` + `~/.cargo/git` +
    `.cargo-target`，key 绑定 `rust/Cargo.lock` / `Makefile` / 本文件哈希；
  - feeds 只拉本地 `modem_pro`/`ci_stub` 两个 feed（不拉 `packages`/`luci`），也不安装 `lang/rust`；
  - **在 feeds 扫描之前**就把选包与开关（含 `CONFIG_MODEM_PRO_HOST_RUST=y`）写进
    `.config`；并把 `MODEM_PRO_HOST_RUST=1` 提到 **job 级 `env`**，让 feeds 扫描阶段
    也带上该开关——这是绕开 rust/host 源码编译的关键；
  - 编译步骤只再传 `MODEM_PRO_CARGO_TARGET_DIR`（指向工作区 `.cargo-target` 复用产物）；
  - 超时由 240 分钟下调到 60 分钟，后又放宽到 90 分钟（编 vendor 拨号器 + kmod 需要余量）。
- **文档**：`docs/BUILD.md`、`docs/INTEGRATION.md` 补充两种构建方式与切换方法。

### 云编译流水线 + LuCI 菜单汉化

- **LuCI 菜单硬编码中文**（`menu.d/luci-app-modemd.json`）：`5G 终端` /
  `设备中心` / `服务控制`（独立调试台页面本来就是全中文）。
- **GitHub Actions 云编译**（`.github/workflows/build.yml`）：官方 SDK 编译，
  只要 Arm64 一档（`armsr/armv8` → 包架构 `aarch64_generic`，Cortex-A53 及以上通用）：
  - 主线 APK（snapshots，25.x）与 24.10 IPK（opkg）两档矩阵并行；
  - 流程细分：依赖 → stub → SDK → 缓存 → feeds → 选包 → 编译（V=s）→
    产物 + SHA256 → artifact；失败按 step 定位；
  - Rust 后端构建方式见上方「编译提速」条目（已改宿主工具链直编）；
  - SDK 无 base 包源码，`DEPENDS:+luci-base +odhcp6c` 均为运行时依赖，用本地 stub feed 满足（不发布）；
  - 推 `v*` 标签自动创建 Release 并上传两档产物与校验和。

### 修复：GitHub Actions 首跑失败（feed 名 / 元数据污染 / 拨号器开关）

- **feed 名带连字符被拒**：`src-link modem-pro` / `ci-stub` 的 feed 名含 `-`，
  被 `scripts/feeds` 的 `(\w+)` 解析拒绝（报 `Syntax error in feeds.conf.default`）；
  改为不带连字符的 `modem_pro` / `ci_stub`，包名 `modem-pro` / `odhcp6c` 保持不变。
- **feed 元数据污染**：`src-link` 直接指向仓库根会扫到解压进去的 `sdk/`
  （OpenWrt SDK 自带数百个包 Makefile），造成同名冲突；改为指向最小 feed
  包装目录（只放包体本身所需文件），并去掉 `feeds install -a`，
  改精确安装 `rust` / `luci-base` / `odhcp6c` / `modem-pro`。
- **feed 源禁止 symlink**：最小 feed 目录里用 `ln -sfn` 把仓库根 `Makefile`
  软链进 feed，OpenWrt 的 feed 扫描对 symlink 源 Makefile 处理有 bug，会把
  `$(eval $(call BuildPackage,modem-pro))` 当成路径嵌进生成的
  `.files-packageinfo.mk`，报 `target pattern contains no '%'` 而失败；
  改为 `cp -r` 真实复制，并 `find -name target -exec rm` 清掉混入的
  Rust/内核模块构建产物（避免体积爆炸与平台残留污染 feed 索引）。
- **拨号器开关从未接线**：独立 `Config.in` 的四个 `MODEM_PRO_*` 开关此前从未被
  `Makefile` `source`，导致 `make defconfig` 丢弃 `CONFIG_MODEM_PRO_*=y`、
  `Build/Compile` 的 `$(if $(CONFIG_MODEM_PRO_*),...)` 全为假、拨号器一个都不编译；
  已内联进 `Makefile` 的 `Package/modem-pro/config` 块（删除独立 `Config.in`）。
- **消除 Node.js 20 弃用警告**：GitHub Actions runner 已弃用 Node 20，
  `actions/checkout@v4` / `actions/cache@v4` 被强制在 Node.js 24 上运行并报警告；
  将全部动作升级到 Node 24 兼容版本——`checkout@v7`、`cache@v6`、
  `upload-artifact@v7`、`download-artifact@v8`、`softprops/action-gh-release@v3`
  （input 语法向后兼容，仅升版本号）。
- **修正 feed 扫描失败的根本原因**：此前把失败归因于「symlink 源 Makefile」，实测
  仓库内无任何 symlink，真正根因是 **包 Makefile 被放在了 feed 根目录**
  （`feeds/modem_pro/Makefile`）。OpenWrt 的 `scripts/feeds` 扫描（scan.mk 的
  `find | grep 'call BuildPackage' | sed` 截断逻辑）要求包 Makefile 位于 feed 的
  **子目录**下（`feed_dir/<pkg>/Makefile`），否则会把 `call BuildPackage` 的 grep 输出
  （含 `Makefile:$(eval ...)`）误当成包路径，导致 `.files-packageinfo.mk` 报
  `target pattern contains no '%'`、`install -p modem_pro modem-pro` 报
  `No feed for package 'modem-pro' found`、defconfig 丢弃 modem-pro 配置。
  修复：把包整体复制到 `$FEED_DIR/modem-pro/` 子目录（Makefile 随之进入子目录，
  其内 `$(CURDIR)/rust` 等相对引用仍成立）。本地已用 scan.mk 的 sed 逻辑复现验证。

### NTP 时间同步：插件自己校时，支持设置时区

新增 `rust/src/runtime/ntp.rs`（SNTP 客户端 + 后台同步循环）与
`rust/src/config/ntp.rs`（`[ntp]` 段）。要点：

- **节奏**：启动 3 秒后首试，失败每 3 秒重试（最多 10 次）→ 退避每 1 分钟
  （最多 10 次）→ 常驻每 20 分钟；一个周期内依次尝试全部服务器
  （默认 `ntp.tencent.com` / `time.cloudflare.com` / `ntp.aliyun.com`）。
- **校准语义**：同步成功只校准**插件自己的时间源**（`logging::time::unix_now()`，
  日志 / 定时器 / 配额全用它），不改系统时钟；一直失败偏移保持 0，
  时间回退为设备时间（要求的降级路径）。算偏移用未校准的系统时间，防自激；
  originate 回显校验 + kiss-o'-death 拒收 + 往返时延 >5s 判不可信。
- **时区**：`[ntp] timezone` 接受 IANA 名（`Asia/Shanghai`）或 POSIX 写法
  （`CST-8`），优先于 `TZ` / `/etc/TZ`；页面「改时区」运行期立即生效并写回
  配置文件。
- **接口**：`GET /api/ntp`（状态）、`POST /api/ntp/sync`（立即同步）、
  `POST /api/ntp/set`（改时区）；状态变化广播 `ntp` WS 消息。
- **前端**：概览页新增「时间同步」卡片（本机时间 / 来源 / 偏移 / 阶段 /
  成败计数 / 服务器 + 立即同步 / 改时区 / 刷新）。

### 拨号器托管：QMI/MBIM 的数据呼叫交给 modemd 看着

阶段 6。此前 QMI/MBIM 模组的 `/api/dial/connect` 是**明确报错**的：
「数据呼叫归主机侧厂商拨号器（拨号器托管属阶段 6，尚未迁移），请手动运行」。
所以 FM350/RW350 这类模组在页面上点「拨号」只会拿到一句红字，用户得自己
ssh 上去敲 `quectel-CM -s <apn>` —— 而配置注释里明明写着「modemd 每轮组网
循环把它拉起、看着、退出自动重试；模组拔了/手动断开就停掉」。

新增 `rust/src/runtime/dialer/`，两层结构与其它域一致：

- **`mod.rs` 纯策略 + 编排（22 个单测）**：命令行怎么拼、该不该跑、怎么收。
  - `plan_dialer()` 把配置注释里散着的口径收成一处：**MBIM 一律
    quectel-CM**（只有它实现了 `mbim-cm.c` + `SOFTWARE_MBIM`，而它认 MBIM
    靠的是标准 USB CDC 描述符、与厂商无关）；MBIM **必须有 APN**（没 `-s`
    拨号器直接 `exit(-404)`）；`-i` 给**父口**（QMAP 的驱动属性在 `wwan0`，
    数据在 `wwan0_1`）；`-n <pdn>` 只传给 quectel-CM / fibocom-dial / meig-cm，
    **simcom-cm 不认**；`host_dial_pdn=0` 时先按网卡名反推（`wwan0_1` → 1），
    反推不到但驱动是 QMAP 就兜底传 1。
  - `reconcile()` 是「看着」那一半：模组没了 / 手动断开 / 拨号总开关关掉 /
    非 QMI-MBIM 类型 → 收起；计划变了（换了 APN 或网卡）→ 停下旧的起新的；
    **计划没变就一个动作都没有**（已配好的模组每轮必须是空操作）。
  - `reap_dead()` 是「退出自动重试」那一半。没有它是个隐蔽的坑：拨号器退
    一次后 slot 还在、计划也没变，`reconcile` 会认为「已经在跑同一个计划」
    而跳过 —— 于是再也不起来。测试抓不到这种 bug，因为单测没有真进程。
- **`proc.rs` 薄 IO（7 个单测）**：拉起、探活、按**进程树**收。

  **为什么不按进程组收**：配置注释的原话是「按进程组一起收（连它拉起的
  busybox udhcpc 一起）」。进程组是对的，但纯 `std` 拿不到 `setsid()` ——
  那要引 `libc`，而 `Cargo.toml` 的「依赖刻意保持精简」不想为它破例（这个
  仓库现在连 `libc` 都没直接依赖）。所以改成扫 `/proc` 收进程树、按**正
  pid** 发信号，三个好处：不依赖 `setsid` 到底 fork 不 fork（fork 了 pid
  就对不上）；不依赖 BusyBox `kill` 收不收负 pid；收的是**真实的那棵树**
  （拨号器 → udhcpc → 它再起的脚本）而不是「碰巧同组的所有进程」。

  ⚠️ `/proc/<pid>/stat` 的第 2 个字段是**可含空格与括号的 comm**，不能按
  空格切 —— 从最后一个 `)` 之后开始切才对。`ppid_of()` 就是干这个的。

  ⚠️ 还有个更隐蔽的坑：**`spawn` 必须把 `Child` 交回去**。Unix 上父进程不
  `wait` 的话，子进程退出后会变成僵尸，`/proc/<pid>` 依旧存在 —— 那时
  「探活」会永远认为它活着，「退出自动重试」彻底失效。所以 `alive()`
  连状态字段一起看（`Z` 算不在），`Slot` 存 `Child` 而不是只存 pid，
  `stop` 在 KILL 之后也要等到树真的干净（不然调用方紧接着 `reap` 时
  进程还在死，`try_wait` 返回「还活着」就不收了）。单测
  `zombie_is_not_alive` 起一个立刻退出的子进程、故意不 wait，把这条路钉住。

接线：

- `netprov/provision.rs::run_once` 末尾调拨号器托管，**放在 UCI 之后**：
  先把接口建好，拨号器起来时网卡才在（QMAP 的 `wwan0_1` 要等 netifd 拉接口）。
- ⚠️ 顺手修了一个真 bug：`run_once` 开头是
  `if !auto_wan || !uci::available() { return; }`，拨号器托管原本会被它
  连带跳过 —— `auto_wan=false` + `host_dial=true` 是**合理组合**（接口
  用户自己建，拨号器还是 modemd 托管），那样拨号器就永远不跑。现在改成
  「建接口那段受开关管，拨号器托管无条件跑」。
- `dial_inner()` 的 QMI/MBIM 分支从「报错」改成「交办」：把用户刚填的 APN
  落到拨号状态里（上面 `dial_update` 已经做了），下一轮循环按它起拨号器。
  `msg` 说清楚是「已交办」而不是「已连上」—— 进程起来到拿到地址要几秒。
- `dial_disconnect.rs`：`hangup` **之前**先停拨号器。顺序要紧 —— 反过来的
  话模组侧先 `AT+CGACT=0`，拨号器下一轮 QMI 轮询发现承载没了会自己重拨，
  两边会抢。
- `dial_enable.rs` 关拨号时的文案改成如实描述（拨号器会被收起），不再留
  「尚未迁移」的能力缺口说明。

测试 469 → **491**（dialer 27 个）。`cargo build` 零告警。

**实机未验证，且命令行参数没对过四家源码**：拨号器源码不在本仓库（构建时
由 Makefile 从 third_party 拉），参数是照配置注释与各家通用用法拼的。全部
集中在 `mod.rs::plan_dialer` 一处，对不上的时候只改那一处。
`stdout/stderr` 进 `null`（它每轮刷一大片，而 modemd 的日志通道是「文件或
stderr」二选一，inherit 不一定落得了盘）；排障靠 modemd 这边打的完整命令行
（可复制手跑）+ 退出原因。真要在设备上看拨号器自己说什么，得另外想办法。

### 企业微信推送：新短信自动转发到微信机器人
### 企业微信推送：新短信自动转发到微信机器人

阶段 5 最后一项。此前 `/api/wechat/*` 六个接口全部落在 `stub::todo`（501），
且 `[wechat]` 配置段有两个字段**被静默丢弃** —— `bot_user_id` 与
`allowed_senders` 写在配置注释里、配置里也确实有这两行，但 `WeChatConfig`
结构体没收。serde 默认忽略未知键，所以这种丢失完全无声无息：扫码登录把凭据
写回 config.toml，下次启动读不到，又变回未登录；`/api/wechat/set` 存了发送者
过滤，重启后也读不回来。用户看到的现象是「明明配过，重启就没了」。

新增 `rust/src/wechat/`，三层结构与其它域一致（纯逻辑 + 薄 IO + 运行期状态）：

- **`mod.rs` 纯逻辑（22 个单测）**：模板渲染、发送者过滤、推送目标选择、
  登录状态机、端点构造。三处判断值得单说：
  - **认不出的占位符原样留着**：模板写成 `{typo}` 时消息里会露出 `{typo}`，
    比静默变成空串好排查（空串只会让人以为「这条短信没内容」）；
  - **长短信后缀只在多条时出现**：单条时 `{parts}` 是空串 —— 默认模板是
    `📩 新短信{parts}\n…`，留个空括号比什么都没有更难看；
  - **学到的发送者也要过过滤**：`listen` 开着时会学「最近给机器人发过消息的人」
    当推送目标，学到一个被 `allowed_senders` 过滤掉的人，**等于把后续短信
    推给他** —— 那比不学更糟，所以过滤放在「学」的时候而不是「发」的时候。
- **`transport.rs` 薄 IO**：跑 `curl` 发 HTTPS。**为什么用 curl 而不是引
  TLS 库**：`base_url` 是 `https://`，而 `std` 没有 TLS；引 `rustls` /
  `openssl` 要另配一份根证书与交叉编译链接参数，`Cargo.toml` 里「依赖刻意
  保持精简」的约定也就破了。设备上 `curl` 是自带的，没有就**明确报错**，
  不假装发成功（页面上也有 `transport_ok` 字段说明）。仓库里已有 `modprobe`
  的先例（`modem/usbdev.rs`）。
- **`client.rs` 运行期状态（16 个单测）**：登录态、学到的目标、已推过的短信、
  节流与真正的「发」。

接线：

- `http/handlers/wechat.rs` 六个接口全部实现。**每一个都过短信密码闸门** ——
  `[wechat]` 比短信本身还敏感：能改推送目标就能把短信转发到任意微信，
  包括看起来只读的 `GET /api/wechat`（它回显的正是 `to_user_id` 与
  `allowed_senders`）。实现上用「闸门只判、由调用方回」：`tiny_http::Request`
  是一次性的，被闸门吃掉就没法再回业务响应了。
- `http/loops.rs` 的 `+CMTI` 路径：重读短信成功后**先推微信再推页面** ——
  微信那次是同步 curl 到公网（超时 15 秒），不该拖住页面的即时性。
- `config/wechat.rs` 补上 `bot_user_id` / `allowed_senders` 两个字段。

三个「别过度设计」的决定：① 运行期覆盖存内存 + 写盘两个都做，不引入文件监听；
② 已推过的短信按 `index|timestamp|number` 记、上限 200 条（带 timestamp 是因为
模组重启/换卡后 index 会从 1 重新起，只按 index 去重会把新卡的第一条当成已推过）；
③ 推送节流 3 秒，命中时不回滚已记的键（宁可少推，也不要并发发一堆 curl）。

测试 426 → **469**（`wechat/` 39 个 + handler 4 个）。`cargo build` 零告警。

**实机未验证，且 API 路径没有对过官方 SDK 文档**：iLink Bot 的路径是从配置注释
与行为描述反推的，集中在 `wechat/mod.rs::api` 一处的常量里
（`QR_PATH` / `QR_CHECK_PATH` / `SEND_PATH` / `POLL_PATH`），对得上的时候
只改那一处，别散着改。扫码登录 → 发消息的完整链路需要在设备上配好机器人后
走一遍。

### 自动组网：模组一插上就有能上网的 WAN 接口（FM350 / RW350）

### 自动组网：模组一插上就有能上网的 WAN 接口（FM350 / RW350）

阶段 5 最后一块功能欠账，也是**唯一还在影响可用性**的一项：此前新插上的模组
只是「有信号、有 IP」，系统里却没有对应接口 —— 拿不到默认路由、进不了防火墙
wan 区，用户每次都要去 LuCI 手工建接口。`/api/dial` 的 `auto_wan` 字段一直
是 `null`。

新增 `rust/src/netprov/`，三层结构与其它域一致（纯策略 + 薄 IO + 编排）：

- **`mod.rs` 纯策略**：给定模组与既有配置，算出「该有哪些接口、各自什么
  proto / device / metric」以及「UCI 该动哪些行」。无 IO，**18 个单测**全打它。
  - `plan_wan()`：三种模式下 **qmi / ecm / ncm / rndis / mbim / 未知类型一律
    `modem_pro`**（2026-10-13 起的口径，不再只认 QMI，MBIM 也不再例外 ——
    它的会话建立改由 modemd 托管的 quectel-CM 做，接口跟其它类型一样是一张
    `modem<id>`）。**唯一例外**：`host_dial` 关掉时 MBIM 退回 `proto=mbim`
    （那时没人拨号，只能靠 netifd 的 `mbim.sh` / `umbim`）。
  - `diff_plan()`：**只在真有差异时动手**。已配好就一条都不出 ⇒ 每轮循环
    零写入、零 commit、零 reload，不会把 UCI 的 commit 时间戳刷成「刚改过」。
- **`uci.rs` 薄 IO**：跑 `uci show` 读现状（解析 `pkg.section.key='value'` 行）、
  把 `UciOp` 落盘。为什么不用 libuci：OpenWrt 上没有稳定的 libuci 开发包可链，
  而 `uci show` 的输出格式稳定，解析它只要几十行纯函数。
- **`provision.rs` 编排**：读现状 → 算计划 → diff → 应用 → 回报（`AutoWanReport`）。

三条红线（配置注释里写明的口径，逐条照搬）：

1. **我们没建过的接口不动**：只有 proto 落在 `modem_pro` / `dhcp` / `dhcpv6` /
   `mbim` 里的同名接口才归我们管；用户手工建的 `pppoe` / `static` / `qmi`
   接口一律原样留着（手工配置优先）。
2. **只在 Linux/OpenWrt 上真正动手**：没有 `uci` 就整段空操作（跨平台要求）。
3. **认不出的 `wan_mode` 一律按 modem_pro**：配置写错不该让网络换形态。

两个实现上的关键点：

- **device 按模组逐个取，不用全局扫描**：`dial_iface_snapshot()` 是全局扫网卡，
  分不清哪个口属于哪个模组 —— 多模组时会把两张卡都绑到 `modem1` 上。新增
  `usb_netifs_for_port()` 从**这个 AT 口的 USB 设备**的 `net/` 子目录取，
  sysfs 按 USB 设备组织，取到的才是准的。**QMAP 模组优先绑 mux 数据口**
  （`qmap_mode=1`）：父口 `wwan0` 没有 IP，地址在 `wwan0_1` 上，绑错父口
  netifd 永远 NO-CARRIER。
- **模式切换就地生效**：从 `dhcp` 切到 `modem_pro` 时改主接口的 proto、按需删掉
  不再需要的 `modem<id>_6` 并移出防火墙 zone（手工建的那个不删）。

接线：`http/loops.rs` 新增 `spawn_netprov`（15 秒一轮，线程数 7→8），
`/api/dial` 的 `auto_wan` 字段现在返回真实报告
（`ifaces` / `applied` / `skipped` / `ok`）。

测试 392 → **426**：`netprov/mod.rs` 18 个（命名、三模式 proto 选择、MBIM 退路、
计划形态、diff 的建/改/不动/删/zone/自定义 zone/无 device/无 metric、
`UciOp::describe`、MANAGED_PROTOS 表）、`netprov/uci.rs` 6 个（section 解析、
zone 反查、缺省值、`unquote`、`op_args` 语法、空批次）、
`netprov/provision.rs` 2 个（报告存取、计划命名一致性）。

**实机未验证**：这一轮只做了代码与单测。它**会改设备的网络配置**，所以
部署前需要确认：`auto_wan = true` 是默认值，第一轮就会在设备上建
`modem0` 接口并加进防火墙 wan 区。已在日志里留下「动了什么」的逐条记录
（`自动组网 模组 0（qmi，模式 modem_pro）：建接口 modem0；modem0.proto=…`），
部署后第一件事就是看这几行。

### 屏端异步推送：USSD 应答与新短信角标自动到达（FM350 / RW350）

阶段 5 剩下的最后一块体验缺口：`+CUSD` / `+CMTI` 现在会**自动推到页面**，
不必手动刷新。此前这两类 URC 根本不被过滤层识别（同步收取路径收不到任何东西）。

- **`at/urc.rs` 新增两个类别**：
  - `+CMTI` 走前缀表（本程序只发 `AT+CMGL` / `AT+CPMS` / `AT+CMGF`，
    没有一条的应答长这样，进表安全）；
  - `+CUSD` **不能进前缀表** —— 它的查询应答 `AT+CUSD=?` → `+CUSD: (0-2)`
    与主动上报 `+CUSD: 0,"text",15` 同前缀，一刀切吞掉就会把「USSD 支持情况」
    的查询应答一起吞了（正是 `PRIVATE_URC_PREFIXES` 开头警告的那类事故）。
    改用**形态判据**（`cusd_is_urc`）：主动上报第一字段永远是数字状态码，
    查询应答是取值范围 `(...)`，拿「冒号之后是不是 `(`」区分 —— 注意剥的是
    冒号不是空格，`+CUSD:(0-2)` 这种无空格写法也要放行（单测专门盯这条）。
    另有一层兜底：`parse_cusd_line` 对 `(0-2)` 解析状态码会失败，真误入了
    也变不成一条假应答。
- **`model/urc.rs` 折算层扩到四类动作**：`sim_dirty` / `net_dirty` /
  `cusd`（原文 Vec，按到达顺序）/ `sms_dirty`。为什么这两个例外要交原文而不是
  置脏：USSD 应答**没有可重读的查询路径**（只在 URC 里来一次），
  `+CMTI` 的索引也不够用（要重读列表才知道内容）。
- **双路径收取不重复推**：`+CUSD` 无论同步还是异步到达，现在都从同一个 URC
  队列出。`send_ussd` 新增 `take_pending_cusd()`，用 `take_urc_of([Cusd])`
  **只取 CUSD 这一类**（不碰队列里的 `+GTSIM` / 注册态 URC —— 否则一个功能的
  顺手优化会吃掉另一个功能的巡检加速信号），取走即清；消费循环下一拍再来取时
  已经没有这条了。谁先拿到谁推，天然单推。
  为此在 `AtPort` / `Modem` 上新增 `take_urc_of(kinds)` 出口。
- **`+CMTI` 触发一次短信重读**（`refresh_sms_on_urc`）：清缓存不够（页面会显示
  「没有短信」），必须重读 `AT+CMGL` 才能把新短信推上去。节流 10 秒 ——
  群发短信时 `+CMTI` 连着来，每条都重读会把串口占满。读失败不清缓存
  （旧列表比空列表诚实），只记 debug。
- ⚠️ **URC 循环「不发 AT」的边界有了第一个例外**，注释里写清了理由与约束。
- 测试 385 → 392：`at/urc.rs` 五个（CUSD 形态判据、查询应答放行、CMTI 识别、
  `take_urc_of` 只取目标类别且保留其它、多类别同取）；`model/urc.rs` 三个
  （CUSD 原文保序、CMTI 置脏、四类动作共存）。
- **实机未验证**：当前模组短信存储为空，收不到 `+CMTI`；USSD 受 CS 域注册限制。
  这两条要靠「有短信的卡 + CS 已注册的卡」才能验，与 P1-4 短信 PDU 同一轮补测。

### 接线：URC 消费端——暂存的主动上报现在会驱动巡检加速（FM350 / RW350）

P1-5 的 URC 过滤层（`at/urc.rs`）此前只做「过滤与暂存」，暂存队列一直没有消费端 ——
`Modem::take_urc_lines()` / `Modem::urc_dropped()` 是死的。本版本把消费端接上。

- **新增后台循环 `spawn_urc`**（`http/loops.rs`，1 秒一拍）：只处理 AT 口已打开的模组，
  不发任何 AT，只把暂存队列取走；与查询共用同一把 AT 锁，拿到锁后仅 `mem::take` 一个 Vec。
- **新增纯函数折算层 `model/urc.rs`**：一批 URC 进来、一份脏标记出来（无 IO，可单测）。
  为什么只置脏不改状态：单字段注册态 URC（`+CEREG: 5`）与 `AT+CEREG?` 在 `n=0` 时的
  应答**完全同形**，它的值不足以推定状态；拿它覆盖缓存等于用不可信数据盖掉可信数据。
- **三类处置**（取证程度逐条写明）：
  - `+GTSIM:` → `mark_sim_dirty()`：SIM 巡检从「等到点（默认 20 秒）」变成「下一拍
    （≤2 秒）就核实」。误判代价仅多读一次 `AT+CPIN?` + ICCID；身份以 ICCID/IMSI 为准
    且有 30 秒冷却，不会误动作。
  - `+CEREG:` / `+CGREG:` / `+CREG:` 单字段 → `mark_net_dirty()`：注册态变化 ⇒
    `/api/network` 缓存过期，推送循环下一拍重查（仍受推送节拍限制，最快 1 秒一次）。
  - 其余类别（`Ver` / `GtStatus` / `GtRgc` / `GtUsbmode` / `GtSrv` / `GtPin` /
    `GtDss` / `GtCellInfo`）：无动作，仅日志——手册与参考实现均无语义记载，不猜。
- **洪水丢弃成为观测项**：`urc_dropped()` 累计值大于 0 时按 30 秒限流告警一次
  （主动上报被垃圾行淹没 ⇒ 加速失灵，这是排障第一个该看的数）。
- **日志分两级**：每类计数 + 首条原文打 debug；某模组首次见到某类打 info
  （排障时想知道「这台模组到底会发什么」就看这几行）。
- 顺带改准两处注释：`sim.rs` 与 `Modem::mark_sim_dirty` 原写「读协程看到与已知状态
  不同的 `+CPIN:` 时调」，与实际不符——`+CPIN` 与查询应答同形、过滤层刻意不吞，
  它不可能是置脏来源。
- 测试 378 → 385（`model/urc.rs` 六个：三类分流、空批次、混合批次、按类统计保序；
  `modem.rs` 一个：`take_net_dirty` 取走即清），`cargo build` 零告警。
  **实机未验证本轮新增行为**——首见 info 日志就是为了在实机上收集证据。
- 仍未做：`+CUSD` / `+CMTI` 的**屏端异步推送**（需要扩 `PRIVATE_URC_PREFIXES`
  并同步改造 `ussd.rs` / `sms.rs` 的同步收取路径）。

### 加固：IMEI 改写链路——同族只读快照、空 IMEI 可写、写后重启提示（FM350 / RW350）

P0-3 原设想「用 SFI 读写把 MFG 块整块备份出来」**没有命令面可走**——广和通 FM350
AT 手册（v2.2 / v2.10）全文 0 命中 MFG / SFI / NV。改用替代方案「**写前同族快照 +
本地备份记录**」，并把参考实现（luci-app-fm350）在 IMEI 上踩过的两个坑一并收口。

实机取证（停服务 → 脚本探针 → 重启服务；AT 口此次为 `/dev/ttyUSB1`）：

| 命令 | 实机应答 |
| --- | --- |
| `AT+EGMREXT=0,7` | `+EGMREXT: "<imei>"` + `OK` |
| `AT+EGMREXT?` | `+CME ERROR: 100`（查询写法不可用，只能 `=0,7`） |
| `AT+CGSN` / `AT+GSN` | 与上面同一个值 |

- **写前快照改走同族只读命令**（`imei_snapshot`）：广和通 MTK 平台先发
  `AT+EGMREXT=0,7`（与写入指令 `AT+EGMREXT=1,7,"<imei>"` 同族同参位，备份的正是
  即将被覆盖的那个值），解析失败再退回 `AT+CGSN`；台账新增 `source` 字段记录
  原值是哪条命令读到的（旧 `imei.json` 无此字段，按空串读入，不让老台账失效）。
- **空 IMEI 模组从「永久拒绝写入」变为「可写」**：旧逻辑以 `original` 非空判定
  「还没备份过」，于是空 IMEI 模组（**正是最需要改写的维修场景**）永远走不到写入
  命令；且原值为空时，下一次改写会把「原值」覆盖成上一次写进去的值，一键恢复
  从此失效。现以 `saved_at` 判定是否已备份，空原值照记不覆盖（`record_original`）。
  恢复时若原值为空则拒绝——把 IMEI 写成空没有意义。
- **写后回读按平台分流**（`write_imei`）：参考实现 1.0.14-r2 的实机记录表明
  `AT+EGMREXT` 写入**需模组重启后才在射频 / 网络侧生效**，所以广和通 MTK 平台
  回读到空值 / 旧值都可能是「还没生效」而非「写失败」——该平台只把「模组明确
  报错」当失败，其余情况附重启提示返回成功；其它厂商（高通 `AT+EGMR`）维持原
  语义「回读不一致即报错」，不放松没证据的路径。
- 新增三态解析 `parse_egmrext`：`Some("")` = 模组明确应答空值（合法状态）、
  `Some(imei)` = 读到 15 位、`None` = 读不到（不许当备份）；`/api/imei` 的
  `note` 按平台说明真实口径，set / restore 响应新增 `warning` 字段（仅当次操作
  携带，GET 不带）。
- 前端 `toast` 新增警告态（琥珀色，6 秒）：写 IMEI / 恢复 IMEI 时，「已回读确认」
  与「已下发、需重启后复查」分开说，不拿「已回读确认」糊弄。
- 测试 372 → 378（EGMREXT 解析三态、备份不覆盖、旧台账兼容），`cargo build`
  零告警。**未做实机改写验证**——那会改掉用户设备的真实 IMEI，需用户明确授权后才做。

- **屏端 / WebSocket 动作协议**：`/ws` 与 `/display/ws` 现在认 8 个 action ——
  `snapshot` / `ping` / `sms_read`（`AT+CMGR` 读一次即已读）/ `sms_delete`（删完重读列表）
  / `mode_get` / `mode_set`（下发 + 回读确认）/ `sms_send` / `net_get`；新增 `mode` 消息
  （`value` / `name` / `short` 短名 / `choices` / `screen` 三个按钮 / `read_only` / `msg`）
  与 `sendsms` 消息。除 `snapshot` / `ping` 外全部丢独立线程执行（发短信要等提示符、
  切模式要等回读，卡在 WS 读循环里会让这一路的心跳与推送全停）。FM350-GL 实机验证通过。
### 修复：广和通 MTK 平台 SINR 恒为空、基站扫描 0 条（FM350 / RW350）

实机 FM350-GL（固件 `<固件版本>`）复现并验证。两个现象同一个根因：
该平台的 `AT+GTCCINFO?` 回的是**无标签行表**（先回一行 `+GTCCINFO:` 头，随后每行
一个小区、首列 1 = 服务 / 2 = 邻区），而此前的解析器只认移远 / 广和通高通那种带
`"servingcell"` 标签的形式，整包被丢弃——`/api/cells` 恒 0 条；LTE 制式下
`AT+CESQ` 又不带 SINR 格，`nr_sinr` 跟着恒空。

- **新增 `cells::parse_gtccinfo_rows`**（列序按手册 §11.1.15 钉死）：按 `<rat>`
  （2 = WCDMA / 4 = LTE / 9 = NR）+ 角色分别定位 `<band>` / `<bandwidth>` /
  `<sinr>` / `<rsrp>` / `<rsrq>` 列——服务小区与邻小区的列序不同（邻区没有
  `<band>`，NR 邻区连 `<bandwidth>` 都没有，LTE 邻区还没有 `<rssnr_value>`）。
  `<band>` 实测为空时按频点反推（NR 只有这一条路）；`<rssnr_value>` 本身就是 dB
  （-100 ~ 100，255 = 未上报）。`scan_cells` 在该平台改走 `AT+GTCCINFO?`，
  不再白发 4 条 QENG / FENG。
- **信号查询按平台分流**（`signal.rs`）：广和通 MTK 跳过 `AT+QENG`（移远私有指令，
  发过去只会白等一个超时），SINR 取 GTCCINFO 服务小区行，RSRP / RSRQ 仍以
  `AT+CESQ` 为准、GTCCINFO 兜底。实机 `/api/signal` 从「只有 RSRP / RSRQ」变为
  `nr_sinr:-3`；`/api/cells` 从 0 条变为 1 服务 + 2 邻区（含同频标记）。
- **修正 `AT+CESQ` 的 LTE 索引换算（原来差一整档）**：原实现 `idx - 140` /
  `(idx - 39) / 2` 取的是区间**上界**口径，而同一函数里的 NR SSB 分支、参考实现、
  3GPP TS 27.007 §8.69 与手册 §11.1.15 都是**下界**（索引 1 = [-140,-139) dBm）。
  现统一为 `idx - 141` / `(idx - 40) / 2`（半步值向零截断），读数整体向悲观侧
  修 1 dB / 0.5 dB。
- 索引换算抽成 5 个纯函数（`lte_rsrp_index_dbm` 等），CESQ 与 GTCCINFO 共用
  一套口径；测试 364 → 372，`cargo build` 零告警。
- 已知限制（照报不猜）：该机型 GTCCINFO 的 `<mnc>` 实测报 `1`（`AT+COPS?` 报
  `11`，参考实现的机型报 `0`），运营商名因此不出现在基站表；`<rxlev>` 与
  `<rsrp>` 同表（手册明示），不重复取。

### 杂项：IMEI 脱敏

- 仓库里的实机 IMEI 全部换成示例值 `867698040012349`（`docs/PROGRESS.md` 的实机
  说明、`docs/API.md` 的实机应答示例、`imei.rs` 与 `store.rs` 的测试夹具），
  并已重写 git 历史后强推（`16eb301` 之前的提交同样不含实机 IMEI）。
- 顺带修正示例值本身：原先的 `867698040012345` 的 Luhn 校验位是错的（各位和
  66，不是 10 的倍数），却配着 `"luhn_ok":true` 出现在 `docs/API.md` 的示例里。
  末位改成 9 之后全仓库只保留这一个**合法**示例值，文档示例不再自相矛盾。

### 修复：后台运营商不显示（`/api/network/status`）

实机 FM350-GL 上「网络」卡片只剩 PLMN 之后的整行空白：`operator` 从未被赋值
（`network_status()` 里是硬编码 `None`，对所有厂商都一样），而 `AT+QNWINFO`
在该机型回 `ERROR`，制式 / PLMN 也一起空。

- **`AT+COPS?` 兜底**（`modem/model/network.rs` 新增纯函数 `merge_network`）：
  仅当 `AT+QNWINFO` 没给出 PLMN 时才多发这一条 3GPP 通用指令，补
  `plmn` / `mcc` / `mnc` / `tech` / `tech_label` / `operator`。该指令可用的
  厂商不增加往返。`band` / `band_text` / `earfcn` / `net_tech` 没有通用替代，
  保持为空、不造数据。
- **`operator` 三级取值**：内置 MCC/MNC 表（`model/cells.rs`，四家中国运营商
  全名）→ 模组上报名（`AT+COPS?` format=0 才有）→ 原始 PLMN。实机
  `+COPS:0,2,"46011",11` 是 format=2（无上报名），但 `46011` 在第一级表里，
  直接得到 **中国电信**。
- **`<AcT>` → 制式**按 3GPP TS 27.007（`7`=LTE、`11`=NG-RAN、`2/4/5/6`=WCDMA、
  `0/1/3`=GSM）。⚠️ FM350 AT 手册 v2.2 的表早于 NG-RAN 入表（把 11/12 写成
  eMTC/NB-IoT），**不能照它写**；实机注册在 NR n78 时 `<AcT>` 就是 `11`。
  已知限制：NR NSA 下模组常以 LTE 注册、`<AcT>` 报 `7`，字段照报不猜。
- 测试 357 → 364，`cargo build` 零告警；纯函数化（`merge_network` /
  `operator_display` / `cops_act_tech`）后不接串口也能全覆盖。

### 广和通 FM350 / RW350 系列对齐（P0-2：频段锁定写）

实机 FM350-GL（固件 `<固件版本>`，AT 口 `/dev/ttyUSB3`，中国电信）验证。
**契约不变式**：四家拨号器与解析分支互不顶替，改广和通分支不得破坏其它厂商。

- **新增第三套锁网方言 `fibocom`**（`modem/model/lock.rs`）：广和通 MTK 平台
  （FM350 / RW350）用 `AT+GTACT`，与广和通高通平台（走 QNW）**完全不同**。
  此前该机型探测不到方言，回落成 QNW，页面上显示「未探测到」却仍拿
  `AT+QNWPREFCFG` 去写——必然失败。现在 `is_fibocom_mtk()` 直接认出来，
  一次 `AT+GTACT?` 即命中（实机 `dialect_tried` 只有一条）。
- **频段锁定读**：`GET /api/lock/status` 的 `band_lte` / `band_nr` /
  `mode_pref` 从空变为真实值（实机 31 个 LTE + 19 个 NR，`mode_pref` =
  `NR/WCDMA/LTE`）。`GET /api/lock/caps` 的探测项也按方言生成。
- **频段锁定写**（`POST /api/lock/band`）：实机验证通过——锁 LTE→B3、
  覆盖成 B1+B3、锁 NR→n78、解锁还原全部 `code=0` 且回读一致。
- **锁小区写明确拒绝**（`POST /api/lock/cell`）：实机 `AT+EMMCHLCK=?` 只给出
  5 个参数组 `(0-1),(0,2,7),(0,1),(0-46589),(0-511)`，末两组能认出是
  EARFCN / PCI 区间，但中间两组枚举含义无手册可查、锁定态回显格式也未实测。
  **解锁（`=0`）可以做**（参考实现明确写过这个写法），锁定不行——发错一条
  会把小区锁死在错误位置上。

三个设计决策（都是 `AT+GTACT` 的**结构**决定的，不是偏好）：

1. **频段按值域分类，不按下标**。`AT+GTACT?` 回显是位置参数，但 `AT+GTACT=?`
   显示有 9 个参数组、其中 3 组是空集，在回显里不占位，逐组数下标必然错位。
   因此前三个字段（`<rat>` + 两个控制字段）按位置取，其余按值域分类
   （手册 §11.1.14）：UMTS `1..=10`、LTE `101..=199`（`100+n`）、NR `>=500`
   （`"50"`+band 拼接，如 5078 = n78、5020 = n20）。
2. **必须读改写**。一条 `AT+GTACT` 同时写 GSM / LTE / NR 三组，改一组就得把
   另外两组原样带回去。控制字段、GSM 组与**认不出值域的频段值**都没有 UI
   入口，照样原样带回——实机验证「锁 NR→n78」时 `band_lte` 的 31 个频段
   原样不动。认不出的值也原样带回（进 `other`），悄悄丢掉比带回更危险。
3. **解锁写回支持全集，不写 0**。手册记 `<band>=0` 为 "automatic band
   selection"，但一条命令混带多组频段时 0 作用于哪些组没有实测；解锁走
   **已实机验证**的写法——写回 `AT+GTACT=?` 的支持全集（等效「不限制」）。
   读不到支持全集就拒绝执行；锁定时也用它校验用户勾的频段（实测 B99 被拒
   并列出支持全集）。

另外两条硬约束：锁频前要先离线（`CFUN=0` → 下发 → `CFUN=1`），首尾两条
best-effort、中间那条失败必须冒泡且要把模组拉回在线；锁定的频段不在支持
列表里一律拒绝，不发没把握的指令。

实机验证后 `AT+GTACT?` 与操作前**逐字节一致**（配置完全还原）。测试数
348 → 357，`cargo build` 零告警。

### 广和通 FM350 / RW350 系列对齐（P0-1：IPv6 归一化接进网络链路）

实机 FM350-GL（固件 `<固件版本>`，AT 口 `/dev/ttyUSB3`，中国电信）验证。
**契约不变式**：四家拨号器与解析分支互不顶替，改广和通分支不得破坏其它厂商。

- **`AT+CGCONTRDP` 的地址字段下标不能写死**（`modem/model/network.rs`）：3GPP 规定
  `<ip_addr>` 在第 3 位，但实测广和通 MTK 平台把它放到了**第 5 位**（第 3、4 位是空串），
  照 3GPP 取下标只会得到空串——`/api/network/status` 的 `ipv4`/`ipv6` 在该机型上
  **从未上报过**。新增 `parse_active_context()`（纯函数，单包测试落点）：从第 3 位起扫
  第一个含地址的字段，并把紧邻的下一字段也看一眼（有的固件把 v4 / v6 拆成相邻两列）；
  再往后是 DNS（也是合法 IPv4），不再当地址收。解析部分已从 `Modem` 方法里抽出，
  可脱离串口测试。
- **`AT+CGCONTRDP` 按 cid 归并**：实测广和通 MTK 平台把**同一个**上下文的 IPv4 与 IPv6
  拆成两行报（cid 相同，一行点分 IPv4、一行 16 段点分的 IPv6）。原先只取首行，
  永远看不到 IPv6；现在按 cid 归并后两行都取到。
- **`split_addr` 接入 `addr` 归一化**（原先只按 `contains(':')` / `contains('.')`
  分家族、零校验）：两个家族都走 `modem/model/addr.rs`，与 `AT+CGPADDR` 那条路
  （`dial::split_addrs`）**同一套判据**。修掉两个实证缺陷：① 广和通 MTK 平台的
  点分 `::1` 占位（`0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1`）含 `.`，会被当成 **IPv4**
  显示到页面上；② `gg::1`、`2001::8::1` 这类脏值会原样进接口配置。
- **纯 IPv6 上下文不再被跳过**：选中判据从「有 APN 或 v4」改为「有任一数据」，
  无 APN、无 v4 的 v6-only 上下文现在也能上报地址。
- **删掉 `dial.rs` 里重复的点分 16 段解码**：`format_ipv6` / `bytes_from_decimal` /
  `has_zero_prefix` 三处下沉合并到 `addr::decode_dotted_ipv6`（单一事实源）；
  `dial::split_addrs` 的点分分支改走 `addr::normalize_ipv6`。RFC 5952 压缩的测试
  随实现一起搬到 `addr.rs`。
- **`addr::decode_dotted_ipv6` 收紧为统一严格判据**：除全零与 `::1` 外，还拦
  「前 8 字节全 0」（只上报接口标识、没有前缀）——实测 FM350-GL 就这么回。
  与参考实现的偏差已写进 `addr.rs` 模块文档。

实机验证：`GET /api/network/status` 由「无 `ipv4`/`ipv6` 字段」变为
`"apn":"ctnet.MNC011.MCC460.GPRS"` / `"ipv4":"203.0.113.53"` /
`"ipv6":"2001:db8::1"`（由 `32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.1` 解出）。
日志无 ERROR/WARN。测试数 333 → 348。

### 变更
- **LuCI 界面退役 Lua 世代，只保留 JS + ucode**（2026-10-18）：删除  `luci/app/lua/**`（控制器、Lua 翻译表、`overview.htm`/`service.htm`）与 Makefile 的
  Lua 安装块；`postrm` 增加清理，opkg 升级时顺手删掉设备上的 Lua 残留。
  JS 视图（`overview.js`/`service.js`）内联 i18n（`modemdLang()` + `I18N` zh-cn/zh-tw +
  `T()`，语言取 `document.documentElement.lang`，查不到回落英文），概览页补上端口说明句与
  iframe 直嵌。已知限制：菜单标题维持英文（固件无 `po2lmo`，造不出 `.lmo`）。
- **修掉 JS 世代三个实证缺陷**（均为真机跑出）：① `rpc.declare` 的 `expect` 是单键选择器
  而非校验器，`expect:{result:[]}` 会把 `luci.getInitList` 的返回削成空数组，导致自启态
  永远显示「未设」——改为不写 expect、按对象解析；② 运行态原先读 `getInitList` 里不存在的
  `running` 字段，改为从 procd `service list` 取（ACL 已授权 `service:list`）；③
  `ucode/controller/modemd_auth.uc` 忘了在 menu.d 注册节点，端点根本访问不到（404），
  且 ucode 没有 `typeof` 运算符导致 `require` 直接 500——已按 `luci-base.json` 的
  `admin/translations/*` 范式注册，并改用 `type()`。
- 文档同步：`PROGRESS.md` 新增 §11（退役清单、i18n 方案、三个缺陷根因、已知限制、
  实机验证记录），`PORTING.md` 末节与 `HANDOFF.md` 硬性要求第 14 条补 LuCI 世代约束。

### 广和通 FM350 / RW350 系列对齐（P1 八项，参考 `luci-app-fm350`）

实机 FM350-GL（固件 `<固件版本>`，AT 口 `/dev/ttyUSB3`）逐项验证。
**契约不变式**：四家拨号器与解析分支互不顶替，改广和通分支不得破坏其它厂商。

- **P1-1 频段反解**（新增 `modem/model/band.rs`）：`+GTCCINFO` 的 `<band>` 是**编码值**
  而不是人可读名，新增 `lte_band_from_code` / `lte_band_from_earfcn` / `nr_band_from_code` /
  `nr_band_from_arfcn` / `band_label` / `bandwidth_text`。**修掉一处既存 bug**：原
  `network.rs::band_label` 注释写 B3、实现给 `n3`（LTE 频段被按 NR 前缀输出），现已统一；
  FM350 在 NR 制式下 `<band>` 实测为空，只能靠 ARFCN 反推。`NetInfo` 与 `CellInfo`
  新增 `band_label` 字段（`cells.rs` 回填）。
- **P1-2 温度**（`modem/model/siminfo.rs`）：广和通 MTK 平台走
  `AT+GTSENRDTEMP=0`，回 `+GTSENRDTEMP: <e1>,<e2>,<e3>`，量纲 **0.001 ℃**。
  **该平台的读形式 `AT+GTEMP?` 回 `phone failure`，不要用**。新增 `parse_gtsenrdtemp` /
  `gtsen_sensor_name` / `round1`；实机解出 **23 路**传感器（`soc_max` / `cpu_little0-2` /
  `gpu1` / `dramc` / `mmsys` / `md_5g` / `soc_dram_ntc` / `ltepa_ntc` / `nrpa_ntc` /
  `rf_ntc` / `pmic` / `pmic_vcore` / `pmic_vproc` / `pmic_vgpu`），手册未列编号正确退化为
  `sensorN`；`raw=0` 的通道跳过（未接入）。
- **P1-3 IMEI**（`modem/model/imei.rs`）：`imei_write_cmd()` 新增广和通 MTK 分支
  `AT+EGMREXT=1,7,<imei>`；`ImeiStatus` 新增 `luhn_ok` 字段（Luhn 校验，**仅非阻断提示**
  —— 部分厂商 IMEI 不严格遵循 Luhn）。**取证程度说明**：该指令**未被广和通手册
  （v2.2 / v2.10）收录**，属社区与实机验证命令；本程序已有三重保护（15 位数字校验、
  改写前备份、下发后回读 `AT+CGSN` 确认），故破例收录，注释已写明。
- **P1-4 短信 PDU 发送**（`modem/model/sms.rs`）：广和通 MTK 平台**整条发送序列换成 PDU
  模式**（不是在文本序列上打补丁）——依据参考实现的「TEXT 模式下中文与长短信都不可靠」。
  新增 `GSM7_EXT` 表（`gsm7_ext` 改为查表，单一事实源）、`string_to_gsm7`、`pack7`
  （`unpack7bit` 的逆运算）、`utf16_bytes`、`split_content_for_pdu`（按**真实 septet /
  UTF-16 容量**切分）、`build_submit_pdu`。序列：`AT+CMGF=0` → `AT+CMGS=<TPDU 字节数>` →
  等 `>` → PDU + Ctrl-Z → **发完切回 `AT+CMGF=1`**（短信模式是全局状态）。UDH 开启时
  不自动发 `UDHI`（变通）；失败按「成功几条 / 共几条」报。实机未验证（模组短信存储为空）。
- **P1-5 URC 过滤层**（新增 `modem/at/urc.rs`，**本项风险最高、单独提交**）：串口 `read`
  不保证按行返回，URC 一旦混进应答就会被解析层当成查询结果（实测 `+CEREG: 5` 抢跑导致
  注册态误报，`regstate::parse` 取的是第 2 字段）。新增 `LineGate`（字节层行分割 +
  每口一份过滤缓冲）、`classify_urc` / `UrcKind` / `UrcLine`、洪水阈值
  （1 秒 60 行，**只对 URC 行计数**——`AT+COPS=?` 扫网能一次回上百行，一起限流会截断
  查询结果）与 `Modem::take_urc_lines()` / `Modem::urc_dropped()`。识别 10 类原生 URC：
  `VER` / `+GTSTATUS` / `+GTRGC` / `+GTUSBMODE` / `GTCELLINFO` / `+GTSIM` / `+GTSRV` /
  `+GTPIN` / `+GTDSS`，注册态 `+CGREG`/`+CEREG`/`+CREG` **靠字段数区分**（单字段是 URC、
  双字段是查询应答）。**安全边界**：① 只吞明确是主动上报的行——参考实现用 `+GT` 宽匹配，
  结果把 `AT+GTDNS` 的查询响应一起吞掉、巡检 DNS 恒为空，故这里逐条列前缀；
  ② `+CPIN` 刻意不吞（与 `AT+CPIN?` 应答同形）；③ 本程序查询过的前缀一律不进表。
  实机验证：`AT+CEREG?` 回 `+CEREG: 0,1` + `OK`，双字段应答完整保留。
- **P1-6 PDP 判据**（`at/parser.rs` / `model/params.rs` / `model/dial.rs`）：
  `is_duplicate_activate` 识别 `+CME ERROR: 5847`（已激活），`set_pdp` 仅在**激活方向**
  容忍；新增 `cgact_state(resp, cid)` **三态**回读，`hangup` 用它区分「没列明细」与
  「已确认去激活」（FM350 上 `AT+CGACT?` 只回 `OK`、不列 `+CGACT:` 行，空转会误判）；
  新增 `DialReadiness` 与 `Modem::dial_readiness()`（CPIN / CFUN / CEREG 优先，回落
  CGREG / CREG），只在**失败路径**调用（成功路径零成本）。`CFUN=4` 飞行模式自恢复
  **不做**——同步 HTTP 会挂住请求线程。
- **P1-7 IPv6 编解码**（新增 `modem/model/addr.rs`）：`is_valid_ipv4` / `is_link_local_v6` /
  `normalize_ipv6` / `decode_dotted_ipv6` / `format_ipv6_groups` / `looks_v6`。
  `normalize_ipv6` **拒占位地址**（全零 `::`、纯 `::1`、16 段点分的 `::1`）——FM350 在
  IPv6 未分配时回 `0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1`，不拦会配出一条永远不通的 v6 默认路由；
  剥 zone id（`%eth2`）与方括号；冒号形式**原样返回不重新压缩**；内嵌 IPv4
  （`::ffff:1.2.3.4`，占 2 个 16 位组）仍认。`split_addrs` 冒号分支改走统一归一化，
  结构不合格（`gg::1`、两个 `::`、组数不对、组过长）一律拦下；16 段点分分支**刻意保留
  比参考实现更严的判据**（前 8 字节全 0 也不显示）—— 那是 FM350-GL 只报接口标识的实机行为，
  按项目惯例不显示不可信地址。`dial.rs` 的 `parse_ipv6` / `looks_like_ipv6` / `format_ipv6`
  三处重复实现下沉到本模块（编解码只留一份）。
- **P1-8 EMBIND 体检**（新增 `modem/model/bind.rs`）：`AT+EMBIND?` / `AT+EMBIND=?` /
  `AT+EMBIND=1,"<L2P>",<cid>` / `AT+EMBIND=0,"<L2P>",<cid>`，L2P ∈
  `M-CCMNI` / `M-RNDIS` / `M-MBIM` / `M-LHIF`，主机侧落点只有 RNDIS / MBIM。
  新增 `parse_embind` / `BindState` / `BindEntry` / verdict
  （`host` / `ccmni-only` / `none` / `unknown`）/ `explain` / L2P 常量，
  `Modem::embind_status()` 过机型闸，`at/presets.rs` 补 3 条广和通 MTK preset。
  **只做只读体检 + preset，不做 `rebind_to_host()` 自动改绑**。

工程指标：`cargo build` 零告警、`cargo test` **333 项全绿**（P1 前 276 项）。
文档同步：`API.md`（imei 三节、at/presets、signal 说明）、`PROGRESS.md` §15、
`PORTING.md` 广和通 MTK 判据表。

### 新增
- **Rust 后端迁移（只读域，进行中）**：新增同端口网关与 WebSocket 升级处理、路由总表
  `ROUTES` 与未迁移接口兜底（`stub.rs`）、推送中枢 `WsHub`；已迁移
  `/api/modem/info`、`/api/modem/status`、`/api/sim`、`/api/vendor/cmds`、`/api/cells`、
  `/api/cells/all`、`/api/lock/status`、`/api/lock/caps`、`/api/params`、`/api/mode`、
  `/api/usbnet`、`/api/imei`、`/api/airplane`、`/api/roam`、`/api/ussd`（读）、
  `/api/signal/history`、`/api/changes`、`/api/changes/clear`。
- **短信只读域 + 访问密码闸门**：新增 `/api/sms/read`（PDU/文本解码、长短信拼接、去重与按时间倒序）、
  `/api/sms/storage`（GET，`+CPMS?` 存储区用量）、`/api/sms/auth`、`/api/sms/unlock`、`/api/sms/lock`。
  新增 `modem/model/sms.rs`（PDU 解码、GSM7/8bit/UCS2、UDH 拼接、存储解析，含单元测试）与
  `http/smsauth.rs`（令牌 30 分钟 / 最多 32 个 / 连错 5 次锁 60 秒）。未解锁访问短信接口返回
  HTTP 401 + `{"locked":true,"required":true}`；`send` / `delete` / `password` / `storage`(POST) 待补。
- **流量配额域**：新增 `/api/quota`（采样 + 本周期累计 + 超限评估）、`/api/quota/set`
  （含 `used_gb` 对账校正）、`/api/quota/reset`（清零并起新周期）、`/api/quota/unblock`
  （本周期不再限制）、`/api/traffic/reset`（清零模组累计计数器）。新增
  `modem/model/traffic.rs`（计数式 `AT+QGDCNT?` / 速率式 `AT+GTSTATIS?` 按量纲解析）、
  `modem/model/quota.rs`（周期、用量、超限判定、PDP 断网动作）与 `runtime/store.rs`
  （`quota.json` 台账，写在配置文件同目录、重启不丢）；`config.toml` 新增 `[quota]` 默认段。
  超限默认只对**上网用**的 PDP 上下文发 `AT+CGACT=0,<cid>`（跳过 IMS/sos），`block_rf` 走 `AT+CFUN=4`。
- **短信写操作域**：新增 `/api/sms/send`（文本模式 `AT+CMGS` 两步下发，中文 / emoji 自动切
  UCS2、发完切回 IRA，超长按 70/160 字符自动分段）、`/api/sms/delete`（`index` 单条 / `all` 全部）、
  `/api/sms/password`（改密码与开关保护：再验一次当前密码 + 限流，写回 `config.toml` 的 `[ui]` 段
  —— 行级原地替换、注释原样保留、改前备份 `.bak`，并吊销全部旧令牌、给当前页面补发新令牌）、
  `/api/sms/storage`(POST)（切存储区并回读确认，写回 `[sms] sms_memory`）。
  新增 `config/patch.rs`（保留注释的行级配置写回）；短信闸门支持运行期覆盖（改完不用重启）。
- **参数 / 开关写操作域**：新增 `/api/params/apn`（写 APN 后回读确认 + 重新激活 + 激活 IMS）、
  `/api/params/apn/delete`、`/api/params/pdp`（激活/停用/先断后连）、`/api/params/netselect`
  （`AT+COPS=0` / `AT+COPS=1,2,"<plmn>"`）、`/api/mode/set`、`/api/usbnet/set`
  （仅移远/美格可写，广和通 RNDIS 拒绝执行）、`/api/airplane/set`（`AT+CFUN=4/1`）、
  `/api/roam/set`（写回 `[net] allow_roaming` 并即时生效）。APN / PLMN 入参做了字符白名单校验。
- **主动扫网**：新增 `/api/netscan`（`?tech=lte|nr|both&minutes=3~10`，`AT+COPS=?` 可见运营商 +
  芯讯通 `AT+CNWSEARCH` 小区）与 `/api/netscan/stop`（可中断：AT 口读取支持取消标志，
  中断后返回已扫到的部分并标 `stopped`）。新增 `modem/model/netscan.rs` 与带取消语义的
  `AtPort::command_cancellable`。
- **IMEI 写操作域**：新增 `/api/imei/set`（`{modem_id,imei}`：首次改写前自动备份原始值到
  `imei.json`，下发 `AT+EGMR=1,7,"<imei>"` 后回读 `AT+CGSN` 确认，不一致即报错）与
  `/api/imei/restore`（一键写回备份的原始值，记录 `restored_at`）。仅移远 / 美格等高通系
  可写（`imei_write_cmd()` 认不出机型就拒绝，不乱发厂商私有指令）；IMEI 必须是 15 位数字。
  `runtime/store.rs` 新增 `ImeiStore`（`imei.json`，与配置文件同目录、重启不丢），
  `ImeiStatus` 扩展 `original` / `writes` / `can_restore` / `restored_at` 字段。
- **设备级操作（重启 / 校时）**：新增 `/api/modem/reset`（`AT+CFUN=1,1`，指令写进串口
  即视为成功 —— 重启断开 AT 口、等不到 OK 是常态，用 `confirmed` 字段区分）与
  `/api/modem/clock`（`AT+CCLK` 写 UTC；固件不收时区尾巴时自动回退为不带时区重发，
  返回整份模组状态供前端刷新时钟卡片）。`AT+CCLK?` 读侧统一去引号（界面直显）。
  顺带修正 `/api/modem/status` 的漫游判定走运行期覆盖值（改过 `allow_roaming` 后
  不用重启即生效）。
- **锁网写操作域**：新增 `/api/lock/band`（锁/解频段：QNW 冒号列表下发 + 回读确认；
  **解除不写 0**——那会清空频段导致脱网，而是读 `policy_band` 全量写回，读不到就拒绝）、
  `/api/lock/cell`（`lock_type` 0/1/2，`AT+QNWLOCK`，写完回读确认）、
  `/api/lock/mode`（旧接口，复用 `mode` 域下发）、`/api/lock/unlock`（一键解除全部）。
  芯讯通方言的小区锁定与两种解锁写法**未验证，明确拒绝盲写**（与「不发未经验证的
  厂商私有指令」原则一致）；`NRNSA` 仅在分两套列表的 QNW 方言里可用。
- **网络状态补流量**：`/api/network/status` 增加 `traffic`（累计上行/下行 + 实测速率 +
  估算标记），沿用 `model/traffic.rs` 的同一份采样。
- **波动记录与信号采样历史**：新增 `/api/signal/history`（`?modem_id=-1&minutes=30&limit=2000`）、
  `/api/changes`（`?limit=200&modem_id=-1`，返回 `entries` + `current`）、`/api/changes/clear`
  （连基线与采样历史一起清，并广播 `changes_clear`）。新增 `modem/metrics.rs` 与
  `model/changes.rs`、`model/history.rs`（进程内环形缓冲：波动 500 条、每模组 1800 段）；
  记录点挂在 `/api/signal`、`/api/network/status`、`/api/cells` 的既有查询上，**不额外发 AT**；
  阈值（3 dB / 跨品级）+ 同字段 30 秒节流防刷屏。
- **USB / PCIe / atport 绑定与 netinfo**：新增 `/api/netinfo`（按厂商发 `AT+QNWINFO` /
  TDTech `AT^SYSINFOEX`；Unisoc 回空）、`/api/usb`（`modem/usbdev.rs`：已知身份表 +
  sysfs 设备枚举 + 事件环形缓冲 60 条 + 补驱动能力）、`/api/usb/clear`、`/api/usb/bind`
  （手动补 usb-serial 驱动）、`/api/pcie`（`modem/pcidev.rs`：模组驱动/设备判定 +
  AT/QMI/MBIM 候选口）、`/api/pcie/reset`（remove + rescan）、`/api/pcie/bind`、
  `/api/atport/bind`（GET/POST，`modem/runtime/store.rs` 新增 `AtPortBindStore`
  （`atport_bind.json`）：按 USB `vid:pid` 钉死命令口）。`catalog_models` 字段暂缺。
- **AT 解析**：`+COPS/+CREG/+CGREG/+CEREG/+C5GREG`、`AT+CGDCONT/CGACT`、
  `AT+QENG`/`AT+FENG`（serving/neighbour）、`+CPSI`、`+CESQ`（含广和通 SSB 扩展）、
  `+QNWPREFCFG`/`+QNWLOCK`/`+CSYSSEL` 等，落地为带单元测试的解析模块。
- **AT 直发域**：`/api/at` 改为 `data:{cmd, resp}` —— `resp` 是**应答行数组**
  （去 `\r\n` 与空行）；只允许单条指令（空 cmd / 含换行 / 超 128 字符分别报
  `cmd required` / `cmd must be a single line (<=128 chars)`；非 POST 报
  `method not allowed, use POST`）；发送失败也回 HTTP 200 + 半截 `resp`。
  新增 `/api/at/presets`（3GPP 通用 28 条 + 六家私有表，同名覆盖、展锐抑制 `AT+CGCONTRDP`；
  无模组回 code 0 兜底清单）与 `/api/at/port`（手动指定 AT 命令口，身份键台账 `atport.json`，
  探活后切口 / 恢复自动探测，支持 `imei:`/`model:`/`ports:` 兜底键）。厂商识别补齐
  **鼎桥**与**展锐**（共七家；型号短名按实测过的三类写法归一）；
  `ModemView` 新增 `tty_list`/`vendor_name`/`model_raw`/`at_port_manual`。
- **拨号域**：新增 `GET /api/dial`（拨号状态 + USB 类型识别 + 类型表 + 开关三件套一次拉全，
  `?force=1` 重认类型）、`POST /api/dial/connect`（`{modem_id,cid,apn,pdp_type,reconnect,force}`）、
  `/api/dial/disconnect`、`/api/dial/enable`、`/api/dial/mode`、`/api/dial/ipv6`。
  新增 `modem/model/dial.rs`（USB 类型三路识别：固件 `AT+QCFG="usbnet"` / USB 描述符类码 /
  主机侧网卡驱动；拨号状态机：写 APN → 激活 PDP → 激活 IMS → `AT+CGPADDR` 读本机地址 →
  主机侧网卡候选，全过程分步记录 `steps`）与 `config/wan_mode.rs`（WAN 模式
  modem_pro / dhcp / both，含别名与展示名）。**QMI/MBIM 不代发 `AT+CGACT`**——数据呼叫归
  主机侧厂商拨号器（托管属后续阶段），明确拒绝而不是假装成功；失败时 `data` 仍带回
  `DialState`（页面显示卡在哪一步）。开关三接口写回 `[net] dial_enabled|wan_ipv6|wan_mode`
  （行级替换、即时生效）。`auto_wan` 暂为 null（组网循环属阶段 5）。
- **定时器域**：新增 `GET /api/schedule`（任务列表 + 8 种动作元信息 + 目标模组下拉一次拉全）、
  `POST /api/schedule/set`（校验归一化：`8:00`→`08:00`、tech 别名、频段排序去重、模式 7 档语义值
  或本机原码；`modem_id` 回退 + 旧版单动作写法兼容）、`/api/schedule/delete`、`/api/schedule/toggle`、
  `/api/schedule/run`（立即执行开始动作；模组拒绝时带视图与错误文本）。新增
  `modem/model/schedule.rs`（模型 / 校验 / 本机时区时间判定 / 视图）、
  `runtime/store.rs::SchedStore`（`schedule.json` 台账：槽位去重跨重启、原子写、坏任务跳过）与
  `http/schedule.rs`（视图组装 / 循环 tick / 立即执行）。**定时任务按本机时区判定**
  （`logging/time.rs` 新增 POSIX TZ 解析：`TZ` / `/etc/TZ`，不解析 TZif、不套 DST，认不出按 UTC
  并 warn），到点后 15 分钟内允许补做、同一"日期+时间"只执行一次；后台循环每
  `schedule_interval`（默认 10 秒）tick 一轮、启动先补跑、不看有没有客户端在线；目标解析
  端口键优先、miss 且 IMEI 唯一匹配才迁移键；动作执行全部复用锁网 / 组网模式 / 飞行模式
  既有实现（下发 + 回读确认口径一致），执行前做机型能力预检（方言须**真被固件接受过**，
  不支持关射频的机型提前标「开飞行模式」不可用）。
- **推送域收口（阶段 4）**：`http/push.rs` 补齐 `push_cells` / `push_lock` / `push_sms` /
  `push_smscount` / `push_ussd`，并把它们接到对应写操作的成功分支上 ——
  `/api/cells`（真扫后）、`/api/lock/{band,cell,mode,unlock}`、`/api/sms/read`（`sms` + `smscount`）、
  `/api/quota/{set,reset,unblock}`、`/api/traffic/reset`、`/api/ussd`（POST 同步收到 `+CUSD` 时）。
  `WsHub::broadcast_sms` 按连接过滤：**带正文的 `sms` 过短信密码闸门**（与 `/api/sms/read`
  共用同一把令牌），`smscount` 只有 `{total,unread}`、不含正文，因此不过闸门。
  闸门有 3 个专测（真实 TCP 对：按令牌过滤 / 无闸门人人可收 / 对端已关不带崩广播）。
  `/api/traffic/reset` 的推送**不过滤**「不上报计数器」的模组（周期循环才过滤，口径同 Go）。
- **USSD 下发（`/api/ussd` POST）**：`modem/model/ussd.rs` 重写 —— `valid_ussd_code`
  （只允许数字与 `* # +`，长度 1~64，防注入）、`parse_cusd_line`（引号定界，正文可带逗号；
  UCS2 按短信同一套判据还原）、`Modem::send_ussd`（校验 → `AT+CPIN?` 判 PIN 锁 → 清旧缓存 →
  `AT+CUSD=1,"<code>",15`；应答含 error 时报「USSD 是电路域(CS)业务，当前 CS 未注册或卡不支持」；
  同步到达的 `+CUSD` 就地收下并推送）、`Modem::sim_pin_locked` / `clear_ussd`。
  GET 契约修正为 `{modem_id, result}`（`result` 可为 null）；路由总表补上 POST 的 dispatch
  （此前落到未迁移兜底）。5 个单测覆盖字符集、状态码、正文带逗号、UCS2 还原、纯数字不误解码。
- **修复（实机事故）`/api/dial/mode` 写回配置把服务写死**：`[net] wan_mode` 是字符串值，
  之前误用 `patch::set_raw`（原样写、不加引号），配置文件里留下非法的 `wan_mode = dhcp`，
  服务从此解析不了配置、进 crash loop，只能上机手工改文件。改为 `patch::set_string`
  （自动加引号并转义）；`set_raw` 只留给 `true`/`false`/数字这类原样值。
- **配置加载不再一坏就全丢**（`config/loader.rs`）：`config.toml` 是 conffile（升级不覆盖），
  被写坏一行就会让服务再也起不来、整台设备的模组界面全丢。现在按
  「主配置 → `config.toml.bak` → 内置默认值」依次兜底，每级把原因打到 stderr
  （procd 收进 syslog）。兜底方向是**收紧**：登录闸门与短信密码闸门默认都开着。
  附带修正 `ServerConfig::default()` 里硬编码的 Unix 日志/PID 路径（改走 loader 的
  平台判定，Windows 上不再往 `/var/log` 写）。
- **SIM 热插拔巡检（阶段 5 一部分）**：`modem/model/sim.rs`（`SimIdentity` / `cpin_present` /
  `sim_identity_differs` / `sim_hotswap_event` / `should_reset_modem` / `should_check_sim` /
  `in_cooldown`，6 个单测）+ `http/loops.rs::spawn_sim_watch`（2 秒一拍，只有到点或 URC 置脏
  才发 AT；`[sim] hotswap_interval` 缺省 20s、`hotswap_reset` 默认 false）。
  换卡 / 拔卡时清短信缓存并补推空 `sms` + `smscount`；`GET /api/sim` 增补
  `last_event` / `last_event_at` / `checked_at`。
- **文档**：新增 `docs/PORTING.md`（接口级迁移清单）、`docs/HANDOFF.md`（进度交接表与要求）、
  `docs/PROGRESS.md`（完整进度文档 + 提示词要求）；`README.md` 增加「致谢」段。

- **主机侧网卡配置（`/api/host/nic*`）**：新增 `GET /api/host/nic`（网卡清单 + 建议值 +
  三组命令 + OpenWrt 的 UCI 持久化块）、`POST /api/host/nic/apply`（`renew` / `static` / `dhcp`，
  命令**默认只生成不执行**，`execute=true` 才下发且不拼 shell）、`POST /api/host/nic/save`
  （写 `hostnic.json`）。新增 `runtime/hostnic.rs`（机型特例只认 FM350、掩码/前缀互转、
  配置校验归一化、OpenWrt 不写 `resolv.conf`、UCI 块全部 `manual` 只显示不执行）与
  `http/hostnic.rs`；`Modem` 补 `dial_state_ipv4()` / `query_own_ipv4()`。FM350-GL 实机验证通过。
- **模组组网修复（350 系列重点）**：数据网卡改为**驱动 + 厂商 VID** 探测（`rndis_host` /
  `cdc_ether` / `cdc_ncm` / `qmi_wwan` / `mbim`），排除板载（`mtk_soc_eth` 等）与已被
  lan/wan 占用的口，兜底 `eth2/eth3/wwan0/usb0` —— 实机从 eth0/eth1 收敛到 **eth2**；
  防火墙 zone 改用**真实 section 路径**（实机的 wan 是匿名 `firewall.@zone[1]`，旧的
  `firewall.wan` 必然 `Invalid argument`）并正确拆分列表值；修掉 `network reload` 被当成
  uci 子命令执行（配置写进去却从不生效）；「UCI 已符合计划但 netifd 未加载」时补一次 reload；
  **FM350 / RW350 在 dhcp 模式下改用 `proto=static` + 模组上报地址**（`ipaddr` / `netmask`
  / 同网段 `gateway`，对齐 `luci-app-fm350`），并新增 `UciOp::Unset` 在切回 dhcp 时清残留；
  `sync_dial_state` 与「有没有拨过号」解耦（idle 也能拿到模组地址）。FM350-GL 实机：
  `modem0` up、`10.42.0.16/24`、`ping -I eth2 223.5.5.5` 通，WAN 未改动。
- **350 系列 IPv6 取址（对齐 `luci-app-fm350`）**：新增 `[net] wan_ipv6_mode`
  （`dhcpv6` 默认 / `ra` / `static` / `off`）。`dhcpv6` 把 `modem<id>_6` 配成
  `proto=dhcpv6` + `device=@<主接口>` + `extendprefix=1`（`/64` 委派给 LAN），地址交给
  odhcp6c（用户态收 RA，不吃 `accept_ra` 的坑）；`ra` 用 `proto=none` 并由本程序打开
  `accept_ra=2` 及 `accept_ra_defrtr/pinfo`、主动写 `router_solicitations` 触发 RS；
  `static` 写 `ip6addr=<addr>/128` 并补默认路由。**过滤 `AT+CGPADDR` 的无前缀残值**
  （实机 `::18df:9a01:f982:815a`）—— static 模式下检测到自动退回 dhcpv6，绝不写出
  永远不通的 `/128`；换模式时清掉 `extendprefix` / `ip6addr` 残留。`/api/dial` 新增
  `wan_ipv6_mode` / `wan_ipv6_modes`。FM350-GL 实机验证（未动 WAN）。
- **厂商模块拆分**：`modem/vendor/` 改成一厂商一模块（quectel / fibocom / simcom /
  meig / tdtech / unisoc / unknown），新增 `VendorSpec` 统一接口与注册表。AT 预设、
  厂商指令集、流量候选、温度指令、USB 网卡写入策略、组网模式读指令、主动扫小区、
  拨号器与识别关键字全部收进各自模块；调用方只写 `vendor.spec().xxx()`，新增厂商
  不改调用方。默认实现保守（没有就不给），行为与拆分前逐字一致。
- **Rust 原生 QMI 拨号器**：新增 `runtime/dialer/{qmi,native}.rs`。QMUX 分帧/TLV/
  WDS 请求应答逐条对齐 `files/third_party/quectel-cm`；从 sysfs 反查
  `/dev/cdc-wdmN`、置 `qmi/raw_ip=Y`、CTL Sync → Get Client ID → WDS Start
  （v4/v6 分开 client）→ 退出 WDS Stop；每轮用 Packet Service Status 探活、掉线重拨、
  失败退避 5 秒。职责与外部拨号器一致（地址仍由 netifd 取），`[net] host_dial_bin =
  "native"` 显式开启（只接管 QMI，MBIM 仍走 quectel-CM）；`/api/dial` 新增
  `host_dialer` 后端状态。**QMI 真机未验证**（测试机只有 FM350），默认不启用。
- **350 系列识别扩到 RW350**：新增统一判定 `is_fibocom_350(model)`（FM350 / RW350），
  `is_fibocom_mtk`、`hostnic::quirk`、`dial::host_nic_quirk` 三处改为调用它（此前两处
  各自硬编码 `contains("FM350")`，加型号必漏）；广和通厂商识别关键字补 `RW350`
  （部分固件 ATI 只回型号）。`/api/host/nic` 的 `required`、拨号前置体检提示同步覆盖。
- **收尾巡检**：`cargo clippy --all-targets` 清零（1 个 deny 错误 + 28 条警告）；
  删除 `handlers/stub.rs` 与 `is_known` 兜底（96 条登记路由全部有 handler，
  未登记 `/api/*` 直接 404），新增路由表一致性测试；
  删掉 4 个从未被 Rust 解析的配置键（`qmi_proto` / `qmi_v6_sibling` /
  `v6_fix_models` / `lan6_fix_models`）并修正 `lan_ipv6` / `wan_metric` 的失实注释；
  清理 `has_clients` / `is_automatic` / `active_cids` / `sms_read_at` 等死代码；
  校验 `Makefile` 引用路径与四家拨号器源文件清单全部存在。
- **断链修复与资源释放**：`[sms] sms_memory` 现在真的生效（启动后首次读短信应用，
  换卡重置）；原生 QMI 停止/失败时释放 client id（防止 16 个槽位耗尽）；
  `static` IPv6 默认路由每轮补确认（netifd 异步加地址的时序）；
  init 脚本 `term_timeout 20` + 退出前 `flush_all()` 落盘。
- **仓库脱敏**：全量替换实机现场值（家庭网段 / 模组 CGNAT / 运营商 DNS 与 IPv6 前缀 /
  固件版本串），测试样本与断言成对更新；新增 `docs/PRIVACY.md` 与
  `scripts/check-desensitize.sh`（POSIX sh 守门，退出码非 0 = 有现场值）。
- **四家拨号器 Rust 重构**：新增 `runtime/dialer/mbim.rs`（MBIM 1.0 线协议）与
  `native/{qmi_link,mbim_link}.rs`，`native/vendors/{quectel,fibocom,meig,simcom}.rs`
  只声明「支持哪条链路 / 优先级 / 开关」；协议实现共用。修掉两个真实布局错误：
  `COMMAND_DONE` body 应从 status 起（头 40 字节）、`CONNECT` 固定头是 64 字节
  （早先按 56 会让字符串 offset 偏 8）。分流改为「链路类型 + 厂商策略」双判定：
  Fibocom 的 MBIM 模组不会被硬塞给原生后端。`/api/dial.host_dialer` 新增
  `backend=native-mbim` 与 `vendor` 字段。
- **厂商方言 6 家 → 14 家**：新增华为（`^`）、有方（`$`）、Telit（`#`）、Thales（`^`）、
  Sierra（`!`）、富士康（`!` + NV）、高新兴（`Z`）、合宙（`*`）。`mode_read` 默认改为
  **不发**（空指令 ⇒ 诚实降级），避免给别家发移远的 `QNWPREFCFG`；新增
  `huawei_acq` / `ws46` / `selrat` 三个模式解析分支。8 家的 USB 切换一律只读
  （参数未逐档取证），富士康的 `AT!nv=550` 只给模板并标高危。
### 修复
- **`process_tree(死 pid)` 恒返回非空**（`runtime/dialer/proc.rs`）：进程树的根不在 `/proc` 里时
  现在直接返回空表，与用例「死 pid 没有树」一致 —— 此前 `stop(死 pid)` 会拿着 `[dead]`
  去发信号并白等两轮 `wait_gone`。这条用例在 Windows 开发机上跑不到（`cfg(unix)`），
  所以 main 在 Linux 上是红的，本轮修掉。
- **表单/query 百分号解码**（`http/request.rs`）：`merge_form` / `parse_query` 原来不做百分号
  解码，前端 `encodeURIComponent` 编码过的指令与参数会原样带着 `%XX` 到达 handler
  （如 `AT+CGSN` 变字面 `AT%2BCGSN`）。query 与表单统一解码：`+` → 空格、
  `%XX` → 字节、UTF-8 容错、畸形序列原样保留。

### 说明
- **仓库脱敏**：清除文档里的本机绝对路径、开发机账号等环境信息；代码注释按仓库规范改为中性
  表述（不再出现指向性描述，对照关系只集中在 `docs/PORTING.md` 与 `docs/PROGRESS.md`）。
- 未迁移接口返回 HTTP 200 + `{"code":-1,"msg":"接口尚未迁移到 Rust 后端: <path>"}`。
- 本地迁移参考副本不纳入版本控制。
- **致谢**：本项目的方向、接口契约与真机调参经验来自
  [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro) 与
  [ouyangzq/luci-proto-modem-pro](https://github.com/ouyangzq/luci-proto-modem-pro)，详见 `README.md`。
- 当前质量指标：`cargo build` 零告警、`cargo test` 575 项全绿。

## [1.0.0-1] - 2026-10-18

### 新增
- **仓库整合**：把原分离的 LuCI 前端插件与模组后端合并为单一仓库、单一包 `modem-pro`，
  统一顶层 `Makefile`、`Config.in`、版本号（`VERSION`）与安装脚本。
- **Rust 后端**：`modemd` 守护进程改为 Rust 实现，按功能域细分模块
  （`cli` / `config` / `logging` / `ubus` / `modem` / `http` / `runtime`），引入 `lib.rs` +
  瘦 `main.rs`，附单元测试。
- **HTTP 接口**：`/api/version`、`/api/modems`、`/api/signal`、`/api/network`、`/api/modules`、
  `/api/at`、`/api/logs`、`/api/session`、`/api/login`、`/api/logout`，统一
  `{code,data,msg}` 契约（见 `docs/API.md`）。
- **鉴权**：可选「跟随 LuCI 登录」，经 ubus `session.login` / `session.get` 校验，
  ubus 不可用时放行并记日志。
- **LuCI 前端**：同一包内只含 ucode/JS 世代（JS 视图、ucode 控制器端点、menu.d 菜单、
   ACL、协议 JS），含概览页与服务控制页（2026-10-18 起 Lua 世代已退役，见「变更」）。
- **内置调试台**：静态页编译期嵌入二进制，安装后 `http://<路由器>:9527/` 直接可用。
- **厂商拨号器**：四家（Quectel / Fibocom / Meig / SIMCom）源码随包编译，按 `Config.in`
  开关裁剪；netifd 协议 `modem_pro` 单接口同时承载 IPv4(DHCP) 与 IPv6(SLAAC/DHCPv6)。
- **文档**：`docs/INTEGRATION.md`（整合方案）、`docs/BUILD.md`（编译与验证）、`docs/API.md`。

### 说明
- 版本沿用「主版本.次版本.修订-发布号」，此次为整合后的首个统一版本。
- 迁移清单（短信/小区/锁频/流量配额/定时任务/推送/PCIe/组网/URC 等）见
  `docs/INTEGRATION.md` 第 5 节与 `docs/API.md` 末尾。
- 若曾安装旧包（`modemd-pro` / `luci-app-modemd-pro` / `luci-proto-modem-pro`），请先卸载。
