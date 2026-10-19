# 完整进度文档（PROGRESS）

> 本文件是**唯一入口**式的进度快照：接手者（人或模型）读完本文件即可继续工作。
> 配套文件：
> - [`PORTING.md`](./PORTING.md)：接口级勾选清单（逐条路由）。
> - [`HANDOFF.md`](./HANDOFF.md)：阶段交接表 + 硬性要求。
> - [`API.md`](./API.md)：对外接口契约。`INTEGRATION.md` / `BUILD.md`：整合方案与编译。
>
> 快照时间：2026-10-19 ｜ 分支 `main` ｜ 远端 `https://github.com/LianXia233/modem-pro-unified`
> 最近提交：`0472de2`（fix(CI)：包 Makefile 必须位于 feed 子目录，修正 feeds 扫描失败真正根因）
> 工作区：广和通 FM350 / RW350 系列对齐 **P0-1（IPv6 归一化接进网络链路）、
> P0-2（频段锁定写）均已完成、实机验证通过**（见 §16 / §17）；
> **P0-3（IMEI 的 MFG 块备份）已按替代方案完成**（见 §20）；
> P0 其余项（CFUN 自恢复）待做

---

## 1. 项目目标（不变式，任何时候都要满足）

把原本分离的 LuCI 前端插件与模组后端**整合为单一仓库、单一包 `modem-pro`**，后端由 Go 改写为 **Rust**：

1. 路由 / 方法 / JSON 键与原后端**逐字对齐**；前端 `rust/web/index.html`、`mini.html`
   **一行都不用改**即可正常工作；
2. 依赖关系、包名、版本号统一（版本单一来源 = 仓库根 `VERSION`）；
3. OpenWrt SDK / feeds 下可编译、可安装；四家拨号器可按包内 config 子选项
   （`MODEM_PRO_*`，内联在 `Makefile` 的 `Package/modem-pro/config` 块）裁剪。

**绝对不做**：修改前端接口调用方式；在注释/文档里写「与旧 Go 后端对比」这类指向性描述
（对照关系只放在 `PORTING.md` / 本文件）。

---

## 2. 总体进度

| # | 阶段 | 状态 | 完成度 |
| --- | --- | --- | --- |
| 1 | 地基（网关 / 路由 / 推送中枢 / 鉴权 / 静态页） | ✅ 完成 | 100% |
| 2 | 只读域接口 | ✅ 完成 | ~100%（短信域、波动记录、采样历史、流量配额、主动扫网、`netinfo`、USB/PCIe 视图均已完成） |
| 3 | 写操作域接口 | ✅ 完成 | ~100%（流量配额、短信、参数与开关、IMEI 写、重启/校时、锁网写、AT 直发域、拨号域、定时器、USB/PCIe 绑定、atport 绑定均已完成） |
| 4 | WebSocket 全量推送 | ✅ 完成 | 100%（握手 + snapshot + 各面板推送函数齐备；写操作成功后顺手广播；短信推送过密码闸门） |
| 5 | 后台循环（采样 / 配额 / 定时 / 热插拔 / 组网 / 微信 / 历史） | ✅ 完成 | 100%（push / traffic / quota / change / hotplug / sim / logs / **urc** / **netprov** 九类循环已接线；SIM 热插拔巡检含换卡清短信缓存，且 `+GTSIM` / 注册态 URC 可把巡检加速到下一拍；**自动组网 netprov 已完成**（§23）、**企业微信推送已完成**（§24）；`+CUSD` 应答直接推页面、`+CMTI` 触发一次短信重读（§21 / §22）） |
| 6 | 拨号器托管（hostdial + udhcpc） | ✅ 完成 | 100%（外部：按厂商启停拨号器、按进程树收起连 udhcpc、退出自动重试；`/api/dial/connect` 对 QMI/MBIM 从「拒绝」改为「交办」。原生：Rust QMI 拨号器（`host_dial_bin=native`，见 §31.2）。**验证缺口**：外部四家的命令行没在真机跑过（§25）；原生 QMI 的线协议有单测、数据呼叫缺 QMI 真机） |

量化的工程指标（可作为「没跑偏」的自检）：`cargo build` **零告警**、`cargo test` **575 项全绿**、
已迁移接口在实机上返回 **HTTP 200** 的统一信封。

---

## 3. 环境与工具链

- 开发机：Windows，`pwsh`（PowerShell 7）；仓库为一个普通 git 工作副本。
- 可用：`git`、`cargo` / `rustc`。
- **没有 `rg`**：检索请用编辑器的 Grep 工具或 `Select-String`。
- 目标运行环境：OpenWrt（Linux）；核心代码须 Linux/Windows 双兼容，macOS 尽力兼容；
  **禁止硬编码系统路径**；涉及系统能力先做平台检测再分支。

---

## 4. 仓库结构

```
Makefile              顶层单包构建（接入 feeds/packages/lang/rust/rust-package.mk）
Config.in             MODEM_PRO_{QUECTEL,FIBOCOM,MEIG,SIMCOM} 拨号器裁剪开关
VERSION               版本单一来源（1.0.0-1）；打包注入 MODEMD_VERSION（未注入为 "dev"）
LICENSE               MIT
.gitattributes        强制 LF；third_party/** -text
.gitignore            /rust/target/、*.ipk、构建产物、/legacy-go/
rust/                 Rust 后端 modemd
  Cargo.toml          依赖：serde/serde_json/toml/tiny_http/serialport/log(必须 std+alloc)/ctrlc/sha1/base64
  Cargo.lock          **必须提交**（RUST_PKG_LOCKED=1）
  build.rs            版本注入
  src/main.rs         瘦入口
  src/lib.rs          run()
  src/cli/            命令行参数
  src/config/         config.toml 解析（server/serial/sms/sim/net/quota/dial/modem/ui/wechat）
  src/logging/        log 初始化 + 文件双写 + 无依赖时间戳（logger.rs / time.rs）
  src/ubus/           ubus HTTP JSON-RPC 客户端（会话校验）
  src/runtime/        进程生命周期（pidfile / 信号 / 后台巡检 / quota.json 台账）
  src/modem/          AT 口与模组模型
    at/{mod,port,command,parser,presets,urc}.rs   AT 口抽象、命令与解析助手、常用指令清单、URC 行级过滤
    catalog.rs / vendor.rs / manager.rs / discovery/*
    model/            领域模型与对外视图（见 §7）；urc.rs = URC 消费端折算层（脏标记，无 IO）
  src/http/           HTTP 与 WS
    gateway.rs         同端口网关（WS 升级识别 + HTTP 透传）
    server.rs          监听循环入口
    ws.rs              WS 握手/帧编解码 + 推送中枢 WsHub
    router.rs          ROUTES 全表 + 分发（登记项→handler；未知 /api/*→404）
    request.rs / response.rs / auth.rs / static_ui.rs
    handlers/*         一个接口一个文件（没有「未迁移」兜底）
  web/                静态调试台 index.html / mini.html（编译期嵌入）
luci/                 LuCI 前端：ucode/JS 世代（JS 视图/ucode 控制器/菜单/ACL/协议 JS）
files/                安装到设备的文件
  etc/modemd/config.toml          默认配置（conffile，UTF-8）
  lib/netifd/proto/{modem_pro.sh,quectel.sh}
  usr/lib/modem-pro/mp-client.sh
third_party/          四家拨号器 C 源码（不改动、可裁剪）
packaging/            init.d/modemd、control/{postinst,prerm,postrm}
docs/                 API / BUILD / INTEGRATION / PORTING / HANDOFF / PROGRESS(本文件) / example-config
legacy-go/            （本地参考副本，**不在版本控制内**，不构建不推送）
```

---

## 5. 已完成：地基（阶段 1）

### 5.1 同端口网关 + WebSocket（`http/gateway.rs`、`http/ws.rs`）

- **为什么必须自建网关**：前端硬编码 `location.host + '/ws'`，而 `tiny_http` 不支持协议升级。
  故由网关独占监听端口：识别 `Upgrade: websocket` 交 `ws.rs`，其余字节**透明转发**给
  loopback 上的 `tiny_http`（`127.0.0.1:0`）。**这是硬约束，不得改回「两个端口」方案。**
- `WsHub` 提供 `broadcast` / `has_clients` / `client_count` / 短信令牌预留；
  握手后客户端发 `{"action":"snapshot"}` 索取初始快照；**无客户端连接时不发 AT**。
- 已验证：WS 握手成功，`{"action":"snapshot"}` 收到 `{"type":"modems","data":[]}`。

### 5.2 路由总表与分发（`http/router.rs`）

- `ROUTES` 登记全部路由（方法 + 路径）。分发规则：
  1. 已迁移 → 对应 `handlers::*`；
  2. 每个登记项都有 handler（迁移期曾有 `stub::todo` 兜底，已随全部接口完成而删除）；
  3. 未登记的 `/api/*` → **404**；
  4. 其余路径 → `static_ui::serve`。

### 5.3 鉴权（`http/auth.rs`）

- cookie `modemd_sess`，跟随 LuCI 登录（ubus `session.login` / `session.get`）。
- 放行：静态页 + `/api/version|session|login|logout`；**loopback 免登录**；
  ubus 不可用则放行并记 `warn`；登录失败 HTTP 401 + `{need_login,login_url}`。
- WS 侧同款闸门 + 短信令牌 `sms_token`；`/display/ws` 免密固定 key `h29k-tft-9f3c1a7b`。

### 5.4 静态页（`http/static_ui.rs`、`rust/web/`）

- 编译期嵌入；`/`、`/ui`、`/mini`、`/mini.html`、`/layouts/*.css` 可访问。

### 5.5 短信访问闸门（`http/smsauth.rs`）

- **不需要密码**的三种情形（任一成立）：`[ui] require_luci_login` 开启（外层 LuCI 登录已够）、
  `[sms] sms_password_required = false`、`sms_password` 显式写成空串。
- 令牌：`X-SMS-Token` 头优先，其次 query `sms_token`；TTL 30 分钟、最多同时 32 个；
  密码连错 5 次锁 60 秒（HTTP 429 + `retry_after_s`）；默认密码 `password`。
- 未解锁访问短信接口 → **HTTP 401** + `{"code":401,"data":{"locked":true,"required":true},
  "msg":"短信需要密码，请先解锁"}`；`/api/sms/auth` 自身永不被闸门拦。
- 令牌用 `sha1` 混合时间/pid/栈地址/自增计数生成，**不引入 `getrandom`**。

### 5.6 波动记录与采样历史（`modem/metrics.rs` + `model/changes.rs` + `model/history.rs`）

- 两份纯存储：`Tracker`（波动记录，环形 500 条）与 `HistoryStore`（采样历史，每模组 1800 段），
  由 `metrics::Metrics` 持有在 `AppState` 上（进程内，重启即清空）。
- **记录点挂在已有查询上，不额外发 AT**：`/api/signal`、`/api/network/status`、`/api/cells`
  （后者只在真扫时记，`?cache=1` 不记——否则缓存为空会把「0」当基线）。
- 两道闸门防刷屏：阈值（RSRP/RSRQ/SINR 3 dB、RSSI 5 dB，跨品级一定记）+ 同字段 30 秒节流；
  「当前值」与「上一条记录」分开维护，被压掉的采样只更新当前值。
- 采样技巧：值没变时只把最后一个点的时间往后推，曲线上仍是一条画到「现在」的平线。
- 推送：新增的波动记录经 `WsHub::broadcast("changes")` 下发；`changes_clear` 单独一个事件类型。

### 5.7 流量配额（`config/quota.rs` + `model/traffic.rs` + `model/quota.rs` + `runtime/store.rs` + `http/quota.rs`）

- **计量**：按厂商给候选指令逐个探测，第一个解析出数据的才算支持 —— 移远 `AT+QGDCNT?`
  是**计数式**（累计字节，准）；广和通 `AT+GTSTATIS?` 只有**速率式**（瞬时 bps），
  由本进程按 `速率 × 间隔` 积分，标 `estimated`；都取不到数就 `supported=false`，
  界面如实显示「不支持计量」，配额无法启用。
- **用量**：本周期用量 = 「当前累计 − 上次累计」的累加；首次采样**只建基线**（不能把模组
  开机至今的总量算进本周期）；模组重启 / 计数器归零后 `cur < last` ⇒ 把当前值当增量。
- **落盘**：`quota.json` 写在配置文件同目录（`/etc/modemd/quota.json`），重启不丢；
  config.toml 只提供**默认值**（conffile 不该被程序改写）。写失败只 warn，不阻断。
- **周期**：`reset_day` 1~28 → 每月该日零点滚动（自动清零并解除限制）；0 = 不自动重置，
  只能手动清零。29~31 一律夹成 0（有的月份没有这一天）。
### 5.9 写操作域：参数与开关（`model/params|mode|usbnet|airplane|roam.rs`）

- **写 APN**：`AT+CGDCONT=<cid>,"<pdp>","<apn>"` → **回读确认**（模组回 OK 不代表真存下）
  → 重新激活该上下文 → 顺手激活 IMS（失败不影响）。APN 走字符白名单（`字母数字 . - _`）。
- **PDP**：`AT+CGACT=0/1,<cid>`；`reconnect` 先断后连，内部上下文不允许停用时跳过断开直接激活。
- **选网**：`AT+COPS=0` / `AT+COPS=1,2,"<plmn>"`（给 60 秒：选网要等重注册）。
- **组网模式 / USB 网卡 / 飞行模式**：写完**回读确认**，不一致就算失败（界面要看到模组的真值）。
  USB 网卡只有移远/美格 `AT+QCFG="usbnet"` 可写；广和通 RNDIS 直接拒绝（会整机掉 USB）。
- **数据漫游**是**策略开关**（不下 AT）：写回 `[net] allow_roaming` + 运行期覆盖值，即时生效。
- 与短信密码同款：运行期改过的配置值要同步一份覆盖值，否则进程内 `Config` 还是启动时的旧值。

### 5.10 短信写操作域（`model/sms.rs` 写侧 + `config/patch.rs` + 四个 handler）

- **发**：文本模式 `AT+CMGS` 是**两步指令**（先等 `>` 提示符，再发正文 + Ctrl-Z），
  所以 AT 层加了 `write_raw` / `read_until_marker` / `read_response` 三个原语。
  含非 ASCII 就切 `AT+CSCS="UCS2"` + `AT+CSMP=17,167,0,8`，地址与正文都按 UTF-16BE 十六进制下发；
  **发完一律切回 IRA** —— 字符集是模组全局状态，不切回来的话之后读到的正文会是一串十六进制。
  超长按 70（UCS2）/ 160（ASCII）字符自动分段。
- **删**：`AT+CMGD=<index>` 单条、`AT+CMGD=1,4` 全部；位置只认数字。
- **切存储区**：`AT+CPMS="SM","SM","SM"` 三处一起切，并回读确认（实测有些模组刚下发会短暂报成 MT）。
- **写回 config.toml**：`config/patch.rs` 做**行级原地替换**（`toml::to_string` 一序列化就会把
  满文件的真机注释冲掉，绝对不能用）；改前备份 `config.toml.bak`。
  改密码 / 开关保护后同步一份**运行期覆盖值**到 `SmsAuth`，因此不用重启进程就生效。

- **超限动作**：`block` 只对**上网用**的 PDP 上下文发 `AT+CGACT=0,<cid>`（IMS/sos/mms 跳过 ——
  实测停用 IMS 会回 ERROR）；`block_rf` 走 `AT+CFUN=4`，且**先查 `AT+CFUN?` 能力**，
  展锐 SIPC 那一族（F50）直接报错；`warn` 只提醒。已断网的按 `block_interval` 再压一次
  （模组可能自己又拨上了）。后台采样循环上线前，**每次查询顺手评估**（含新周期/清零后的恢复）。

### 5.8 主动扫网（`model/netscan.rs` + `AtPort::command_cancellable`）

- `AT+COPS=?` 是全网搜索，可能几分钟；`minutes` 夹到 1~10。期间**独占该模组的 AT 口**，
  其它查询排队（与界面文案一致）。
- 必须可中断：`Modem` 上挂 `Arc<AtomicBool>` 取消标志，`AtPort::command_cancellable` 每轮读
  循环检查它；叫停后**已收到的半截应答照常解析**并返回 `stopped=true` + 说明文案。
- 制式只影响**结果过滤与步骤**（一次 `AT+COPS=?` 会把所有制式一起报回来）：切制式去改模组
  网络模式会让正在上网的设备掉线，不划算。`tech` 认不出时按 `both` 处理。
- 主动扫小区只有部分固件提供（芯讯通 `AT+CNWSEARCH`），按量纲定位解析（PLMN / 频点 / PCI /
  RSRP / RSRQ），认不出就给 `cell_note`，不假装有结果。

### 5.11 IMEI 写操作域（`model/imei.rs` 写侧 + `runtime/store.rs` ImeiStore + 两个 handler）

- **写**：`AT+EGMR=1,7,"<imei>"` 只对**移远 / 美格**（高通那套）下发，
  `AT+EGMREXT=1,7,"<imei>"` 只对**广和通 MTK 平台**下发；`imei_write_cmd()`
  认不出机型直接拒绝 —— 本程序不对认不出的机型发未经验证的厂商私有指令。
- **先备份再写**：第一次改写前先用**与写入指令同族的只读命令**快照当前值存进
  `imei.json`（广和通 MTK 走 `AT+EGMREXT=0,7`，其余走 `AT+CGSN`；台账 `source`
  记录是哪条命令读到的），两条路都读不到才拒绝改写；模组**明确应答空值算备份
  成功** —— 空 IMEI 模组正是最需要改写的维修场景。「已备份过」以 `saved_at`
  非空判定，**不能**以 `original` 非空判定（否则空原值会被下一次写入覆盖，
  一键恢复失效）。之后 `writes` 累计、`original` 保持不变。备份落盘失败只记
  warn，不阻断改写。
- **回读确认**：模组回 OK 不代表真写进去了，写完必须再读一次，不一致时的口径
  按平台分流 —— 广和通 MTK 的 `AT+EGMREXT` **需重启模组后才在射频 / 网络侧
  生效**（参考实现 1.0.14-r2 实机记录），回读到空值 / 旧值都可能是「还没生效」，
  此时不判失败、响应带 `warning` 提示重启后复查；其它厂商维持「不一致即报错」
  （界面上看到的一定是模组的真值）。
- **一键恢复**：`restore` 把备份的原始值写回去并记 `restored_at`；没有备份
  （`saved_at` 为空）或备份原值为空（模组出厂即无 IMEI，写空没有意义）时拒绝
  执行。`can_restore` = 备份存在且当前值与它不同。
- IMEI 入参只认 **15 位数字**（`is_imei_text`），本地校验不过就不下发。
- 验证：虚拟串口 + `fake_modem.py` 端到端（`AT+EGMR` 写 → `AT+CGSN` 回读 → 恢复），
  外加无模组时三个路由的 `modem not found` 信封核查；EGMREXT 只读形式与三态解析
  的实机取证见 §20。

### 5.12 设备级操作：重启与校时（`model/device.rs` + 两个 handler）

- **重启**：`AT+CFUN=1,1`（设置并复位）。关键口径：**指令写进串口即视为成功** ——
  重启会让 AT 口断开，模组还没回 OK 就掉线是常态，等不到确认不能算失败；
  用 `confirmed` 字段告诉界面「到底收没收到 OK」，`note` 说明原因。
- **校时**：`AT+CCLK="yy/MM/dd,HH:mm:ss+00"` 统一写 UTC（读侧 `parse_cclk` 也是按
  UTC 解释字段的，校完 `clock_skew_s` 应回到 0 附近）。个别固件不收时区尾巴：
  先发带 `+00` 的，回 ERROR 再退一格发不带时区的，两条都失败才算失败。
  返回体与 `GET /api/modem/status` 同构（前端拿它直接刷新 SIM/温度/时钟卡片）。
- 读侧修正：`AT+CCLK?` 的 `module_time` 去掉模组应答自带的引号（界面直显）；
  `/api/modem/status` 的漫游判定改走运行期覆盖值（`state.allow_roaming()`）。
- 验证：虚拟串口 + `fake_modem.py` 端到端三条（重启确认、校时写 UTC 并回读偏差归零、
  模拟不收时区的固件走回退分支）。

### 5.13 锁网写操作域（`model/lock.rs` 写侧 + 四个 handler）

- **锁频段**：`AT+QNWPREFCFG="<key>",<bands>`（key 带引号、值不带，冒号分隔、排序去重），
  芯讯通走 `AT+CSYSSEL`（逗号分隔）；写完**回读确认**（按频段集合比对，容忍模组改顺序）。
- **解除频段锁定**：QNW **绝不能写 0**（会把频段清空、直接脱网）——读 `policy_band`
  后**全量写回**；读不到 policy 就拒绝执行。芯讯通的解锁写法未验证，同样拒绝。
- **锁小区/频点**：`AT+QNWLOCK="common/4g",<type>,<earfcn>[,<pcid>][,<scs>]`，
  `lock_type` 0/1/2；QNWLOCK 置 0 是文档化的解锁（与频段不同，可以直接写）。
  芯讯通没有可用的锁小区指令，拒绝。
- **一键解除**：各技术域（QNW 含 NRNSA）频段回 policy + 小区置 0；芯讯通整体拒绝。
- **旧接口 `/api/lock/mode`**：复用 `mode` 域的 `set_mode_pref`（含回读确认），返回锁网状态。
- `tech` 归一化：LTE/NR 通用，NRNSA 只在分两套列表的 QNW 方言里合法。
- 验证：虚拟串口 + `fake_modem.py` 端到端（锁 3:8 / 78 / 41 → 读 policy 全量写回解除 →
  锁小区 1,2452,155 → 2,2452 → 0 → 一键解除），外加无模组时四个路由的信封核查。

### 5.14 AT 直发域（`/api/at` 字段对齐 + `/api/at/port` + `/api/at/presets`）

- **`/api/at` 对齐 Go `apiSendAT`**：`data:{cmd, resp}`，`resp` 是**应答行数组**
  （`split_at_lines`：去 `\r\n` 与空行；原来返回 `{response}` 单字符串）。校验：空 cmd →
  `cmd required`；含 `\r`/`\n` 或超 128 字符 → `cmd must be a single line (<=128 chars)`；
  非 POST → `method not allowed, use POST`。发送失败也回 HTTP 200 + `data:{cmd,resp:[]}` + 错误 msg。
- **厂商识别补齐七家**（`model/vendor.rs`，对齐 Go `DetectVendorFromATI` 顺序 quectel →
  fibocom → simcom → meig → tdtech → unisoc，先中先得）：新增**鼎桥**（MT5700）与**展锐**
  （UMS9620/MOCORTM/QOGIRN/SPRD）。识别流程同 Go `detectVendor`：ATI 厂商 Unknown 且应答是
  纯错误行时回落 `AT+CGMR`/`AT+CGMM` 重跑；型号取 `short_model_name`（三类写法：`Model:`
  标签 / 移远逐段 / 展锐标签版本块，认不出截断 40 字符）。
- **`/api/at/presets`**（`model/at/presets.rs`，新增）：3GPP 通用 28 条 + 六家私有表
  （移远 18 / 广和通 14 / 芯讯通 6 / 美格 7 / 鼎桥 10 / 展锐 38）。合并规则同 Go `Combine`：
  私有表同名（不分大小写）覆盖通用项；展锐抑制通用 `AT+CGCONTRDP`。无模组/解析失败回
  **code 0** + `{modem_id:-1, vendor:"unknown", 通用清单}`，msg 说明原因。
- **`/api/at/port`**（`http/handlers/at_port.rs` + `runtime/store.rs` 的 `AtPortStore`，
  台账 `atport.json` 与 config.toml 同目录）：键 = 模组身份（`imei:` 优先、`model:` 其次、
  兜底 `ports:<当前口>`），**不用端口号当键**；目标口必须真实存在（`port_exists`：字符设备）；
  非当前口先探 AT（已识别模组探不过即拒，未识别放行 `verified=false`）；空串 = 恢复自动探测。
  简化模型（一个 AT 口 = 一个模组）下「立刻切」= 整口换掉并重新识别（`Modem::repoint`）
  + 回收幻影模组；切口后身份键迁位、兜底键清除。
- **表单百分号解码修复**（`http/request.rs`）：`merge_form` / `parse_query` 原来不做百分号
  解码，前端用 `encodeURIComponent` 编码的模板指令（含 `"` `=` `,` 空格）到服务端会变形
  （`AT+CGSN` 变字面 `AT%2BCGSN`）。与 Go `ParseForm`/`URL.Query()` 对齐后修复（`+` → 空格、
  `%XX` → 字节、UTF-8 容错）。
- 验证：socat 虚拟串口 + `fake_modem.py` 端到端 4/4（ATI 识别 quectel + `RM500Q-GL`、
  `/api/at` 的 resp 行数组单行/多行、presets 私有表、repoint 换口重识别），单测 179 全绿；
  无模组时 curl 信封核查（method not allowed / cmd required / single line / presets
  兜底 code 0 / at/port not found），`%41`×130 解码后触发超长校验证实解码生效。

### 5.15 拨号域（`model/dial.rs` + `config/wan_mode.rs` + 六个 handler）

- **`GET /api/dial`**（`http/handlers/dial.rs`）：一次拉全 `modem_id / state / usb /
  class_notes / types / auto_wan / wan_ipv6 / dial_enabled / wan_mode / wan_mode_label /
  wan_modes`；`force=1` 重认 USB 类型（30 秒 TTL 缓存）。
- **USB 类型三路识别**（`Modem::usb_type`，对齐 Go `usbType`）：① 固件
  `AT+QCFG="usbnet"`（最权威）；② USB 描述符接口类码（`usb_descriptor_for_port`，
  经 sysfs 找 USB 祖先）；③ 主机侧网卡驱动（`usbnet_from_host_driver`：qmi_wwan→0 等）。
  取值表 5 条（0 QMI/NDIS、1 ECM、2 MBIM、3 RNDIS、5 NCM；MBIM `at_dial=false` 其余 true）；
  `dial_kind_from_name` **rndis 必须先于 ndis**（"RNDIS" 里含 "ndis"）。
- **拨号主流程**（`Modem::dial` → `dial_inner`，busy 锁防并发）：总开关拒 → cid(0~42)/apn/
  pdp 校验 → USB 类型分派：
  - QMI/MBIM → **明确拒绝**（数据呼叫归主机侧厂商拨号器；模组侧先激活会把拨号器请求顶掉，
    实测 RM502Q-GL `requestSetupDataCall` 被拒 QMUXError 0xe；托管属阶段 6，不假装成功）；
  - 其余 → ①写 APN（可选、失败不致命）→ ②`set_pdp`（reconnect 先断后连；失败且 apn 空时用
    上次从模组读出的 APN 补写重试——实测 FM350-GL 停用后 cid 从 `AT+CGDCONT?` 消失）→
    ②b IMS 激活（非致命）→ ③`AT+CGPADDR` 读本机地址（两候选：标准写法被拒（FM350 quirk）
    再试 `AT+CGPADDR=<cid>`；三种固件写法：两引号字段 / 16 十进制字节 / 零前缀过滤）→
    ④主机侧网卡候选（强/弱候选 + 拒绝名单，纯本机枚举不发 AT）→ connected。
- **`POST /api/dial/connect`**：成功 `ok, 已拨号 cid=N`；**失败也回 `data=DialState`**
  （`response::fail_data`，页面显示卡在哪一步）。
- **`POST /api/dial/disconnect`**：`AT+CGACT=0,<cid>` + 回读确认；QMI/MBIM 仅重置状态；
  `manual_stop=true` 让自动组网不再替用户拨回来。
- **开关三接口**（`enable` / `ipv6` / `mode`）：`on` 必须显式存在（缺 →
  `缺少 on（要 true 或 false）`）；写回 `[net] dial_enabled|wan_ipv6|wan_mode`
  （`patch::set_raw`，失败照 roam_set 先例 log warn 不阻断）+ 运行期覆盖即时生效；
  `WanMode` 三模式（modem_pro 默认 / dhcp / both + 别名 + Label + `list_payload`）。
- **状态同步**（`sync_dial_state`）：拨号/断开后顺手用 network_status 校准 APN/IP/网卡，
  **不额外发 AT**；WS 推送 `dial` + `network`（`push_dial`/`push_network`，模型层保持纯净）。
- 诚实降级记录：`auto_wan` → null（netprov 属阶段 5）；`host_ips` 暂空（Rust 无
  getifaddrs/netlink，`up` 用 operstate≠down 近似）；QMI/MBIM 不发 AT+CGACT；
  `query_cmd` 仅 AT 查询成功时置 `AT+QCFG="usbnet"`（广和通等方言未移植）。
- 验证：单测 210 全绿（dial 27 + wan_mode 4 + 既有 179）；socat 虚拟串口 +
  `fake_modem.py` 端到端 2/2（ECM 全流程：写 APN/激活 PDP/IMS 跳过/读地址/网卡候选 +
  断开回读 + 重拨 + 总开关拒 + cid=43 拒 + hangup(0) 落当前 cid；QMI 明确拒绝 + 断开仅重置），
  夹具按 `<state>,<cid>` 语义修正后停用/回读全部生效；`cargo build` 零告警。

### 5.16 定时器域（`model/schedule.rs` + `runtime/store.rs` SchedStore + `http/schedule.rs` + 五个 handler）

- **模型**（`model/schedule.rs`，对齐上游 schedule/sched_view/schedule_loop 三件）：
  `Task`（`modem_key` 用与配额同一套 `port:<口>` 键，避免同 IMEI 双模组串台）、
  `Action`（8 种 kind；`lock_type` 1=频点+PCI 2=仅频点；`persist` 走
  `AT+QNWLOCK="save_ctrl",1,1` best-effort，失败只 warn）、`Run`（phase/slot/at/ok/msg）、
  `KindInfo`/`ActionView`/`TaskView`（`#[serde(flatten)]` 同名键由视图层覆盖，
  前端拿到的就是带 `text` 的动作列表）/`SchedView`。
- **校验归一化**（`validate_task`/`validate_action`，错误文案逐字对齐 Go）：
  `parse_hhmm` 容忍 `8:00`；tech 别名（`5g`/`NR5G-SA`/`NSA`…）；频段排序去重统一冒号；
  `earfcn_valid` 按技术域取范围（LTE≤65535、NR≤3279165）；`looks_like_mode_code`
  「原码形态」判定（纯数字 / 0x 十六进制 / 带分隔符 / 纯字母缩写放行，
  `5G`/`4g`/含空格等号叹号中文一律拒绝，与上游 TestLooksLikeModeCode 同批向量）；
  模式收 7 档语义值或原码；`days` 0~6 去重排序；任务名缺省 = 动作文案 + 开始时间。
- **时间判定**（全部走**本机时区**）：`logging/time.rs` 新增 `tz_offset_secs()`
  （`TZ` 环境变量 → `/etc/TZ` 兜底；只认 POSIX std offset（西为正需翻转），
  不解析 TZif、不套 DST，认不出按 UTC 并 warn）+ `local_unix_now()`；
  `due_phases`：槽位 ≤ now < 槽位+15min（`CATCH_UP`）且星期匹配且 `slots[phase]` 非本槽位；
  `next_runs` 8 天窗口取 3 条；`times_text` = `MM-DD HH:MM 周X`。
- **台账**（`runtime/store.rs::SchedStore` → `schedule.json`，与配置同目录）：
  version:1 + saved_at + tasks；`.tmp`+rename 原子写；载入时逐条 `validate_task`
  （无 id / 校验不过的跳过并 warn）；`upsert` 保留 created_at/slots/last 不被前端覆盖；
  `record` 推进槽位（`manual` 不推进）+ 5 秒落盘节流；`retarget` 换端口迁移；
  `log_allowed` 同任务 5 分钟日志节流。
- **执行**（`Modem::apply_sched_action` 全部复用既有实现，不另开下发路径）：
  band_lock/unlock → `set_band_lock`（解锁按 policy 全量写回）；cell_lock/unlock →
  `set_cell_lock`；unlock_all；mode_pref → `set_mode_pref`；airplane → `set_airplane`。
  执行前 `sched_action_supported` 能力预检：needs_locker 看**真被固件接受过**的方言探测
  （`DialectInfo` 新增 `probed` 字段区分「探测选中」与「猜测兜底」）、needs_mode 看
  mode 指令可用性（30 秒 TTL 缓存）、`airplane_on` 机型不支持时提前标
  （`airplane_blocked_reason` 统一文案；`airplane_off` 永远放行——它是恢复手段）。
  `Modem` 新增 `imei_cache`（`repoint` 时作废）与两个 30 秒 TTL 能力缓存槽。
- **循环与视图**（`http/schedule.rs` + `runtime/scheduler.rs::spawn_schedule`）：
  `[server] schedule_interval`（默认 10 秒）tick；启动先补跑一轮；**不看有没有客户端在线**；
  目标解析先按端口键精确匹配、miss 且 IMEI 唯一匹配才迁移键（两台相同不猜）；
  离线 / 串口未开记 `last` 但不动动作；一槽内动作依次下发、单个失败不影响后续，
  结果汇总「已执行 N/M 个动作；失败：…」；变化即 `schedule` 推送 + flush。
  `run`（立即执行）跑开始动作并把当前窗口槽位标记已跑；模组拒绝时 `fail_data` 带视图。
- **验证**：单测 240 全绿（schedule 模型 26 + SchedStore 3 + 时区 1 + 既有 210）；
  无模组 curl 信封核查：set（含 legacy 单动作、`8:00` 归一化、tech 别名、自动任务名）、
  toggle/delete 不存在文案、run 离线、非法时间/缺目标/未知动作逐条对齐；
  补跑窗口实测（任务建在窗口内 → 一轮 tick 后 `last`=「目标模组当前不在线，已跳过」
  且槽位去重生效）；`TZ=CST-8` 下 next_runs 输出本地星期；重启后台账读回；
  `cargo build` 零告警。

### 5.17 USB/PCIe/atport 绑定（`modem/usbdev.rs` + `modem/pcidev.rs` + `runtime/store.rs` + 若干 handler）

- **`/api/netinfo`**（`model/network.rs::net_info` + `handlers/netinfo.rs`）：按厂商选指令
  （TDTech `AT^SYSINFOEX`；Unisoc 回空串、不发空指令；其余 `AT+QNWINFO`），
  返回原始应答行用 `|` 连接（含回显与 OK）。
- **USB 视图**（`modem/usbdev.rs`）：已知身份表（移远/广和通/鼎桥/芯讯通/美格）+
  模组类码判定；事件环形缓冲 60 条；`usb_hotplug_info(auto_bind)` 输出
  `platform/auto_bind/can_bind/bind_note/devices/events/advice`；unix 走 sysfs
  `/sys/bus/usb/devices` 枚举，非 unix 空清单。`/api/usb` 读、`/api/usb/clear` 清并广播 `usb`、
  `/api/usb/bind` 手动补 usb-serial 驱动（`new_id`）。
- **PCIe 视图**（`modem/pcidev.rs`）：`PCIE_MODEM_DRIVERS` 与厂商名表；`/api/pcie` 列设备
  （绑驱动或「未绑驱动 + 芯片厂 VID + 无线控制器类」）+ AT/QMI/MBIM 候选口；
  `/api/pcie/reset` remove + 2s 等待 + rescan；`/api/pcie/bind` 绑驱动（driver 空则挨个试）。
  `catalog_models` 字段暂缺（按 `omitempty` 省略）。
- **atport 绑定**（`AtPortBindStore`（`atport_bind.json`）+ `handlers/atport_bind.rs`）：
  GET 列 `{supported,path,ports,binds}`（每项带 `present`）；POST `{vid,pid,port}`
  （port 空=解绑），键归一 `vid:pid`（每段 4 位十六进制），改动后对已发现模组切口并重发现。
  `ModemManager::apply_saved_at_port` 改为**先按 vid:pid 绑定**、再按模组身份 `atport.json`。
- **验证**：单测 251 全绿（含 `at_port_bind_key` 归一、`AtPortBindStore` 往返、PCIe BDF/驱动/glob 判定）；
  `cargo build` 零告警。

### 5.18 推送域收口 + USSD 下发（`http/push.rs`、`http/ws.rs`、`model/ussd.rs`、`http/handlers/ussd.rs`）

- **推送触发点补齐**（写操作成功后顺手广播，免得各页面各查一遍）：
  `/api/cells`（真扫后推 `cells`）、`/api/lock/{band,cell,mode,unlock}`（成功后推 `lock`）、
  `/api/sms/read`（推 `sms` + `smscount`）、`/api/quota/{set,reset,unblock}`（推 `quota`）、
  `/api/traffic/reset`（推 `traffic`）、`/api/ussd` POST 同步收到 `+CUSD` 时推 `ussd`。
- **短信推送闸门**：`WsHub::broadcast_sms(msg_type, modem_id, data, allow)` 按连接过滤；
  `sms` 带正文，与 `/api/sms/read` 共用同一把令牌（`smsauth::token_valid` 为此开放）；
  `smscount` 只有 `{total,unread}`、不含正文，因此不过闸门。
- **USSD 域**（`model/ussd.rs` 重写）：`valid_ussd_code`（数字与 `* # +`，1~64）、
  `parse_cusd_line`（引号定界，正文可带逗号；UCS2 按短信同一套判据还原）、
  `Modem::send_ussd`（校验 → `AT+CPIN?` 判 PIN 锁 → 清旧缓存 → `AT+CUSD=1,"<code>",15`
  → 应答含 error 时报 CS 域说明；同步到达的 `+CUSD` 就地收下并推送）、
  `Modem::sim_pin_locked` / `Modem::clear_ussd`；5 个单测覆盖字符集、状态码、
  正文带逗号、UCS2 还原与纯数字不误解码、`\r\n` 前缀容错。
- **`/api/ussd` 契约修复**：GET 回 `{modem_id, result}`（`result` 可为 null）；
  POST `{modem_id, code}` 成功回 `msg:"已下发，等待模组应答（异步推送）"`；
  路由总表补 `POST /api/ussd` 的 dispatch（此前落到未迁移兜底）。
- **SIM 热插拔连带动作**：换卡 / 拔卡时清短信缓存（旧卡的短信不能挂在别人页面上），
  补推空 `sms` + `smscount`（口径：先清缓存再推，所以角标同步归零）。
- **验证**：单测 276 全绿、`cargo build` 零告警。闸门行为有 3 个专测（`http/ws.rs`：
  真实 TCP 对上按令牌过滤、无闸门广播人人可收、对端已关时不带崩广播）。
- **`/api/traffic/reset` 的推送不过滤**：周期循环里的 `push_traffic` 会跳过「不上报计数器」
  的模组（口径同 Go 的 TrafficLoop），但 reset 是用户刚按了按钮，必须给回响，
  所以那里直接广播、不走 `push_traffic`（口径同 Go 的 apiResetTraffic）。

### 5.19 SIM 热插拔巡检（`model/sim.rs` + `http/loops.rs::spawn_sim_watch` + `handlers/sim.rs`）

- **为什么要兜底轮询**：不少固件「悄悄换了卡却不发 `+CPIN:` URC」，页面上的 ICCID/运营商
  还是旧卡的；`AT+CPIN?` + ICCID/IMSI 比对是唯一可靠判据。
- **判定口径**（`model/sim.rs`）：只有 **ICCID/IMSI 确实变了**才算换卡；
  读不到确定答复（`AT+CPIN?` 无应答）时**不判任何事件** —— 模组复位 / USB 重新枚举期间
  CPIN 一定读不到，把它当拔卡会引出无限复位（真机事故：模组每 2~3 分钟掉一次线）。
- **事件三态**：`removed` / `inserted` / `swapped`；身份没读到过旧值的只能算 `inserted`，
  **绝不能当换卡去复位模组**。
- **复位开关**：`[sim] hotswap_reset`（默认 false）。只有确认换卡**且**用户显式打开才
  `AT+CFUN=1,1`；两次动作之间至少隔 `SIM_COOLDOWN`（30 秒）。
- **巡检节拍**：循环本身 2 秒一拍（`SIM_WATCH_TICK`），但**只有到点或收到 URC 置脏才真发 AT**，
  所以平时不占串口；`[sim] hotswap_interval` 缺省 `20s`，写 `"0s"` 关掉检查。
  置脏来源是 `+GTSIM`（广和通 MTK 的 SIM 状态上报），由 §21 的 `spawn_urc` 调
  `mark_sim_dirty()` —— **不是** `+CPIN`，它与查询应答同形、过滤层刻意不吞。
- **连带动作**：换卡 / 拔卡时清短信缓存并补推空 `sms` + `smscount`（旧卡的短信不能挂在别人页面上），
  再推 `sim` 事件（载荷只带 `kind` 与身份，刻意做小）。
  （相反方向由 §22 的 `+CMTI` 负责：来新短信时重读并推上去。）
- **接口**：`GET /api/sim` 增补 `last_event` / `last_event_at` / `checked_at`（没有过事件就不出现）。
- **验证**：6 个单测覆盖在否判定、身份比对、事件三态、复位开关、节拍与冷却。

### 5.20 实机事故修复：配置写坏导致服务起不来（`config/patch.rs` 用法 + `config/loader.rs`）

- **现象**：实机上 modemd 进 crash loop，`/api/*` 全部连不上；syslog 里是
  `TOML parse error at line 106: wan_mode = dhcp / invalid string`。
- **根因**：`handlers/dial_mode.rs` 用 `patch::set_raw` 写 `[net] wan_mode` ——
  `set_raw` 是「原样写」（给 `true`/`false`/数字用的），字符串值必须走 `set_string`
  （自动加引号并转义）。少两个引号，配置文件就成了非法 TOML。
- **修复一（根因）**：`dial_mode.rs` 改调 `patch::set_string`。审计了其余 6 处
  `patch::*` 调用，只有这一处把字符串当原样写，其余都是布尔值。
- **修复二（兜底）**：`config/loader.rs` 不再一坏就退出 —— 按
  「主配置 → `config.toml.bak` → 内置默认值」依次回落，每级把原因打到 stderr
  （procd 收进 syslog）。默认值那一级方向是**收紧**：`require_luci_login` 与
  `sms_password_required` 默认都开着，不会因为配置坏了就敞开。
  配套 4 个单测：最小配置解析与默认值、坏配置回落 `.bak`、坏配置无备份回落默认值、
  文件不存在回落默认值。
- **附带**：`ServerConfig::default()` 里硬编码的 `/var/log/modemd.log` 与
  `/var/run/modemd.pid` 改走 `loader::default_*` 的平台判定（Windows 上不再往
  `/var/log` 写），符合「禁止硬编码平台路径」规范。
- **处置记录**：设备上的配置是按「下载 → 本地改 → 上传」修好的（设备没有 `base64`，
  sed/printf 的引号经 SSH + PowerShell 两层转义会被吃掉，行级拼接反而容易丢行）。
- **教训**：写回用户配置的代码，值类型必须与写入函数匹配；这类「改一个字让服务起不来」
  的路径要有兜底，否则现场只能上机救。

---

## 6. 已完成：只读接口（阶段 2）

约定：响应信封 `{"code":i64,"data":any,"msg":string}`，成功 `code:0`；
业务失败默认 **HTTP 200**；一律 `Cache-Control: no-store`；参数惯例 `?modem_id=N`（缺省 0）。

| 接口 | 方法 | handler 文件 | 模型 / 关键实现 | 状态 |
| --- | --- | --- | --- | --- |
| `/api/version` | GET | `version.rs` | `{version,go,platform,luci_auth}` | ✅ |
| `/api/session` `/api/login` `/api/logout` | GET/POST | `session.rs` | ubus 会话 | ✅ |
| `/api/modems` | GET | `modems.rs` | `ModemManager::list()`；`?discover=1` 重探 | ✅ |
| `/api/modem/info` | GET | `modem_info.rs` | `model/imei.rs`：IMEI/CGMR/厂商名 | ✅ |
| `/api/modem/status` | GET | `modem_status.rs` | `model/siminfo.rs`：ICCID/IMSI/号码/温度/`+CBC` 电压/`+CCLK?`/选网/漫游 | ✅ |
| `/api/sim` | GET | `sim.rs` | `+CPIN` 等 | ✅ |
| `/api/vendor/cmds` | GET | `vendor_cmds.rs` | `vendor.rs`：指令集与中文标签 | ✅ |
| `/api/signal` | GET | `signal.rs` | `model/signal.rs`：`CSQ` + `QENG servingcell` → `CESQ`（含 Fibocom SSB 扩展） | ✅ |
| `/api/network` | GET | `network.rs` | 基础注册状态 | ✅ |
| `/api/network/status` | GET | `network_status.rs` | `model/network.rs`：plmn/tech/band/apn/ipv4/cs·ps_reg/roaming + `traffic` | 🚧 ca/serving/ambr 待补 |
| `/api/cells`（`?cache=1`） | GET | `cells.rs` | `model/cells.rs`：`QENG`/`FENG` serving+neighbour、`+CPSI`；缓存 | ✅ |
| `/api/cells/all` | GET | `cells_all.rs` | 各模组扫描缓存 | ✅ |
| `/api/lock/status`（`?refresh=1`） | GET | `lock_status.rs` | `model/lock.rs`：方言探测 + 频段/小区锁定 + 30s 缓存 | 🚧 覆盖 QNW/芯讯通方言 |
| `/api/lock/caps` | GET | `lock_caps.rs` | 同左，`probes[]` + `dialect_tried[]` | 🚧 同上 |
| `/api/params`（读） | GET | `params.rs` | `model/params.rs`：`CGDCONT?`+`CGACT?`+`COPS?` | ✅ |
| `/api/params/apn` | POST | `params_apn.rs` | 写 APN + 回读 + 重新激活 + IMS | ✅ |
| `/api/params/apn/delete` | POST | `params_apn_delete.rs` | `AT+CGDCONT=<cid>` 删除并回读 | ✅ |
| `/api/params/pdp` | POST | `params_pdp.rs` | `AT+CGACT` 激活/停用/先断后连 | ✅ |
| `/api/params/netselect` | POST | `params_netselect.rs` | `AT+COPS=0` / `=1,2,"<plmn>"` | ✅ |
| `/api/mode/set` | POST | `mode_set.rs` | `AT+QNWPREFCFG="mode_pref"` / `AT+CNMP`，回读确认 | ✅ |
| `/api/usbnet/set` | POST | `usbnet_set.rs` | `AT+QCFG="usbnet"`，回读确认 | ✅ |
| `/api/airplane/set` | POST | `airplane_set.rs` | `AT+CFUN=4` / `=1` | ✅ |
| `/api/roam/set` | POST | `roam_set.rs` | 写回 `[net] allow_roaming` + 运行期覆盖 | ✅ |
| `/api/mode`（读） | GET | `mode.rs` | `model/mode.rs`：`QNWPREFCFG mode_pref` / `CNMP?` | ✅ |
| `/api/usbnet`（读） | GET | `usbnet.rs` | `model/usbnet.rs`：`QCFG="usbnet"` | ✅ |
| `/api/imei`（读） | GET | `imei.rs` | `model/imei.rs`：`ImeiStatus` | ✅ |
| `/api/imei/set` | POST | `imei_set.rs` | `model/imei.rs`：先备份 → `AT+EGMR=1,7,"…"` → `AT+CGSN` 回读确认 | ✅ |
| `/api/imei/restore` | POST | `imei_restore.rs` | 写回 `imei.json` 备份的原始值，记 `restored_at` | ✅ |
| `/api/modem/reset` | POST | `modem_reset.rs` | `AT+CFUN=1,1`；等不到确认按已下发上报（`confirmed`） | ✅ |
| `/api/modem/clock` | POST | `modem_clock.rs` | `AT+CCLK` 写 UTC（带回退），返回整份模组状态 | ✅ |
| `/api/lock/band` | POST | `lock_band.rs` | `QNWPREFCFG`/`CSYSSEL` 写频段 + 回读；解除读 policy 全量写回 | ✅ |
| `/api/lock/cell` | POST | `lock_cell.rs` | `AT+QNWLOCK` 锁频点/PCI（0/1/2），回读确认 | ✅ |
| `/api/lock/mode` | POST | `lock_mode.rs` | 旧接口：复用 `set_mode_pref` | ✅ |
| `/api/lock/unlock` | POST | `lock_unlock.rs` | 一键解除（频段回 policy + 小区置 0） | ✅ |
| `/api/airplane`（读） | GET | `airplane.rs` | `model/airplane.rs`：`CFUN?` | ✅ |
| `/api/roam`（读） | GET | `roam.rs` | `model/roam.rs`：注册 + IMSI + PLMN → `effective` 判定 | ✅ |
| `/api/ussd`（读） | GET | `ussd.rs` | `model/ussd.rs`：读每模组缓存（URC 写缓存属阶段 5） | 🚧 POST 待补 |
| `/api/logs` | GET | `logs.rs` | 从日志文件尾部读取 | ✅ 基础 |
| `/api/modules` | GET | `modules.rs` | 内置模块清单 | ✅ 基础 |
| `/api/at` | GET/POST | `at.rs` | 发 AT；响应字段待对齐 | 🚧 |
| `/api/sms/auth` | GET | `sms_auth.rs` | `smsauth.rs`：`required`/`unlocked`/`default`/`ttl_s`/`outer_auth` | ✅ |
| `/api/sms/unlock` | POST | `sms_unlock.rs` | 校验密码发令牌；连错 5 次锁 60s（HTTP 403/429） | ✅ |
| `/api/sms/lock` | POST | `sms_lock.rs` | 吊销当前令牌 | ✅ |
| `/api/sms/read` | GET | `sms_read.rs` | `model/sms.rs`：PDU/文本解码、去重、拼接、按时间倒序 | ✅ |
| `/api/sms/storage`（读） | GET | `sms_storage.rs` | `model/sms.rs`：`CPMS` 支持区 + 各区用量 + 当前区 | ✅ |
| `/api/sms/send` | POST | `sms_send.rs` | `model/sms.rs`：文本模式 `AT+CMGS` 两步下发，UCS2 + 分段 | ✅ |
| `/api/sms/delete` | POST | `sms_delete.rs` | `AT+CMGD=<index>` / `AT+CMGD=1,4` | ✅ |
| `/api/sms/password` | POST | `sms_password.rs` | `config/patch.rs` 行级写回 + 令牌吊销 | ✅ |
| `/api/sms/storage`（写） | POST | `sms_storage_set.rs` | `AT+CPMS` 切换 + 回读 + 写回配置 | ✅ |
| `/api/signal/history` | GET | `signal_history.rs` | `model/history.rs`：环形采样历史（1800 段/模组） | ✅ |
| `/api/changes` | GET | `changes.rs` | `model/changes.rs`：阈值 + 30s 节流 + 品级文案 | ✅ |
| `/api/changes/clear` | POST | `changes_clear.rs` | 连基线 + 采样历史一起清，并广播 `changes_clear` | ✅ |
| `/api/quota` | GET | `quota.rs` | `http/quota.rs`：采样 + 周期滚动 + 用量累计 + 超限评估 | ✅ |
| `/api/quota/set` | POST | `quota_set.rs` | 保存设置（含 `used_gb` 校正）并按新口径评估 | ✅ |
| `/api/quota/reset` | POST | `quota_reset.rs` | 清零并起新周期 + 恢复连接 | ✅ |
| `/api/quota/unblock` | POST | `quota_unblock.rs` | 本周期不再限制 + 恢复连接 | ✅ |
| `/api/traffic/reset` | POST | `traffic_reset.rs` | 清模组累计计数器，返回 `TrafficView` | ✅ |
| `/api/netscan`（`?tech=&minutes=`） | GET | `netscan.rs` | `model/netscan.rs`：`AT+COPS=?` + `AT+CNWSEARCH` | ✅ |
| `/api/netscan/stop` | POST | `netscan_stop.rs` | 置取消标志，立即返回 `{stopped}` | ✅ |
| `/api/netinfo` | GET | `netinfo.rs` | `model/network.rs`：`AT+QNWINFO`（TDTech `^SYSINFOEX`；Unisoc 回空） | ✅ |
| `/api/usb` | GET | `usb.rs` | `modem/usbdev.rs`：设备清单 + 事件 + 补驱动能力 | ✅ |
| `/api/usb/clear` | POST | `usb_clear.rs` | 清事件并广播 `usb` | ✅ |
| `/api/usb/bind` | POST | `usb_bind.rs` | `ensure_serial_tty` 补 usb-serial 驱动 | ✅ |
| `/api/pcie` | GET | `pcie.rs` | `modem/pcidev.rs`：驱动/设备/端口/复位能力（`catalog_models` 暂缺） | ✅ |
| `/api/pcie/reset` | POST | `pcie_reset.rs` | remove + rescan 软复位 | ✅ |
| `/api/pcie/bind` | POST | `pcie_bind.rs` | 绑驱动（driver 空则按已知驱动挨个试） | ✅ |
| `/api/atport/bind` | GET/POST | `atport_bind.rs` | `AtPortBindStore`（`atport_bind.json`）：按 `vid:pid` 钉死命令口 | ✅ |

未迁移（阶段 2 剩余）：仅 `/api/wechat/*`、`/api/host/nic/*`（前端已下线、保留）。

---

## 7. 领域模型与 JSON 契约（`modem/model/*`）

> 字段名严格对齐前端，`omitempty` 语义用 `#[serde(skip_serializing_if = ...)]` 复现。

| 模块 | 主要类型 | 要点 |
| --- | --- | --- |
| `signal.rs` | `SignalInfo{rssi,ber,nr_rsrp?,nr_rsrq?,nr_sinr?}` | `rssi` 是 CSQ 索引（99/255=未上报）；哨兵值过滤与前端一致 |
| `network.rs` | `NetworkInfo` / `NetworkStatus` | `NetworkStatus` 核心字段见 PORTING；`tech_label`/`band_label` 中文；`net_info()` 回 `AT+QNWINFO` 原始行（`|` 连接） |
| `regstate.rs` | `RegState{stat,text,registered,roaming,lac?,cell_id?,act?,raw?}` | `+CREG/+CGREG/+CEREG/+C5GREG` |
| `netselect.rs` | `NetSelect{mode,mode_text,plmn?,operator?,act?,raw?}` | `AT+COPS?` |
| `params.rs` | `Context{cid,pdp_type,apn,active,internal}` / `ParamsStatus` | 内部 APN（ims/sos/mms）不上网 |
| `siminfo.rs` | `ModemStatus` / `Sensor` / `RoamState` | ICCID/IMSI/CNUM/`CBC`/`CCLK`/`QTEMP`/`CPMUTEMP`；运营商表 |
| `imei.rs` | `ModemInfo` / `ImeiStatus` | `AT+CGSN`/`CGMR`，广和通 MTK 另有同族只读 `AT+EGMREXT=0,7`；`ImeiStatus{supported,cmd,note,imei?,orig?,warning?}`（`warning` 仅 set/restore 当次响应携带） |
| `cells.rs` | `CellInfo` / `CellScanResult` | 见 §8 解析口径；含缓存 |
| `lock.rs` | `BandLock`/`CellLock`/`LockStatus`/`LockCaps`/`LockCapProbe` | 方言 `qnw`/`simcom`；`band_locked` 用 policy 基准 |
| `mode.rs` | `ModePref{value,value_text,options[],supported,raw?,note?}` | |
| `usbnet.rs` | `UsbNetMode{value,value_text,options[],...}` | 0=RMNET/1=ECM/2=MBIM/3=RNDIS |
| `airplane.rs` | `Airplane{supported,enabled,reason?,raw?}` | `CFUN: 4` = 飞行模式 |
| `roam.rs` | `RoamView{roaming,roam_scope,home_mcc?,visit_mcc?,allowed,applies,effective,reason?}` | 关开关 + 国际漫游 ⇒ `effective=false` |
| `ussd.rs` | `UssdResult{at,status,text,raw?,sperror?}` | `+CUSD` 是 URC，缓存写在 `Modem` |
| `sms.rs` | `Sms` / `SmsPart` / `SmsStorage` / `ConcatInfo` | PDU（DELIVER/SUBMIT）解码、GSM7/8bit/UCS2、长短信 UDH 拼接与按时间倒序；`+CPMS?` 支持区与用量 |
| `traffic.rs` | `TrafficSample` / `TrafficView` / `TrafficPrev` | 计数式 `+QGDCNT`（取一行里最后两个数）/ 速率式 `+GTSTATIS`（bps）；速率来自差分或模组直报 |
| `quota.rs` | `QuotaSettings` / `QuotaState` | 周期（每月 `reset_day`）、字节/百分比口径、用量拆分与超限判定；PDP 断网动作（跳过 ims/sos/mms） |
| `netscan.rs` | `Operator` / `NetScanResult` | `AT+COPS=?` 括号分组解析、ACT→制式；`+CNWSEARCH` 按量纲定位；扫描可中断 |
| `changes.rs` | `Tracker` / `ChangeEntry` / `ChangeState` | 波动记录：阈值 + 30s 节流 + 品级文案（`rsrp/rsrq/sinr_grade`）；环形 500 条 |
| `history.rs` | `HistoryStore` / `Sample` | 采样历史：每模组 1800 段，`modem_id<0` 归并全部模组 |
| `modem.rs` | `Modem`（核心对象） | 懒开 AT 口；缓存：`ussd`/`cells`/`lock_dialect_slot`/`lock_status_slot`/`sms`（`SmsCache`）/`traffic_prev`；`at_cmd()` 统一执行；`netscan_stop` 取消标志 |

支撑模块（非 JSON 契约）：`modem/usbdev.rs` 的 `ModemUsbDevice`/`UsbEvent`/`UsbHotplugView`
（USB 已知身份表、事件环形缓冲 60 条、`list_modem_usb_devices`/`ensure_serial_tty`/`usb_hotplug_info`）；
`modem/pcidev.rs` 的 `PciDevice`（PCIe 设备枚举、`PCIE_MODEM_DRIVERS`、厂商名表、复位/绑驱动；
`catalog_models` 字段暂缺）。

`Modem` 关键方法：`vendor()`、`model()`、`view()`、`with_at()`、`identify()`、`at_cmd()`、
`at_cmd_cancel()`、`ussd_last()/set_ussd()`、`cells_cache()/set_cells_cache()`、`scan_cells()`、
`cells_cached()`、`lock_dialect()/lock_status()/lock_caps()`、`signal()/params()/mode_pref()/
usbnet()/airplane()/roam_view()/imei_status()/modem_status()`、`traffic()/traffic_view()/
reset_traffic_counter()`、`data_cids()/block_data()/unblock_data()/set_airplane()`、
`scan_network()/request_netscan_stop()` 等。

---

## 8. AT 解析口径（真机验证，**不得回退**）

- `AT+CSQ` → `(rssi, ber)`；`99`/`255` = 未上报。
- `AT+CESQ`：`rsrq 0..34 → (v-39)/2`，`rsrp 0..97 → v-140`；
  **Fibocom 扩展第 7~9 格**（NR SSB）：`ss_rsrp = v-157`、`ss_rsrq = v*0.5-43.5`、
  `ss_sinr = v*0.5-23.5`；仅当前 6 格 rsrq/rsrp 都「未上报」时启用扩展。
- `+QENG="servingcell"`：NR5G SA `idx12/13/14 = RSRP/RSRQ/SINR`；LTE `idx13/14/16`。
- 小区/邻区：**按量纲定位**信号值（`find_rsrp_token` → `apply_signal_fields`），
  `TAC`/`CellID` 按**十六进制**解析（`parse_at_int` 先十进制后十六进制）；
  `band_from_earfcn` 兜底频段；`valid_plmn` 规则 = `mcc` 3 位数字且非全 0、`mnc` 1~3 位数字。
- 兼容「标记行 + 数据行」分开的服务小区写法（RM500U-CNV）；`+CPSI` 支持裸数字频段写法。
- 频率名写法（`NR5G_BAND78`）与 `+GTCCINFO` 等其它厂商写法**待补**（见 §10）。
- 单位口径：`rssi` 存 CSQ 索引；RSRP/RSRQ/RSSI/SINR 存 dB/dBm 整数。
- `AT+QGDCNT?`：`+QGDCNT: <tx>,<rx>`（部分固件多带 cid：`+QGDCNT: <cid>,<tx>,<rx>`）⇒
  取一行里**最后两个**数字；清零写法 `AT+QGDCNT=0`。
- `AT+GTSTATIS?`（广和通）：`+GTSTATIS: <cid>,<tx_bps>,<rx_bps>`，只有瞬时速率、没有累计计数器
  ⇒ 本地积分，标 `estimated`。
- `AT+COPS=?`：`+COPS: (<stat>,"<long>","<short>","<plmn>",<act>),...` 按括号分组解析；
  `act` 7=LTE、12=NR、2/4/5/6=WCDMA、0/1/3=GSM（3GPP TS 27.007 的 `<AcT>`）。

---

## 9. 构建 / 测试 / 验证

```
# 1) 编译与单测（在 rust/ 目录）
cargo build          # 必须零告警
cargo test           # 当前 554 项全绿

# 2) 运行自测（无模组也能验证路由接线）
#    临时配置（示意）：listen 改 127.0.0.1:19527，[ui] require_luci_login = false
modemd.exe -config <临时 config.toml>
#    逐个请求：登记接口应 200；未登记路径应 404（没有 501 占位了）
curl http://127.0.0.1:19527/api/version
curl http://127.0.0.1:19527/api/lock/caps?modem_id=0

# 3) 打包验证：见 docs/BUILD.md（SDK 编译 + 安装 + /etc/init.d/modemd 启动）
```

已迁移接口在无模组时的标准响应：`{"code":-1,"data":null,"msg":"modem not found"}`（HTTP 200）。
`/api/cells/all` 无模组时：`{"code":0,"data":[],"msg":"ok"}`。

---

## 10. 剩余工作（按建议顺序）

1. ~~**`/api/sms/read` + `/api/sms/storage`（GET）**~~ **已完成**：PDU/文本解码、存储区用量、
   密码闸门（`auth`/`unlock`/`lock`）已落地并通过实机自测（无模组时 `modem not found`）。
   剩余：`/api/sms/send`、`/api/sms/delete`、`/api/sms/password`（需配置写回）、`/api/sms/storage`(POST)。
2. ~~**`/api/changes` + `/api/signal/history`**~~ **已完成**：共享历史存储（`modem/metrics.rs`）、
   阈值/节流、`changes_clear`、`changes` 推送已接线。缺的是**阶段 5 的后台采样循环**
   （`change_interval`）——那之前「没人在看」时历史不连续，属预期。
3. ~~**流量配额域（`/api/quota` 读 + `quota/set|reset|unblock` + `traffic/reset`）**~~
   **已完成**：`[quota]` 默认段 + `quota.json` 台账 + 计数式/速率式两种计量口径。
   缺的是**阶段 5 的后台采样**：现在由每次查询顺手累计与评估（含超限压制），
   后台循环起来后改由它按 `traffic_interval` / `block_interval` 驱动。
4. ~~**`/api/netscan`(+stop)**~~ **已完成**：`AT+COPS=?` 全网搜索（可中断）+ 芯讯通
   `AT+CNWSEARCH` 小区；`minutes` 夹到 1~10。待真机校正：`+CNWSEARCH` 的字段量纲。
5. **阶段 3 写操作域（续）**：~~`at`(发 / 响应字段对齐 / port / presets)~~ **已完成**（见 §5.14）→
   ~~`host/nic`(GET + apply + save)~~ **已完成**（见 §28）→
   ~~`dial`~~ **已完成**（见 §5.15）→
   ~~`schedule(set/delete/toggle/run)`~~ **已完成**（见 §5.16）→
   ~~`usb/pcie/atport` 绑定~~ **已完成**（见 §5.17）。
   （~~`usbnet/set`、`mode/set`、`airplane/set`、`roam/set`、`imei(set/restore)`~~
   ~~`modem(reset/clock)`、`lock(band/cell/mode/unlock)`~~ 已完成，见 §5.9 / §5.11 /
   §5.12 / §5.13；`quota(set/reset/unblock)`、`traffic/reset`、`changes/clear`、
   `sms(send/delete/password/storage)`、`params(apn/pdp/netselect)` 均已完成。）
6. ~~**阶段 4 推送**~~ **已完成**（见 §5.18）：各 `push_*` 全部接到 `WsHub`，
  写操作成功后顺手广播；短信推送过密码闸门；**屏端动作协议已完成**（见 §27）。
7. ~~**阶段 5 后台循环**~~ **已完成**：信号/网络/流量采样、配额评估、~~定时任务~~（见 §5.16）、SIM/USB 热插拔、
  ~~组网 netprov~~（见 §23）、~~企业微信~~（见 §24）、历史记录（`change_interval`）均已接线，
  实机日志「后台循环已启动：8 个采样线程」；屏端动作协议收口见 §27。
  （SIM 热插拔巡检已接线，见 §5.19；USB 热插拔推送、~~netprov~~（见 §23）、~~企业微信~~（见 §24）待补。**URC 巡检加速与屏端推送均已接线**（见 §21 / §22）：过滤层暂存的主动上报会折算成「SIM / 网络 / 短信置脏」，`+CUSD` 应答直接推页面，`+CMTI` 触发一次短信重读。）
8. ~~**阶段 6 拨号器托管**~~ **已完成**（2026-10-18，见 §25）：`hostdial` 按厂商启停
   拨号器（含 udhcpc；QMI/MBIM 的数据呼叫即由它接管，`/api/dial/connect` 对这两类
   已从「拒绝」改为「委托」）。**实机未验证**：四家拨号器源码在 `third_party/`（构建时按 `Config.in` 裁剪），命令行
   参数照配置注释拼的，集中在 `runtime/dialer/mod.rs::plan_dialer`。
9. ~~**LuCI 退役 Lua 世代**~~ **已完成**（2026-10-18，细节见 §11）：删
   `luci/app/lua/**`，JS 视图内联 i18n（zh-cn/zh-tw/en），并修掉 JS 世代
   三个实证缺陷（`getInitList` 的 `expect` 单键选择器把返回削成空数组、
   运行态读了个不存在的字段、ucode 控制器忘了在 menu.d 注册节点）；
   菜单标题维持英文（固件无 `po2lmo`，造不出 `.lmo`）。

其它待补：`+CPSI` 带名频段写法、`^CELLINFO`/`AT!GSTATUS` 等厂商私有小区写法
（`+GTCCINFO` 的广和通 MTK 行表**已完成**，见 §19）；
`/api/lock` 的其它厂商方言（MTK/展锐/巴龙/Telit/Thales/Foxconn/Neoway/openluat）；
`/api/modules` 的 `?model=`/`?modem_id=`；
拨号域遗留：`host_ips` 需 getifaddrs/netlink（当前 operstate 近似）、`dial` 流程里
`set_apn`/`set_pdp` 各自回读 `params()` 共三次往返可合并、厂商 usbnet 方言
（GTUSBMODE 等）未移植、`/api/vendor/cmds` 与 `probedCmds` 的差异为既有差距。

---

## 11. LuCI 界面：退役 Lua 世代，只留 JS + ucode（2026-10-18）

### 11.1 为什么退役

真机（Fibocom FM350-GL）上的实证推翻了「JS 视图已生效」的旧判断：

- 设备 `/tmp/luci-indexcache.*.json` 里 Lua 控制器的 glob 明确包含
  `/usr/lib/lua/luci/controller/*.lua`（经 `process_lua_controller` 处理）；
- Lua 控制器在菜单树**后处理**，会覆盖 menu.d JSON 里的 view 动作；
- 带 `Accept-Language: zh-CN` 请求 `/cgi-bin/luci/admin/modemd/overview` 回的是
  中文文案 + `<h2 name="content">` —— 服务端渲染，**真正渲染的是 Lua 模板**，
  JS 视图一直处于休眠。

两个世代并存还带来重复维护（两套 i18n、两套菜单注册）。按用户决定：删
`luci/app/lua/**`，只保留 JS 视图 + ucode 控制器 + menu.d + acl.d。

### 11.2 删除清单

- `luci/app/lua/controller/modemd.lua`（控制器 + 动作端点）
- `luci/app/lua/modemd_i18n.lua`（Lua 世代翻译表）
- `luci/app/lua/view/modemd/overview.htm`、`service.htm`（服务端模板）

Makefile 的「LuCI（Lua 世代）」安装块一并删除；`postrm` 增加清理，让 opkg
升级上来的机器把设备上的 Lua 残留删掉（手动部署的机器同样受益）。

### 11.3 JS 世代补 i18n（必修，否则界面从中文退回英文）

每个视图**内联一份** `modemdLang()` + `I18N` 表 + `T()`：

- 语言取 `document.documentElement.lang`（dispatcher 写入的解析后语言）→
  `navigator.language` → `en`；查不到翻译回落英文原文（源码即英文，与 Lua 世代口径一致）；
- 表只收 zh-cn / zh-tw 两份，其余语言一律回落英文；
- **刻意不做共享模块**：固件自带应用（`h5000m/netmode.js` 等）也都是单文件视图，
  省掉 class 加载机制的依赖，坏不了；
- overview 补上 Lua 模板原有的端口说明句与 iframe 直嵌（去掉 `about:blank` +
  脚本赋值，少一次跳转）。

### 11.4 顺带修掉的三个 JS 世代实证缺陷（都是真机跑出来的）

1. **自启态永远显示错**：`rpc.declare` 的 `expect` 是**单键选择器**而不是校验器
   （见 `rpc.js` 的 `handleCallReply`：`ret = ret[expect 的第一个键]`，取不到或
   类型不符就用默认值顶上）。原来写 `expect: {result: []}`，而
   `luci.getInitList` 返回的是**按服务名建键的对象**（`{modemd:{enabled:…}, …}`），
   没有 `result` 键 → 被削成空数组 → `entry` 恒为 `{}` → 永远显示「未设开机自启」。
   **这两个 ubus 方法一律不写 expect，自己按对象解析。**
2. **运行态读了个不存在的字段**：`luci.getInitList` 只给 `enabled`，没有 `running`；
   真正的进程状态在 procd 的 `service list` 里（`{modemd:{instances:{…:{running}}}}`），
   ACL 已授权 `service:list`，改为从那里取；ubus 调不通时按「未运行」显示，不崩页面。
3. **ucode 控制器忘了注册节点**：ucode dispatcher 不像 Lua 控制器那样自动扫
   `controller/`，`type: function` 动作必须显式 `require(module)`，所以端点要在
   menu.d 里注册才能分发（照 `luci-base.json` 的 `admin/translations/*` 范式）。
   另外 ucode **没有 `typeof` 运算符**（只有 `type()`），写错会在 `require` 时
   Syntax error 整个 500。

### 11.5 已知限制（有意维持）

- **菜单标题是英文**：`load_catalog` 在 `parser.so`（C 模块）里，只认 `.lmo` 二进制，
  构建期需要 `po2lmo`（设备与单包 Makefile 都没有）；且注册期语言解析为 en，
  菜单标题本来就是英文，无回归。界面内文案由 JS 的 `T()` 翻，不受影响。
- **内嵌 modemd UI 需要单独登录**：iframe 指向 `http://<router>:9527/`，而
  `require_luci_login` 开启时 modemd 的 HTTP 闸门只认自己的 `modemd_sess`
  cookie（LuCI 的 `sysauth_http` 带 `path=/cgi-bin/luci/` + SameSite=strict +
  HttpOnly，浏览器不会带到 9527）。表现是内嵌界面先弹一次 modemd 自己的登录框，
  功能不受影响 —— 与 Lua 世代完全一致。
- `ucode/controller/modemd_auth.uc`（把 LuCI 会话 ID 交给嵌在 LuCI 里的小窗）
  已注册可用，但**当前无人调用**：modemd 的 `index.html` 不读 `?luci_sid=`
  （只有 `mini.html` 读），要用它得先改 modemd 侧。留待屏端嵌入时再接。

### 11.6 实机验证（2026-10-18，逐条跑过）

- 索引树动作类型从 Lua 控制器变成 `view`，`luci.controller.modemd` 痕迹为 0，
  页面尾部明确输出 `<!-- Lua compatibility mode active: no -->`；
- 浏览器（`<html lang="zh-cn">`）打开概览页：标题、说明句、端口、iframe、
  「在新窗口打开」全部中文且正确；服务页按钮/间隔/端口全中文，无 JS 报错；
- 服务页状态与真机一致：`运行中`（procd `service list` 的 `running:true`，
  pid 6591）、`未设开机自启`（`/etc/rc.d/` 无链接，`getInitList` 的
  `enabled:false`）—— 手工部署没跑过 `enable`，显示是准确的；
- auth 端点：带会话回 32 位 sid，未登录回 403（带 `X-LuCI-Login-Required: yes`），
  旧的 typo 路径 `modd_auth` 与旧顶层路径均 404。

---

## 15. 广和通 FM350 / RW350 系列对齐（P1 八项，2026-10-18）

> 参考实现：[LianXia233/luci-app-fm350](https://github.com/LianXia233/luci-app-fm350)。
> 实机：FM350-GL，固件 `<固件版本>`，AT 口 `/dev/ttyUSB3`，
> IMEI 已脱敏（全仓库统一用示例值 `867698040012349`），中国电信。
> **范围**：P1 八项全做；P0 项（频段锁定写已完成；IMEI 的 MFG 块备份按替代方案完成，
> 见 §20；CFUN 自恢复等）留待后续。
> **不变式**：四家拨号器与解析分支互不顶替，改广和通分支不得破坏其它厂商。

### 15.1 八项落地明细

| # | 项 | 落点 | 状态 |
| --- | --- | --- | --- |
| P1-1 | 频段反解 | `model/band.rs`（新增）、`model/network.rs`、`model/cells.rs` | ✅ 实机 |
| P1-2 | 温度 | `model/siminfo.rs`、`model/modem.rs`（`is_fibocom_mtk` 提升为 `pub(crate)`） | ✅ 实机（23 路） |
| P1-3 | IMEI | `model/imei.rs` | ✅ 实机 |
| P1-4 | 短信 PDU 发送 | `model/sms.rs` | ✅ 编译+单测；实机未验（短信存储空） |
| P1-5 | URC 过滤层 | `at/urc.rs`（新增）、`at/port.rs`、`at/command.rs`、`at/mod.rs`、`model/modem.rs` | ✅ 实机（应答完整保留） |
| P1-6 | PDP 判据 | `at/parser.rs`、`at/mod.rs`、`model/params.rs`、`model/dial.rs` | ✅ 单测；实机部分 |
| P1-7 | IPv6 编解码 | `model/addr.rs`（新增）、`model/dial.rs`、`model/mod.rs` | ✅ 单测；待四家 v6 样本回归 |
| P1-8 | EMBIND 体检 | `model/bind.rs`（新增）、`model/modem.rs`、`at/presets.rs` | ✅ 单测 + preset 实机 |

### 15.2 广和通 MTK 平台 AT 口径（真机验证，**不得回退**）

| 用途 | 指令 | 应答形态 | 坑 |
| --- | --- | --- | --- |
| 各路温度 | `AT+GTSENRDTEMP=0` | `+GTSENRDTEMP: <e1>,<e2>,<e3>`，量纲 **0.001 ℃** | 读形式 `AT+GTEMP?` 在该平台回 `phone failure`，**不要用** |
| 写 IMEI | `AT+EGMREXT=1,7,<imei>` | 需冷启动 `AT+CFUN=1,1` 生效 | 手册未收录，见 §15.4 取证说明 |
| 读 IMEI（同族只读） | `AT+EGMREXT=0,7` | `+EGMREXT: "<imei>"`（空模组回 `""`） | **已实机验证**（§20）；`AT+EGMREXT?` 回 `+CME ERROR: 100`，查询写法不可用 |
| 读 IMEI（3GPP 通用） | `AT+CGSN` / `AT+GSN` | 15 位数字 | 与 EGMREXT 读值一致；解析不到时用它兜底 |
| 写保护查询 | `AT+GTIMSILOCK?` | — | 未验证，不自动发 |
| EMBIND 读 | `AT+EMBIND?` | — | 未在实机打到（preset 已收录） |
| EMBIND 能力 | `AT+EMBIND=?` | — | 同上 |
| EMBIND 写 | `AT+EMBIND=1,"<L2P>",<cid>` / `=0,...` | L2P ∈ `M-CCMNI`/`M-RNDIS`/`M-MBIM`/`M-LHIF` | 本轮**只读**，不做自动改绑 |
| 注册态 | `AT+CEREG?` | `+CEREG: <n>,<stat>` | `AT+CREG?` 恒 `0,0`，**必须用 CEREG** |
| PDP 状态 | `AT+CGACT?` | **只回 `OK`，不列 `+CGACT:` 行** | 空转会误判「已去激活」，见 P1-6 |
| 短信发送 | `AT+CMGF=0` → `AT+CMGS=<len>` → `>` → PDU + Ctrl-Z | 发完切回 `AT+CMGF=1` | TEXT 模式中文/长短信不可靠 |

### 15.3 关键设计决策与「为什么不」

- **P1-1 频段反解**：修掉既存 bug —— `network.rs::band_label` 注释写 B3、实现给 `n3`
  （LTE 频段被按 NR 前缀输出）。FM350 在 NR 制式下 `+GTCCINFO` 的 `<band>` 实测为空，
  只能靠 ARFCN 反推；解不出给 `null`，不猜。
- **P1-3 IMEI**：Luhn 校验**仅作非阻断提示**（`luhn_ok: Option<bool>`）——
  部分厂商 IMEI 不严格遵循 Luhn，阻断会把合法值挡在门外。
- **P1-4 短信**：整条序列换 PDU，不是在文本序列上打补丁。切分按**真实 septet /
  UTF-16 容量**算（GSM7 140 字节 / UCS2 140 字节），不是按字符数拍脑袋。
  分段失败报「第 N 条被拒绝（已成功 M/T 条）」。
- **P1-5 URC**：只吞**明确是主动上报**的行。三条红线：
  ① 前缀逐条列，参考实现用 `+GT` 宽匹配把 `AT+GTDNS` 的查询响应一起吞掉、
  巡检 DNS 恒为空 —— 本程序查询过的前缀一律不进表；
  ② `+CPIN` 刻意不吞（与 `AT+CPIN?` 应答同形）；
  ③ 洪水阈值**只对 URC 行计数** —— `AT+COPS=?` 扫网能一次回上百行，
  一起限流会把查询结果截断，那比 URC 污染严重得多。
- **P1-6 PDP**：`CFUN=4` 飞行模式自恢复**不做** —— 同步 HTTP 会挂住请求线程。
  `DialReadiness` 只在**失败路径**调用，成功路径零成本。
  `cgact_state` 三态是为了把「没列明细」与「已确认去激活」分开 ——
  FM350 上 `AT+CGACT?` 只回 `OK`，空转会误判。
- **P1-7 IPv6**：`normalize_ipv6` 必须拒 FM350 的 `0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1`
  （16 段点分的 `::1` 占位）—— 不拦会配出一条永远不通的 v6 默认路由。
  **刻意与参考实现不同**：16 段点分分支保留「前 8 字节全 0 也不显示」的更严判据 ——
  那是 FM350-GL 只报接口标识的实机行为（`0.0.0.0.0.0.0.0.24.217.217.24.12.29.50.199`），
  按项目惯例不显示不可信地址；且该判据同时覆盖全零与 `::1` 两种占位。
  `dial.rs` 原有的 `parse_ipv6` / `looks_like_ipv6` / `format_ipv6` 三处重复实现
  下沉到 `addr.rs`（编解码只留一份）。
- **P1-8 EMBIND**：只做只读体检 + preset，**不做** `rebind_to_host()` 自动改绑 ——
  改绑会改变数据面走向，错了整机断网，风险与收益不成比例。

### 15.4 取证程度（P1-3 破例收录的说明）

`AT+EGMREXT=1,7,<imei>` **未被广和通手册（v2.2 / v2.10）收录**，属社区与实机验证命令。
破例理由：本程序已有三重保护 —— ① 15 位数字校验；② 改写前用**同族只读命令**
`AT+EGMREXT=0,7` 快照备份到 `imei.json`（该读形式已实机验证，见 §20）；
③ 下发后回读确认，不一致时按平台分流（广和通 MTK 需重启模组才生效，只提示不误判）。
代码注释与 `API.md` 均已写明取证程度。
相关但**未验证、不发**的：`AT+GTSNWR=1,<imei>`（另一套写法）、`AT+GTIMSILOCK?`（写保护查询）。

### 15.5 实机验证记录（2026-10-18，FM350-GL）

部署：交叉编译 aarch64-musl → `/usr/bin/modemd` → `/etc/init.d/modemd restart`
（pid 25382，`9527` 监听，启动日志无 ERROR/WARN）。

| 检查 | 结果 |
| --- | --- |
| 核心端点无回归 | ✅ `/api/modems` `/api/modem/info` `/api/sim` `/api/network` `/api/lock/status` `/api/lock/caps` `/api/params` `/api/usbnet` `/api/dial` 与 P1 前基线一致 |
| P1-2 温度 | ✅ 23 路传感器解出，命名正确（未列编号退化为 `sensorN`），`raw=0` 通道跳过 |
| P1-3 IMEI | ✅ `cmd:"AT+EGMREXT=1,7,"`、`supported:true`、`luhn_ok:true`；同族只读形式 `AT+EGMREXT=0,7` 的取证见 §20 |
| P1-8 preset | ✅ `AT+GTSENRDTEMP=0` / `AT+EMBIND?` / `AT+EMBIND=?` 三条在位（广和通共 18 条） |
| P1-5 URC | ✅ `AT+CEREG?` 回 `+CEREG: 0,1` + `OK` —— **双字段应答完整保留，没被吞**；`+CPIN: READY` 正确放行；`AT+CGACT?` 只回 `OK`（符合该平台实况） |
| P1-1 cells | ⚠️ ~~`/api/cells` 回 0 条 —— 该机型 `AT+GTCCINFO?` 不出数据~~ **结论有误，已修正**：指令有数据，是解析器不认无标签行表；见 §19 |

### 15.6 遗留与后续

- P1-4 短信 PDU 实机未验证（模组短信存储为空）——换一张有短信的卡补测。
- P1-7 需四家 v6 样本回归（移远 / 美格 / 芯讯通 / 展锐上报的都是标准冒号形式）。
- ~~P1-5 的**消费端未接线**：`Modem::take_urc_lines()` / `Modem::urc_dropped()` 已就绪，
  事件化（驱动巡检加速、状态推进）待后续~~ **已完成**（见 §21：新增 `spawn_urc` 循环
  与 `model/urc.rs` 折算层，`+GTSIM` / 注册态 URC 现在会把巡检加速到下一拍）。
  ~~屏端异步推送（`+CUSD` / `+CMTI`）~~ **亦已完成**（见 §22）。
- P0 项：频段锁定写、IMEI 的 MFG 块备份、`CFUN=4` 自恢复。
  ~~IPv6 归一化接进 `dial.rs` 流程~~ **已完成**（见 §16）。
  ~~频段锁定写~~ **已完成**（见 §17）。
  ~~IMEI 的 MFG 块备份~~ **原设想无命令面，已按替代方案「写前同族快照 + 本地备份
  记录」完成**（见 §20）。

---

## 16. 广和通 FM350 / RW350 系列对齐（P0-1：IPv6 归一化接进网络链路，2026-10-18）

> **范围**：P0 四项里的第一项。P1 八项见 §15。
> **不变式**：四家拨号器与解析分支互不顶替，改广和通分支不得破坏其它厂商。

### 16.1 动机：实机抓到的两个实证缺陷

`AT+CGCONTRDP` 那条路（`network.rs::split_addr` → `NetworkStatus.ipv4/ipv6`
→ `sync_dial_state` → `DialState` + 波动记录）原先**完全没接 `addr` 归一化**：

1. **地址字段下标写死**：代码读 `f.get(3)`（3GPP 规定 `<ip_addr>` 在第 3 位），
   但实测广和通 MTK 平台把它放到了**第 5 位**（`f[3]` / `f[4]` 是空串）。
   后果：`/api/network/status` 的 `ipv4` / `ipv6` 在该机型上**从未上报过**。
2. **点分 16 段被当成 IPv4**：`32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.1` 含 `.`，
   按 `contains('.')` 分类 → 显示成一个假 IPv4。IPv6 未分配时的
   `0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1` 占位同样中招。

### 16.2 改动

| 文件 | 改动 |
| --- | --- |
| `modem/model/network.rs` | 新增 `parse_active_context()`（纯函数，单包测试落点）；地址字段从第 3 位起扫；按 cid 归并多行；`split_addr` 接入 `addr` 归一化 |
| `modem/model/addr.rs` | `decode_dotted_ipv6` 收紧为统一严格判据；`format_ipv6_bytes` 改私有；补 3 个测试 |
| `modem/model/dial.rs` | 删 `format_ipv6` / `bytes_from_decimal` / `has_zero_prefix` 三处重复实现，点分分支改走 `addr::normalize_ipv6` |

### 16.3 三个设计决策

1. **地址字段不写死下标，改为「从第 3 位扫第一个含地址的字段」**。再往后是 DNS
   （也是合法 IPv4），所以扫到第一个就停。但**紧邻的下一个字段也要看一眼**——
   实测两种固件把 v4 / v6 拆开报：广和通 MTK 平台是同一 cid 拆两行，
   移远等是同一行里相邻两列。
2. **按 cid 归并**。广和通 MTK 平台把同一个上下文的 IPv4 与 IPv6 拆成两行
   （cid 相同）。只取首行就永远看不到 IPv6；归并后两行都取到。
   不同 cid 的上下文仍然分开，不会把 cid 2 的地址混进 cid 1。
3. **纯 IPv6 上下文不再被跳过**：选中判据从「有 APN 或 v4」改为「有任一数据」。
   原先 v6-only 上下文（无 APN、无 v4）会被整条跳过，网络卡片上「本机地址」永远空着。

### 16.4 与参考实现的偏差（刻意）

`addr::decode_dotted_ipv6` 除全零与 `::1` 外，还拦「前 8 字节全 0」（只上报接口标识、
没有前缀）——实测 FM350-GL 就这么回。参考实现不拦这条。两条通路
（`AT+CGPADDR` 的 `dial::split_addrs` 与 `AT+CGCONTRDP` 的 `network::split_addr`）
现在共用同一套判据，同一份固件输出不会解出两个样。

### 16.5 实机验证记录

FM350-GL，中国电信，`AT+CGCONTRDP` 原始应答（同一 cid 两行，地址在第 5 字段）：

```text
+CGCONTRDP: 0,5,"ctnet.MNC011.MCC460.GPRS","","","203.0.113.53","203.0.113.54","","",0,,0,...
+CGCONTRDP: 0,5,"ctnet.MNC011.MCC460.GPRS","","","32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.1",...
```

`GET /api/network/status`：修复前**无** `ipv4`/`ipv6` 字段；修复后

```json
"apn":"ctnet.MNC011.MCC460.GPRS","ipv4":"203.0.113.53","ipv6":"2001:db8::1"
```

（`2001:db8::1` 由 `32.1.13.184.0.0.0.0.0.0.0.0.0.0.0.1` 解出）。
日志无 ERROR/WARN。测试数 333 → 348，`cargo build` 零告警。

### 16.6 遗留

- `sync_dial_state` 只在「拨号中/已连接」时同步地址，未拨号时 `DialState` 不更新
  ——实机 `state=idle`，故本次只验到 `/api/network/status`。
- `ipv6` 进波动记录后，「地址消失（掉线）」也算一次变化，这是 §7 既有设计
  （`changes::not_reported`）；首次从「未上报」变为真实地址会产生**一条**记录。

---

## 17. 广和通 FM350 / RW350 系列对齐（P0-2：频段锁定写，2026-10-18）

> **范围**：P0 四项里的第二项。P0-1 见 §16，P1 八项见 §15。
> **不变式**：四家拨号器与解析分支互不顶替，改广和通分支不得破坏其它厂商。

### 17.1 动机：实机抓到的方言缺口

`lock.rs` 原先只有两套方言（`qnw` / `simcom`），FM350 一套都不认：

- 探测顺序是「按厂商猜 + 挨个试」，广和通 → 猜 QNW → `AT+QNWPREFCFG="mode_pref"`
  实机回 `ERROR` → 试芯讯通 `AT+CSYSSEL="lte_band"` 也回 `ERROR` → 回落成
  「未探测到，按厂商猜测实现，命令可能不被接受」，`dialect` 字段为空。
- 后果：`band_lte` / `band_nr` / `mode_pref` 全是空值，而**写路径仍会拿 QNW 的
  指令去发**——必然失败。

实机确认该平台用的是 `AT+GTACT`（`AT+GTACT?` 一次命中）。

### 17.2 实机原始报文（夹具来源）

```text
AT+GTACT?   → +GTACT: 20,6,3,1,2,4,5,8,101,102,…,171,501,502,…,5079
AT+GTACT=?  → +GTACT: (1,2,4,10,14,16,17,20),(2,3,6),(2,3,6),(),(1,2,4,5,8),
                     (101,102,…,171),(),(),(501,502,…,5079)
AT+EMMCHLCK? → +EMMCHLCK: 0
AT+EMMCHLCK=? → +EMMCHLCK: (0-1),(0,2,7),(0,1),(0-46589),(0-511)
```

`AT+GTACT?` 的字段布局（对照 `=?` 的 9 个参数组可完全对上）：

```text
+GTACT: 20,6,3,1,2,4,5,8,101,…,171,501,…,5079
         │  │  │ └──┬──┘ └──┬──┘ └───┬───┘
         │  │  │   GSM 组   LTE 组    NR 组
         │  └──┴─ 两个优先级控制字段（实机值 6 / 3）
         └─ <rat>：1=UMTS 2=LTE 4=LTE/UMTS 10=Automatic 14=NR
                  16=NR/WCDMA 17=NR/LTE 20=NR/WCDMA/LTE（三模全开）
```

### 17.3 三个设计决策

1. **频段按值域分类，不按下标**。`AT+GTACT=?` 有 9 个参数组，其中 3 组是空集，
   在回显里**不占位**，逐组数下标必然错位。因此前三个字段按位置取
   （`<rat>` + 两个控制字段），其余按值域分类（手册 §11.1.14）：
   UMTS `1..=10`、LTE `101..=199`（`100+n`）、NR `>=500`（`"50"`+band 拼接，
   5078 = n78、5020 = n20）。控制字段的值（2/3/6）落在 GSM 值域，但它们
   **在频段之前**，所以不冲突。认不出值域的值进 `other` 原样保留。
2. **必须读改写**。一条 `AT+GTACT` 同时写三组，改一组就得把另外两组原样带回去。
   `GtactRead` 因此把 `rat` / `ctrl` / `gsm` / `other` 都按原样留着——它们没有
   UI 入口，但改 LTE / NR 频段时必须保留。
3. **解锁写回支持全集，不写 0**。手册 §11.1.14 记 `<band>=0` 为
   "automatic band selection"，但一条命令混带多组频段时 0 作用于哪些组没有实测；
   解锁走**已实机验证**的写法——写回 `AT+GTACT=?` 的支持全集（等效「不限制」：
   模组本来也用不了支持集之外的频段）。读不到支持全集就拒绝执行。锁定时也用它
   校验（实测 B99 被拒并列出支持全集）。

另两条硬约束：锁频前先离线（`CFUN=0` → 下发 → `CFUN=1`，首尾 best-effort、
中间失败必须冒泡且拉回在线）；`lock/caps` 的探测项按方言生成（广和通只列
`band_lte` / `policy_band` / `mode_pref` / `cell_lte`，`AT+GTACT?` 不重复列三遍）。

### 17.4 新增/改动的纯函数（单包测试落点）

| 函数 | 作用 |
| --- | --- |
| `parse_gtact_read` | 解 `AT+GTACT?` → `GtactRead{rat, ctrl, gsm, other, lte, nr}`（lte/nr 已解码成 band 号；认不出值域的值进 `other` 原样保留） |
| `parse_gtact_supported` | 解 `AT+GTACT=?` → (LTE 支持集, NR 支持集)，按值域认组 |
| `gtact_raw_line` | 取 `+GTACT:` 行的值部分 |
| `gtact_lte_code` / `gtact_nr_code` / `gtact_nr_decode` | band 号 ↔ `AT+GTACT` 编码 |
| `gtact_family` | 值域 → 技术域 |
| `gtact_rat_label` | `<rat>` 人可读名（`mode_pref` 的值） |
| `parse_emmchlck` | 解 `AT+EMMCHLCK?` → `CellLock` |

### 17.5 实机验证记录

| 操作 | 结果 |
| --- | --- |
| `GET /api/lock/caps` | `dialect="fibocom"`，`dialect_tried` 只有一条（`AT+GTACT?` 接受） |
| `GET /api/lock/status` | `mode_pref="NR/WCDMA/LTE"`，LTE 31 个、NR 19 个，均未锁定 |
| 锁 LTE → B3 | `code=0`，回读 `bands=[3]` |
| 锁 LTE → B1+B3 | `code=0`，覆盖成功 |
| 锁 LTE → B99（不支持） | `code=-1`，列出支持全集后拒绝 |
| 解锁 LTE | `code=0`，回到 31 个 |
| **锁 NR → n78** | `code=0`，**`band_lte` 31 个原样不动**（读改写生效） |
| 解锁 NR | `code=0`，回到 19 个 |
| 锁小区 | `code=-1`，明确拒绝并说明原因 |

验证结束后 `AT+GTACT?` 与操作前**逐字节一致**，配置完全还原。

### 17.6 踩到的坑（留给后续）

- `AT+CFUN=1,1`（整机重启）会把 **PDP 上下文定义清掉**（易失存储）：
  重启后 `AT+CGDCONT?` 为空、`AT+CGCONTRDP` 没有任何 `+CGCONTRDP:` 行，
  `/api/network/status` 的 `ipv4`/`ipv6` 随之消失。这是**模组状态**不是代码问题
  （没有激活上下文时 `parse_active_context` 本就该返回三空）。
  恢复办法：`AT+CGDCONT=1,"IPV4V6","ctnet"` → `AT+CGACT=1,1`，
  实机随即回 `203.0.113.53` / `2001:db8::1`，与操作前一致。
- 重启还会让 USB 重新枚举，AT 口在 `/dev/ttyUSB3` 与 `/dev/ttyUSB4` 之间跳，
  旧串口句柄报 `Broken pipe`，需要重启 modemd 重新发现端口。
- 因此 `POST /api/modem/reset` 与「锁频前 `CFUN=0/1`」是**不同量级**的操作：
  前者会清 PDP 定义，后者不会（实测 6 次 `CFUN=0/1` 后 PDN 仍在）。

---

## 18. 修复：后台运营商不显示（`/api/network/status`，2026-10-18）

> **范围**：用户报「后台运营商不显示」。定位到两个叠加原因，都只动
> `modem/model/network.rs`，前端不用改（前端 `st.plmn` / `st.tech_label` 的消费点
> 本来就在，只是后端没给值）。
> **不变式**：只补空缺，不造数据；`net_tech` 保持 QNWINFO-only。

### 18.1 根因

1. **直接原因**：`network_status()` 里 `operator: None` 是**硬编码**的——这个字段
   对所有厂商从未被赋值，页面上的运营商名自然永远是空的。
2. **次要原因**：该机型 `AT+QNWINFO` 回 `ERROR`（`AT+CSPPLMN?` / `AT+CNMP?` 同样），
   于是 `tech` / `mcc` / `mnc` / `plmn` 一起空；而简版 `/api/network` 走
   `AT+COPS?`（3GPP 通用）一直是有值的——同一个模组，两条路给出完全不同的数据。

实机证据：

```text
AT+QNWINFO          → ERROR
AT+COPS?            → +COPS: 0,2,"46011",11      （format=2：只有 PLMN，无上报名）
AT+CREG?            → 0,0                        （必须用 AT+CEREG?，见 §16）
AT+CGATT?           → +CGATT: 1
```

### 18.2 方案：`AT+COPS?` 兜底 + operator 三级取值

新增三个纯函数（都不接串口，可全覆盖单测）：

| 函数 | 职责 |
| --- | --- |
| `merge_network(&QnwInfo, Option<&NetSelect>)` | QNWINFO 优先，COPS 只补空缺；返回 `NetFields` |
| `operator_display(mcc, mnc, module_name, plmn)` | 三级取值：内置表 → 模组上报名 → 原始 PLMN |
| `cops_act_tech(i32)` | `<AcT>` → `tech` 归一值 |

关键取舍：

- **只在 `info.plmn.is_none()` 时才多发 `AT+COPS?`**。QNWINFO 可用的厂商
  （移远 / 美格 / 芯讯通）不增加这一次往返；FM350 这类才多发。
- **兜底只补 6 个字段**：`plmn` / `mcc` / `mnc` / `tech` / `tech_label` / `operator`。
  `band` / `band_text` / `earfcn` / `net_tech` 没有通用替代（`AT+GTCCINFO?` 在该
  机型不出数据），**保持为空并写进文档，不造数据**。
- **`net_tech` 保持 QNWINFO-only**：它是「模组原始上报串」，兜底路径会凭空造一个
  字符串出来，与「不造数据」冲突。
- **`<AcT>` 映射按 3GPP TS 27.007**（`7`=LTE、`11`=NG-RAN、`2/4/5/6`=WCDMA、
  `0/1/3`=GSM）。⚠️ **FM350 AT 手册 v2.2 §11.1.6 那张表早于 NG-RAN 入表**（它把
  11 写成 eMTC、12 写成 NB-IoT），照它写会把 NR 报成 IoT 制式。实机注册在 NR n78
  时 `<AcT>` 就是 `11`。**已知限制**：NR NSA 下模组常以 LTE 注册、`<AcT>` 报 `7`，
  字段照报、不猜。
- `operator_display` 的第一级是 `cells::operator_name(mcc, mnc)`（四家中国运营商
  全名表，单一事实源）；实机 `46011` 直接命中**中国电信**，不需要上报名。

### 18.3 FM350 AT 手册 v2.2 全文的其它收获（本轮回合补读）

手册（237 页英文版，`fibocom_fm350_at_commands_v2.2.pdf`）已全文提取并 grep 核对：

| 命令 | 结论 |
| --- | --- |
| `AT+COPS` §11.1.6 | AcT 表**过时**（见 §18.2），别照抄 |
| `AT+GTACT` §11.1.14 | UMTS 频段 `1..=10`（代码原为 `1..=8`，B9/B10 会在读改写时被丢）；`PreferredAct1/2` = 2(WCDMA)/3(LTE)/6(NR) 优先；`<band>=0` 文档写 automatic；Note 6「设 10 读回 20」 |
| `AT+EPBSEH` §11.1.12 | **位图**频段命令（`0xffff…` = 全支持，GSM 不支持），不是列表 |
| `AT+EPRATL` §11.1.13 | 设优先 RAT 列表（2/4/128） |
| `AT+CFSN` §3.16 | 只读工厂序列号 |
| **MFG / SFI / NV** | **全文 0 命中**；IMEI 只有 `AT+CGSN` / `AT+GSN` 只读，`AT+EGMR*` 系列不在本手册 |

→ **P0-3（IMEI 的 MFG 块备份）原设想的「SFI 读写备份 MFG 块」没有命令面可走**，
已按替代方案「**写前同族快照 + 本地备份记录**」落地：同族只读命令
`AT+EGMREXT=0,7` 已实机验证（§20），备份与恢复链路见 §5.11 / §15.4。

---

## 19. 修复：SINR 恒为空 + 基站扫描 0 条（`/api/signal`、`/api/cells`，2026-10-18）

> **范围**：用户报「SINR 也无数据」。根因同时打挂了 `/api/cells`，一起修。
> **不变式**：只动广和通 MTK 分支与索引换算口径，其它厂商路径不改。

### 19.1 根因：`AT+GTCCINFO?` 的行表从来没人解析过

该平台的服务小区指令不是移远 / 广和通高通那种带标签形式，而是**无标签行表**：

```
AT+GTCCINFO?
+GTCCINFO: 
1,4,460,1,7B10,006414DEE,3740,119,,,-5,23,23,9      ← 服务小区（首列 1）
2,4,,,FFFF,00FFFFFFF,3740,172,,22,22,6              ← 邻区（首列 2）
OK
```

既有解析器只认 `+QENG: "servingcell",…` 这种**带引号标签**的形态（`cell_line_parts`
找不到 `"` 就整行丢），于是行表被全部丢弃 → `/api/cells` 恒 0 条；而 LTE 制式下
`AT+CESQ` 的 9 格里**没有 SINR**（第 7~9 格是 NR SSB 扩展，LTE 分支用不到），
`nr_sinr` 跟着恒空。§15.5 里「该机型 `AT+GTCCINFO?` 不出数据」的结论是错的：
指令一直有数据，是没人认。

### 19.2 手册 §11.1.15 的列序（按 `<rat>` + 角色钉死，不是猜的）

| 行 | 列序（`<role>,<rat>,<mcc>,<mnc>,<tac|lac>,<cellid>,<arfcn>,<pci>` 之后） |
| --- | --- |
| LTE 服务小区 | `<band>,<bandwidth>,<rssnr_value>,<rxlev>,<rsrp>,<rsrq>` |
| LTE 邻区 | `<bandwidth>,<rxlev>,<rsrp>,<rsrq>`（无 `<band>`、无 `<rssnr>`） |
| NR 服务小区 | `<band>,<bandwidth>,<ss_sinr>,<rxlev>,<ss_rsrp>,<ss_rsrq>` |
| NR 邻区 | `<ss_sinr>,<rxlev>,<ss_rsrp>,<ss_rsrq>`（无 `<band>`、无 `<bandwidth>`） |
| WCDMA 服务小区 | `<band>,<ecno>,<rscp>,<rac>,<rxlev>,<reserved>,<Ec/Io>` |

取值定义（手册 Defined Values）：`<rssnr_value>` / `<ss_sinr>` **本身就是 dB**
（-100 ~ 100，255 无效）；`<rsrp>` / `<rsrq>` 是**阶梯索引**（与 `AT+CESQ` 同表）；
`<rxlev>` 在三种制式下都与 `<rsrp>` 同表（不重复取）；`<band>` / `<bandwidth>`
是编码值，交 `band` 模块反解。

### 19.3 改动

- **`cells::parse_gtccinfo_rows`**（新增纯函数）：按上表逐行定位，服务 / 邻区
  分流，`<band>` 为空按频点反推（NR 只有这一条路）。邻区 PLMN 常为空，沿用既有的
  `fill_neighbour_plmn` / `mark_intra_frequency` 补齐。
- **`scan_cells`**：广和通 MTK 分支改走 `AT+GTCCINFO?`，不再白发 4 条 QENG / FENG
  （每条都是一个完整超时）。
- **`signal.rs::query_serving_signal`**：广和通 MTK 跳过 `AT+QENG`；SINR 取
  GTCCINFO 服务小区行，RSRP / RSRQ 仍以 `AT+CESQ` 为准、GTCCINFO 兜底。
- **修正 `AT+CESQ` 的 LTE 索引换算（原来差一整档）**：`idx - 140` / `(idx - 39) / 2`
  取的是区间上界，而 3GPP TS 27.007 §8.69、手册 §11.1.15、同一函数里的 NR SSB
  分支和参考实现都是**下界**（索引 1 = [-140,-139) dBm）。统一为 `idx - 141` /
  `(idx - 40) / 2`（半步向零截断）。索引换算抽成 5 个纯函数，两处指令共用。

### 19.4 实机验证记录（FM350-GL）

| 检查 | 结果 |
| --- | --- |
| `/api/signal` | ✅ `nr_sinr:-3`（此前只有 RSRP / RSRQ）；`nr_rsrp:-116`、`nr_rsrq:-14` |
| `/api/cells` | ✅ 0 条 → 1 服务 + 2 邻区；服务小区 `B8` / PCI 119 / TAC 31504 / SINR -3；同频邻区正确标记 `intra_cell` |
| 索引口径自洽 | ✅ GTCCINFO 行内 `<rsrp>` 索引 24 → -117 dBm、`<rsrq>` 索引 10 → -15 dB，与 CESQ 同时刻读数（-116 / -14）一致到 1 dB 内的抖动 |
| 测试 | ✅ 364 → 372，`cargo build` 零告警 |

### 19.5 已知限制（照报不猜）

- 该机型 GTCCINFO 的 `<mnc>` 实测报 `1`（`AT+COPS?` 报 `11`；参考实现的机型报 `0`），
  内置 MCC/MNC 表命不中，**基站表不出运营商名**——网络状态页的运营商走 COPS 那条路，不受影响。
- WCDMA 行只取身份与 `<band>`：`<ecno>` / `<rscp>` / `<Ec/Io>` 与本结构的信号格
  口径不同，不硬套。
- 索引 `0` 按手册当作「低于可测下限」接收（`< -140 dBm` → -141），`255` 一律未上报。

---

## 20. 加固：IMEI 改写链路（P0-3 替代方案，2026-10-18）

> **范围**：P0-3「IMEI 的 MFG 块备份」的原设想（SFI 读写把 MFG 块整块备份出来）
> **没有命令面**——广和通手册全文 0 命中 MFG / SFI / NV。改用替代方案
> 「**写前同族快照 + 本地备份记录**」，并把参考实现（luci-app-fm350）在 IMEI
> 上踩过的两个坑一并收口。
> **不变式**：只动 IMEI 写路径；不放松没有证据的厂商路径；前端调用方式不变。

### 20.1 实机取证：同族只读形式可用，查询形式不可用

探针方法：停服务 → 裸 fd 脚本 → 重启服务。本次 AT 口为 `/dev/ttyUSB1`。

| 命令 | 实机应答 |
| --- | --- |
| `AT+EGMREXT=0,7` | `+EGMREXT: "<imei>"` + `OK` |
| `AT+EGMREXT?` | `+CME ERROR: 100` |
| `AT+CGSN` / `AT+GSN` | 与 EGMREXT 读值一致 |

两个探针坑（后来者会再踩，记在这里）：

- `head` 读串口时 stdio 是**块缓冲**，被 `timeout` SIGTERM 杀掉时已读数据随缓冲
  一起丢掉，表现为「端口完全不应答」；换 `cat`（无用户态缓冲，即读即写）即恢复。
- `pgrep -f /usr/bin/modemd` 会匹配到**含该路径的远程 shell 自身**，等进程退出
  必须用 `pgrep -x modemd`；否则服务一直没停，应答全被它抢读。

### 20.2 三处改动

- **写前快照同族化**（`imei_snapshot`）：广和通 MTK 先发 `AT+EGMREXT=0,7`
  （与写入指令 `AT+EGMREXT=1,7,"<imei>"` 同族同参位，备份的正是即将被覆盖的
  那个值），解析不到再退回 `AT+CGSN`；台账新增 `source` 记原值是哪条命令读到的。
- **空 IMEI 模组从「永久拒绝写入」变为「可写」**（`record_original`）：旧逻辑以
  `original` 非空判定「还没备份过」，于是空 IMEI 模组（**正是最需要改写的维修
  场景**）永远走不到写入命令；且原值为空时，下一次改写会把「原值」覆盖成上一次
  写进去的值，一键恢复从此失效。现以 `saved_at` 判定，空原值照记不覆盖。
  恢复时若原值为空则拒绝——把 IMEI 写成空没有意义。
- **写后回读按平台分流**（`write_imei`）：参考实现 1.0.14-r2 的实机记录表明
  `AT+EGMREXT` 写入**需模组重启后才在射频 / 网络侧生效**，所以广和通 MTK 平台
  回读到空值 / 旧值都可能是「还没生效」而非「写失败」——只把「模组明确报错」
  当失败，其余附重启提示返回成功；其它厂商（高通 `AT+EGMR`）维持原语义
  「回读不一致即报错」，不放松没证据的路径。

### 20.3 契约变化（向后兼容）

- `imei.json` 台账新增 `source` 字段；旧文件没有该字段，按空串读入，不让老台账失效。
- `/api/imei`、`/api/imei/set`、`/api/imei/restore` 响应新增 `warning`
  （`skip_serializing_if`，没有就不出现；仅 set / restore 的当次响应携带）；
  `note` 文案按平台说明真实口径。
- 前端：`toast` 新增警告态（琥珀色、6 秒）；写 / 恢复 IMEI 时把「已回读确认」
  与「已下发、需重启后复查」分开说。

### 20.4 验证与未验证

- 单测 372 → 378（EGMREXT 三态解析、空原值不被覆盖、旧台账兼容），
  `cargo build` 零告警；交叉编译 aarch64-musl 成功。
- **未做实机改写验证**：那会改掉用户设备的真实 IMEI，需用户明确授权后才做。

---

## 21. URC 消费端接线：把暂存的主动上报变成巡检加速（2026-10-18）

> **范围**：P1-5 只做了「过滤与暂存」（`at/urc.rs`），暂存队列一直没有消费端 ——
> `take_urc_lines()` / `urc_dropped()` 是死的。本小节把消费端接上：新增后台循环
> `http/loops.rs::spawn_urc` + 纯函数折算层 `model/urc.rs`。
> **不变式**：过滤层的三条红线不动；不发新 AT；不改任何 JSON 契约；前端无感。

### 21.1 为什么只「置脏」而不「直接改状态」

单字段注册态 URC（`+CEREG: 5`）与 `AT+CEREG?` 在 `n=0` 时的应答**完全同形**，
它的值不足以推定当前状态（过滤层当初就是靠这个同形性才决定「不吞 `+CPIN`」）。
拿 URC 的值去覆盖缓存，等于用不可信数据盖掉可信数据。所以消费端对注册态只产
**脏标记**，权威读数仍走既有查询路径 —— 这也是 `model/urc.rs` 做成纯函数
（无 IO、可单测）的原因。

两个例外交**原文**而不是脏标记：`+CUSD` 与 `+CMTI` 携带的内容**没有对应的
查询路径可以重读**（USSD 应答只在 URC 里来一次；`+CMTI` 只给存储索引，
内容要重读 `AT+CMGL` 才知道）。详见 §22。

### 21.2 四类处置（取证程度逐条写明）

| URC 类别 | 动作 | 依据与代价 |
| --- | --- | --- |
| `+GTSIM:`（`GtSim`） | `mark_sim_dirty()` ⇒ 巡检下一拍（≤2 秒）核实 SIM | 广和通 MTK 的 SIM 状态上报。误判代价仅多读一次 `AT+CPIN?`+ICCID；身份以 ICCID/IMSI 为准且有 30 秒冷却，**不会误动作** |
| `+CEREG:`/`+CGREG:`/`+CREG:` 单字段（`Reg`） | `mark_net_dirty()` ⇒ 推送循环下一拍重查网络 | 注册态变了 ⇒ `/api/network` 缓存过期。重查仍受推送节拍限制（最快 1 秒一次） |
| `+CUSD:`（`Cusd`） | 原文交出去 ⇒ 解析后写缓存 + 推 `ussd` | USSD 应答无从重读，只能照搬；判据见 §22.1 |
| `+CMTI:`（`SmsNew`） | `sms_dirty` ⇒ 重读一次短信 + 推 `sms`/`smscount` | 新短信内容只有重读才知道；节流 10 秒 |
| 其余（`Ver`/`GtStatus`/`GtRgc`/`GtUsbmode`/`GtSrv`/`GtPin`/`GtDss`/`GtCellInfo`） | 无动作，仅日志 | 手册与参考实现均无语义记载，**不猜** |

### 21.3 循环边界（与其它后台循环一脉相承）

- **不自己开串口**：只处理 `is_running()`（AT 口已打开）的模组 —— 没人查询过的模组
  不会被这个循环唤醒；
- **不发 AT**：只 `take_urc_lines()` 取走已暂存的行，与查询共用同一把 AT 锁，
  拿到锁后也只是 `mem::take` 一个 Vec，不添串口时序风险。
  （§22 起多了一个例外：`+CMTI` 触发的一次短信重读，见该节说明）；
- **洪水丢弃是观测项**：`urc_dropped()` 累计值大于 0 时按 30 秒限流告警一次 ——
  主动上报被垃圾行淹没意味着加速会失灵，这是排障第一个该看的数；
- **日志分两级**：每类计数 + 首条原文打 debug；某模组**首次**见到某类打 info
  （排障时想知道「这台模组到底会发什么」就看这几行）。

### 21.4 顺带改准的两处文档

`sim.rs` 与 `Modem::mark_sim_dirty` 的注释原写「读协程看到与已知状态不同的 `+CPIN:` 时调」，
与实际不符：`+CPIN` 与查询应答同形、过滤层刻意不吞，它**不可能**是置脏来源。
已改为「`+GTSIM` 被 `spawn_urc` 消费时调」。注释描述不存在的行为，比没有注释更糟。

### 21.5 验证与未验证

- 单测 378 → 385（`model/urc.rs` 六个：三类分流、空批次、混合批次、按类统计保序；
  `modem.rs` 一个：`take_net_dirty` 取走即清）；`cargo build` 零告警；
  `cargo fmt --check` 本轮三个文件干净（仓库既有 fmt 偏差未动，不在本轮范围）。
- **实机未验证本轮新增行为**：设备上尚未实际观察到 `+GTSIM:` / 注册态 URC 到达
  （首见 info 日志就是为了在实机上收集这个证据）。部署后看日志即可确认。

---

## 22. URC 屏端推送：`+CUSD` / `+CMTI`（2026-10-18）

> **范围**：阶段 5 最后一块体验缺口 —— USSD 应答与新短信此前都要手动刷新页面。
> 把这两类 URC 接进过滤层与消费端。
> **不变式**：`+CPIN` 依旧不吞；查询应答一条都不能被吞；前端调用方式不变。

### 22.1 `+CUSD` 为什么不能进前缀表

它的查询应答与主动上报**同前缀**：

```text
AT+CUSD=?        → +CUSD: (0-2)                  ← 查询应答（取值范围）
AT+CUSD=1,*100#  → +CUSD: 0,"余额 12.5 元",15    ← 主动上报
```

按前缀一刀切吞掉，`presets.rs` 里的「USSD 支持情况」预设项就会永远空应答 ——
与参考实现 `+GT` 宽匹配吞掉 `AT+GTDNS` 响应是同一类事故。改用**形态判据**
（`cusd_is_urc`）：主动上报第一字段永远是数字状态码，查询应答是 `(...)`，
拿「冒号之后是不是 `(`」区分。

踩到的坑（单测当场抓住）：判据最初写的是「首字段是不是 `(`」，而传入的 `rest`
**含冒号**，`trim_start()` 不剥冒号 ⇒ `+CUSD: (0-2)` 被判成 URC 吞掉。
改成 `trim_start_matches(':')` 后才对，并补了 `+CUSD:(0-2)`（冒号后无空格）
这条断言盯着。另有一层兜底：`parse_cusd_line` 对 `(0-2)` 解析状态码会失败，
真误入了也变不成一条假应答。

### 22.2 双路径收取为什么不重复推

`+CUSD` 无论同步到达还是异步到达，现在都从**同一个 URC 队列**出：

- 同步路径 `Modem::send_ussd` → `take_pending_cusd()` → `take_urc_of([Cusd])`
  **只取 CUSD 这一类**，队列里的 `+GTSIM` / 注册态 URC 原样留下
  （否则一个功能的顺手优化会吃掉另一个功能的巡检加速信号）；
- 异步路径 `spawn_urc` → `push_cusd()`。

取走即清，所以两条路天然互斥：谁先拿到谁推，不会同一条推两遍。
为此在 `AtPort` / `Modem` 上新增 `take_urc_of(kinds)` 出口，
`LineGate::take_urc_of` 保留其它类别的入队顺序。

### 22.3 `+CMTI` 为什么要重读（URC 循环的第一个 AT 例外）

`+CMTI` 只给存储索引（`+CMTI: "SM",3`），**不带内容**；只清缓存会让页面显示
「没有短信」，比不推更糟。所以 `refresh_sms_on_urc` 重读一次 `AT+CMGL`，
推出 `sms` + `smscount`。

- 节流 10 秒：群发短信时 `+CMTI` 连着来，每条都重读会把串口占满；
- 读失败**不清缓存**（旧列表比空列表诚实），只记 debug；
- 这是 URC 循环「不发 AT」边界的第一个例外，函数注释里写明了理由与约束。

### 22.4 验证与未验证

- 单测 385 → 392：`at/urc.rs` 五个（CUSD 形态判据、查询应答放行含无空格写法、
  CMTI 识别、`take_urc_of` 只取目标类别且保留其它、多类别同取）；
  `model/urc.rs` 三个（CUSD 原文保序、CMTI 置脏、四类动作共存）。
  `cargo build` 零告警。
- **实机未验证**：当前模组短信存储为空，收不到 `+CMTI`；USSD 在该卡上受
  CS 域注册限制（`AT+CUSD` 实际不可用）。与 P1-4 短信 PDU 同一轮补测：
  换一张有短信的卡 + CS 已注册的卡。

---

## 23. 自动组网：模组一插上就有能上网的 WAN 接口（2026-10-18）

> **范围**：阶段 5 最后一块功能欠账，也是唯一还在影响**可用性**的一项 ——
> 此前新插上的模组只是「有信号、有 IP」，系统里没有对应接口，拿不到默认路由、
> 进不了防火墙 wan 区。`/api/dial` 的 `auto_wan` 字段一直是 `null`。
> **不变式**：前端调用方式不变（只是那个字段从 null 变成真实报告）；
> 用户手工建的接口一律不动。

### 23.1 三层结构（与其它域一致）

| 文件 | 职责 | 单测 |
| --- | --- | --- |
| `rust/src/netprov/mod.rs` | **纯策略**：算「该有哪些接口、各自什么 proto/device/metric」+「UCI 该动哪些行」 | 18 个 |
| `rust/src/netprov/uci.rs` | **薄 IO**：`uci show` 读现状、`UciOp` 落盘 | 6 个（只测解析） |
| `rust/src/netprov/provision.rs` | **编排**：读现状 → 算计划 → diff → 应用 → 回报 | 2 个 |

接线：`http/loops.rs::spawn_netprov`（15 秒一轮，线程数 7→8）；
`/api/dial` 的 `auto_wan` 字段返回 `AutoWanReport`
（`at` / `kind` / `mode` / `ifaces` / `applied` / `skipped` / `ok`）。

### 23.2 proto 怎么定（2026-10-13 起的口径，逐条照搬配置注释）

三种模式（`modem_pro` / `dhcp` / `both`）下 **qmi / ecm / ncm / rndis / mbim /
未知类型一律 `modem_pro`**，不再只认 QMI，MBIM 也不再例外 —— 它的会话建立改由
modemd 托管的 quectel-CM 做（只有它实现了 MBIM），接口跟其它类型一样是一张
`modem<id>`。**唯一例外**：`host_dial` 关掉时 MBIM 退回 `proto=mbim`
（那时没人拨号，只能靠 netifd 的 `mbim.sh` / `umbim`）。

顺带改准了 `config/wan_mode.rs` 的模块注释：它还写着「MBIM 是唯一例外」，
与配置注释矛盾。注释描述不存在的行为比没有注释更糟。

### 23.3 三条红线

1. **我们没建过的接口不动**：只有 proto 落在 `modem_pro` / `dhcp` / `dhcpv6` /
   `mbim` 里的同名接口才归我们管；用户手工建的 `pppoe` / `static` / `qmi`
   接口一律原样留着。**手工配置优先**。
2. **只在 Linux/OpenWrt 上真正动手**：没有 `uci` 就整段空操作（跨平台要求）。
3. **认不出的 `wan_mode` 一律按 modem_pro**：配置写错不该让网络换形态。

### 23.4 两个实现上的关键点

**① device 按模组逐个取，不用全局扫描**

`dial_iface_snapshot()` 是**全局扫网卡**，分不清哪个口属于哪个模组 ——
多模组时会把两张卡都绑到 `modem1` 上。新增 `usb_netifs_for_port()`：
从**这个 AT 口的 USB 设备**的 `net/` 子目录取，sysfs 按 USB 设备组织，
取到的才是准的。

**QMAP 模组优先绑 mux 数据口**（`qmap_mode=1`）：父口 `wwan0` 没有 IP，
地址在 `wwan0_1` 上，绑错父口 netifd 永远 NO-CARRIER（配置注释里记着实测）。

**② 零写入就不 reload**

`diff_plan` 一条都不出时，连 `uci commit` 都不跑。否则每轮循环（15 秒）都会
把 UCI 的 commit 时间戳刷成「刚改过」，触发无谓的 network reload ——
那比多写几次 UCI 糟得多。

### 23.5 为什么不用 libuci

OpenWrt 上没有稳定的 libuci 开发包可链；而 `uci show` 的输出是稳定的
`pkg.section.key='value'` 行格式，解析它只要几十行纯函数（还能单测）。
代价是每次组网 fork 两个 `uci` —— 15 秒一轮，可接受。

### 23.6 验证与未验证

- 单测 392 → **426**：`netprov/mod.rs` 18 个、`netprov/uci.rs` 6 个、
  `netprov/provision.rs` 2 个。`cargo build` 零告警；`cargo fmt --check`
  三个新文件干净（仓库既有 fmt 偏差未动，不在本轮范围）。
- **实机未验证，且它会改设备的网络配置**：`auto_wan = true` 是默认值，
  部署后第一轮就会在设备上建 `modem0` 接口并加进防火墙 wan 区。
  日志里留了逐条记录（`自动组网 模组 0（qmi，模式 modem_pro）：建接口 modem0；…`），
  部署后第一件事就是看这几行；不对就把 `auto_wan` 关掉再手动清理 UCI。

## 24. 企业微信推送：新短信自动转发到微信机器人（2026-10-18）

> **范围**：阶段 5 最后一项。此前 `/api/wechat/*` 六个接口全部落在
> 当时代码走 `stub::todo`（501，后来已删除），且 `[wechat]` 配置段有两个字段被静默丢弃。
> **不变式**：前端调用方式不变；`GET /api/wechat` 不回传 token。

### 24.1 先修一个静默丢字段的 bug

`files/etc/modemd/config.toml` 的 `[wechat]` 段注释里写着 `bot_user_id` 与
`allowed_senders`，配置里也确实有这两行 —— 但 `WeChatConfig` 结构体没收。

serde **默认忽略未知键**，所以这种丢失是完全静默的：扫码登录把凭据写回
config.toml，下次启动读不到，又变回未登录；`/api/wechat/set` 存了发送者过滤，
重启后也读不回来。用户看到的现象是「明明配过，重启就没了」。

补上两个字段（`config/wechat.rs`），并把注释写在字段上说明它为什么在。

### 24.2 三层结构

| 文件 | 职责 | 单测 |
| --- | --- | --- |
| `rust/src/wechat/mod.rs` | **纯逻辑**：模板渲染、发送者过滤、推送目标选择、登录状态机、端点构造 | 22 |
| `rust/src/wechat/transport.rs` | **薄 IO**：跑 `curl` 发 HTTPS | 1（超时夹取） |
| `rust/src/wechat/client.rs` | **运行期状态**：登录态、学到的目标、已推过的短信、节流、真正的「发」 | 16 |

### 24.3 三处纯逻辑上的判断

**① 认不出的占位符原样留着**

模板写成 `{typo}` 时消息里露出 `{typo}`，比静默变成空串好排查 ——
空串只会让人以为「这条短信没内容」。

**② 长短信后缀只在多条时出现**

`{parts}` 单条时是空串。默认模板是 `📩 新短信{parts}\n…`，
留个空括号比什么都没有更难看。

**③ 学到的发送者也要过过滤**

`listen` 开着时会学「最近给机器人发过消息的人」当推送目标（`to_user_id`
留空时这是唯一目标）。学到一个被 `allowed_senders` 过滤掉的人，
**等于把后续短信推给他** —— 那比不学更糟。所以过滤放在「学」的时候
（`should_learn_sender`），而不是「发」的时候。

### 24.4 为什么用 curl 而不是引 TLS 库

`base_url` 是 `https://ilinkai.weixin.qq.com`，而 `std` 没有 TLS。两条路：

- 引 `rustls` / `openssl`：跨交叉编译要另配一份根证书与链接参数，
  `Cargo.toml` 里「依赖刻意保持精简」的约定也就破了；
- **跑 `curl`**：OpenWrt 自带，支持 HTTPS，失败时退出码与 stderr 都能带出来。

选后者。设备上没有 `curl` 就**明确报错**，不假装发成功
（`transport::available()`，页面上也有 `transport_ok` 字段说明）。
仓库里已有 `modprobe` 的先例（`modem/usbdev.rs`）。

### 24.5 闸门：为什么 GET 也要过短信密码

`[wechat]` 比短信本身还敏感 —— 能改推送目标就能把短信转发到任意微信。
所以六个接口**全部**过短信密码闸门，包括看起来只读的 `GET /api/wechat`
（它回显 `to_user_id` 与 `allowed_senders`，正是攻击者最想知道的两项）。

实现上用「闸门只判、由调用方回」而不是让闸门自己回：
`tiny_http::Request` 是**一次性**的，被闸门吃掉就没法再回业务响应了。

### 24.6 接线：`+CMTI` 路径上的顺序

```
+CMTI 到达 → refresh_sms_on_urc()
             ├─ ① state.wechat.push_new_sms(...)   ← 同步 curl，超时 15s
             └─ ② push::push_sms / push_smscount   ← 推页面
```

微信那步放在**前面**：它是同步 curl 到公网，慢；页面广播在后，
页面的即时性不受公网往返影响。

### 24.7 三个「别过度设计」的决定

1. **运行期覆盖存在内存里，不热加载 config.toml**：改配置时由 handler 写盘
   （`config/patch.rs` 行级原地替换，保留注释）+ 覆盖内存，两个都做。
   不引入文件监听 —— 一个进程不值得为它加一套 inotify。
2. **已推过的短信按 `index|timestamp|number` 记，上限 200 条**：
   `+CMTI` 只告诉你有新的，不告诉你是哪条；`read_sms()` 回的是整张表。
   带 timestamp 是因为模组重启/换卡后 index 会从 1 重新开始，只按 index
   去重会把新卡的第一条当成已推过。上限满了丢最旧的 —— 短信本身在模组里，
   丢了只是可能重推一次，比内存无上限增长好。
3. **推送节流 3 秒**：群发短信时几十条一起到，一条一条发 curl 会把线程占满。
   节流命中时**不回滚**已记的键（宁可少推，也不要并发发一堆 curl）。

### 24.8 验证与未验证

- 单测 426 → **469**：`wechat/mod.rs` 22、`wechat/client.rs` 16、
  `wechat/transport.rs` 1、`http/handlers/wechat.rs` 4。
  `cargo build` 零告警；`cargo fmt --check` 新文件干净。
- **实机未验证，且 API 路径没有对过官方 SDK 文档**：iLink Bot 的路径是从
  配置注释与行为描述反推的，集中在 `wechat/mod.rs::api` 一处的常量里
  （`QR_PATH` / `QR_CHECK_PATH` / `SEND_PATH` / `POLL_PATH`），
  对得上的时候只改那一处，别散着改。扫码登录 → 发消息的完整链路需要在
  设备上配好机器人后走一遍。

---

## 25. 拨号器托管：QMI/MBIM 的数据呼叫交给 modemd 看着（2026-10-18）

### 起因

阶段 6 的最后一块。此前 QMI/MBIM 模组的 `/api/dial/connect` 是**明确报错**的：
「数据呼叫归主机侧厂商拨号器（拨号器托管属阶段 6，尚未迁移），请手动运行」。
于是 FM350/RW350 这类模组在页面上点「拨号」只会拿到一句红字，用户得自己
ssh 上去敲 `quectel-CM -s <apn>` —— 而 `files/etc/modemd/config.toml` 的
`[net] host_dial` 注释里明明写着：

> modemd 每轮组网循环把它拉起、看着、退出自动重试；模组拔了/手动断开就停掉；
> 进程退出前按进程组一起收（连它起的 busybox udhcpc 一起），不留占着控制口的尾巴。

代码与配置对不上，就是这个阶段要还的债。

### 结构：两层（不是三层）

`rust/src/runtime/dialer/`：

| 文件 | 职责 | 单测 |
| --- | --- | --- |
| `mod.rs` | **纯策略 + 编排**：命令行怎么拼、该不该跑、怎么收 | 22 |
| `proc.rs` | **薄 IO**：拉起、探活、按进程树收 | 5 |

比 `netprov` 少一层，因为这里没有「读现状」这回事 —— 要读的只有 `/proc`，
而且 `proc.rs` 自己就读了。硬拆一层只会多一次转发。

### `plan_dialer()`：把配置注释里散着的口径收成一处

配置注释把拨号器口径散写在 `host_dial` / `host_dial_bin` / `host_dial_pdn`
三段的注释里，代码里此前一处都没体现。现在集中到 `mod.rs::plan_dialer`：

- **MBIM 一律 `quectel-CM`**（不管模组是哪家）：只有它实现了
  `mbim-cm.c` + `SOFTWARE_MBIM`；`fibocom-dial` / `simcom-cm` 源码里压根
  没有，`meig-cm` 有文件但没接上。而它认 MBIM 靠的是**标准 USB CDC
  描述符**，与厂商无关 —— 所以「用别家拨号器顶」在 MBIM 上是可行的，
  在 QMI 上不可行（各家 QMI 初始化序列不同）。
- **MBIM 必须有 APN**：没 `-s` 拨号器直接 `exit(-404)`，所以这里就报错，
  不让用户等一轮空。
- **`-i` 给父口**：QMAP 的驱动属性在 `wwan0`（`qmap_mode` 在它上面），
  数据在 `wwan0_1`。给错父口拨号器起不来。
- **`-n <pdn>` 只传给 quectel-CM / fibocom-dial / meig-cm**：
  **simcom-cm 不认这个参数**（配置注释原话）。
- **`host_dial_pdn` 三值语义**：`>0` 强制；`-1` 永不按 QMAP 兜底（只按
  网卡名反推，老行为）；`0`（默认）先按网卡名反推（`wwan0_1` → 1），
  反推不到但驱动是 QMAP（`qmap_mode=1`）就兜底传 1。
- **`host_dial_bin` 指向不存在的路径直接报错**，不静默退回别的
  （配置注释原话）。所以 `resolve_bin_path` 是纯函数只拼路径，
  存在性由 `proc::spawn` 查，两处分开才好各自单测。

### `reconcile()`：`看着`那一半

```
模组没了 ────────────────→ 收起
拨号器自己退了 ──────────→ 收尸（下一段重拉）
拨号总开关关掉 ──────────→ 收起
用户手动断开（manual_stop）→ 收起
非 QMI/MBIM 类型 ────────→ 收起（走模组侧 AT 拨号）
计划变了（APN/网卡变了）──→ 停下旧的、起新的
计划没变 ───────────────→ 一个动作都没有
```

最后一条是硬要求：**每轮循环跑到已配好的模组必须是彻底的空操作**。
否则 15 秒一轮会把拨号器反复重启，链路永远建不稳。

### `reap_dead()`：测试抓不到的那个 bug

配置注释的「退出自动重试」靠它。没有它会这样：拨号器退一次后 slot 还在、
计划也没变，`reconcile` 走到「已在跑同一个计划 ⇒ 不动」就跳过了 ——
于是**再也不起来**。

这个 bug 单测抓不到：`reconcile` 的输入是真进程的死活，纯函数测试里
`status()` 永远等于计划。是写代码时推演「拨号器自己退了会怎样」发现的。
同类坑在 netprov 里也有一个（`run_once` 提前 return 连带跳过拨号器托管，
见下）。

### `proc.rs`：为什么不按进程组收

配置注释的原话是「按进程组一起收（连它拉起的 busybox udhcpc 一起）」。
进程组是对的，但纯 `std` 拿不到 `setsid()` —— 那要引 `libc`，而
`Cargo.toml` 的「依赖刻意保持精简」不想为它破例（这个仓库现在连 `libc`
都没直接依赖：`serialport` 关了 default features、`ctrlc` 这版也没带）。

所以改成**扫 `/proc` 收进程树、按正 pid 发信号**：

1. 不依赖 `setsid` 到底 fork 不 fork —— fork 了 pid 就对不上，组也更难找；
2. 不依赖 BusyBox `kill` 收不收负 pid（进程组写法要 `kill -TERM -- -PGID`）；
3. 收的是**真实的那棵树**（拨号器 → udhcpc → 它再起的脚本），而不是
   「碰巧同组的所有进程」。

代价是每次 `stop` 扫一遍 `/proc` —— 路由器上进程数几十个，可忽略。

⚠️ `/proc/<pid>/stat` 的第 2 个字段是**可含空格与括号的 comm**，不能按
空格切 —— 从最后一个 `)` 之后开始切才对。`ppid_of()` 就是干这个的，
单测 `ppid_parsing_survives_spaces_in_comm` 拿本进程钉住它。

⚠️ **`spawn` 必须把 `Child` 交回去**：Unix 上父进程不 `wait` 的话，子进程
退出后会变成僵尸，`/proc/<pid>` 依旧存在 —— 那时「探活」会永远认为它
活着，「退出自动重试」彻底失效。第一版就是只存 pid，推演时发现的。
现在的做法：`alive()` 连状态字段一起看（`Z` 算不在）、`Slot` 存 `Child`、
`stop` 在 KILL 之后也等到树真的干净（不等的话调用方紧接着 `reap` 时进程
还在死，`try_wait` 返回「还活着」就不收了）。单测 `zombie_is_not_alive`
起一个立刻退出的子进程、故意不 wait，把这条路钉住。

### 接线与顺序

- `netprov/provision.rs::run_once` **末尾**调拨号器托管，放在 UCI 之后：
  先把接口建好，拨号器起来时网卡才在（QMAP 的 `wwan0_1` 要等 netifd
  把接口拉起来）。
- **顺手修的真 bug**：`run_once` 开头原本是
  `if !auto_wan || !uci::available() { return; }`，拨号器托管会被它连带
  跳过。而 `auto_wan=false` + `host_dial=true` 是**合理组合**（接口用户
  自己建，拨号器还是 modemd 托管）—— 那样拨号器永远不跑。现在改成
  「建接口那段受开关管，拨号器托管无条件跑」。
- `dial_inner()` 的 QMI/MBIM 分支从「报错」改成「**交办**」：APN 在上面
  `dial_update` 时已经落到状态里，下一轮循环按它起拨号器。
  `msg` 说清楚是「已交办」不是「已连上」—— 进程起来到拿到地址要几秒，
  页面随后由 `dial` 推送刷新。MBIM 且**没有任何 APN 可用**时仍报错。
- `dial_disconnect.rs`：`hangup` **之前**先停拨号器。顺序要紧 —— 反过来
  的话模组侧先 `AT+CGACT=0`，拨号器下一轮 QMI 轮询发现承载没了会自己
  重拨，而我们刚把它标成 `manual_stop`，两边会抢。
- `dial_enable.rs` 关拨号的文案改成如实描述（拨号器会被收起），不再留
  「尚未迁移」的能力缺口说明。

### 验证

`cargo build` 零告警；`cargo test` 469 → **491** 全绿
（dialer 27 个：`mod.rs` 22 + `proc.rs` 5，其中 Unix 专测 6 个在
Windows 上被 `#[cfg(all(test, unix))]` 挡掉，留 1 个平台无关冒烟）。

**实机未验证，且命令行参数没在真机核对过**：拨号器源码在 `third_party/`
（构建时由 Makefile 从 third_party 拉），参数是照配置注释与各家通用
用法拼的。全部集中在 `mod.rs::plan_dialer` 一处，对不上的时候只改那一处。

拨号器自己的 `stdout/stderr` 进 `null`：它每轮刷一大片，而 modemd 的
日志通道是「日志文件或 stderr」二选一（见 `logging/logger.rs`），
`inherit` 不一定落得了盘；拿管道接又要防背压满阻塞。排障靠 modemd
这边打的完整命令行（`command_line()` 设计成可复制手跑）+ 退出原因。
真要在设备上看拨号器自己说什么，得另外想办法。

---

## 26. 脱敏口径与遗留（仓库是 public）

每次推送前按 §12 第 9 条扫一遍。当前状态：

| 项 | 状态 |
| --- | --- |
| 实机 IMEI | ✅ 已脱敏。全仓库统一示例值 `867698040012349`（Luhn 合法）；历史已 `git filter-repo` 重写强推 |
| ICCID / IMSI / 手机号 | ✅ 无。示例报文用 `123456789012345` 这种一眼假的串 |
| 口令 / 令牌 / 私钥 | ✅ 无。`bot_token` / `sms_token` 在配置里都是空串，实配值由用户在设备上填 |
| 内网地址 | ✅ 无。文档里是 `<路由器地址>`；`192.0.2.x` 属保留段，本就没出现过 |
| `AT+EGMR=1,…` | ✅ 只有**命令模板**（带 `<imei>` 占位符），没有真值 |
| `DISPLAY_KEY` | ⚠️ **已知遗留**，见下 |

### 26.1 `DISPLAY_KEY` 为什么还没改

`rust/src/http/ws.rs` 里：

```rust
pub const DISPLAY_KEY: &str = "h29k-tft-9f3c1a7b";
```

三个问题叠在一起：**免密**（拿到 key 就能看 `/display/ws` 的屏端推送）、
**硬编码**（改它要重新编译）、**三份文档里明文记录**（`docs/HANDOFF.md`、
`docs/PORTING.md`、`docs/PROGRESS.md` 都写着这个值）。

没擅动的原因：它牵动前端——`rust/web/*.html` 里有用这个 key 建连的地方，
改配置项就得同步改前端调用方式，而「不得要求修改前端」是硬约束。要做的话
顺序是：① 配置项 + 首次启动随机生成 + 写回 `config.toml`；② 前端改成从
`/api/session` 之类拿（不能硬编码）；③ 文档里的明文值一并换掉。

危害评估：它是 **LAN 内展示通道**（副屏/调试台），不碰 AT、不碰配置写回，
所以优先级低于「短信密码闸门」那类。但既然是 public 仓库，key 等于公开，
局域网内任何人都能连 —— 这一点是真实的。

---

## 27. 屏端动作协议收口：WS 的 8 个 action（2026-10-19）

阶段 5 最后一块（屏端 `/display/ws` 的**客户端 → 服务端**指令）补齐，并在 FM350-GL
实机（ImmortalWrt SNAPSHOT / aarch64 / TZ=CST-8）上端到端验证过。

- **协议**：`{action, modem_id, index, value, number, text}`；未给的字段按零值（屏端只发它关心那几个），
  解析失败或 action 不认识都**静默忽略**（屏端可能有老固件在连）。
- **8 个 action**：
  | action | 参数 | 行为 | 推送 |
  | --- | --- | --- | --- |
  | `snapshot` | — | 再来一份模组列表快照 | `modems` |
  | `ping` | — | 回 `{"type":"pong"}` | — |
  | `sms_read` | `index` | `AT+CMGR=<index>`（3GPP：读一次即已读）→ 重读列表 | `sms` + `smscount` |
  | `sms_delete` | `index` | `AT+CMGD=<index>` → 重读列表（条数/角标一起变） | `sms` + `smscount` |
  | `mode_get` | — | 读组网模式（含屏端三个按钮的可写性） | `mode` |
  | `mode_set` | `value` | 下发 + 回读确认，成败都给一句话反馈 | `mode` |
  | `sms_send` | `number`,`text` | 发短信（UCS2 / 分段由 `model/sms.rs` 管） | `sendsms` |
  | `net_get` | — | 读一次网络状态（屏端是 Passive，缓存冷时运营商名会一直空） | `network` |
- **为什么除 `snapshot`/`ping` 外全丢独立线程**：发短信要等提示符、切模式要等回读确认，
  都要占串口几秒 —— 卡在 WS 读循环里会让这一路的心跳与推送全停（与上游 goroutine 同口径）。
- **`mode` 消息**（屏宽 320px 的取舍）：`value` / `name`（中文全名）/ `short`（右上角短名：
  自动 / 仅4G / 仅5G / 4G+5G）/ `choices`（全部档位）/ `screen`（那三个按钮：
  `LTE`→4G、`NR5G`→5G、`LTE:NR5G`→4G+5G，`writable=false` 的置灰）/ `read_only` / `msg`。
  空串与 `false` 一律 omit（屏端按"字段在不在"决定画不画那一行）。
- **实机验证**（FM350-GL，固件不提供可读写组网模式指令）：
  - `mode_get` → `{"read_only":true,"choices":[…writable:false],"screen":[…]}`（与固件实际一致，按钮全置灰）；
  - `net_get` → `network`（运营商「中国电信」/ APN `ctnet` / IPv4+IPv6 齐全）；
  - `ping` → `pong`；握手与推送在同一路 WS 上混跑正常。
- **顺带修的 Linux 必红用例**：`runtime/dialer/proc.rs` 的 `process_tree(死 pid)`
  原来恒返回 `[root]`，与用例「死 pid 没有树」冲突（Windows 开发机跑不到 `unix_tests`）。
  改为根不在 `/proc` 里就直接返回空 —— `stop(死 pid)` 不再白等两轮 `wait_gone`。
- **验证**：单测 501 全绿（新增 4：指令解析与默认值、存储位置校验、屏端短名、`mode` JSON 形状）；
  `cargo build` 零告警；aarch64 musl 静态二进制（2.5 MB）已部署实机，服务起得来、
  8 个后台线程、接口与 WS 动作全部实测通过（**未重启设备**、**未碰 IMEI**）。

---

## 28. 主机侧网卡配置：`/api/host/nic*`（2026-10-19）

迁移清单里最后一组接口。背景：**FM350 系列**的 USB 网卡不给主机发 DHCP ——
模组侧拨号明明成功，主机那张网卡却长期挂在 `169.254.x.x` 上、没有默认路由，于是"上不了网"。
这种机器要在主机侧把网卡配成与模组同网段、网关指向模组。

- **纯逻辑**（`runtime/hostnic.rs`）：机型特例 `quirk()`（只认 FM350，判据用 `contains`
  因为调用方可能给 ATI 原文）、掩码/前缀互转（非连续掩码要回写比对一次）、配置校验与归一化
  （掩码统一点分、DNS 去空去重）、建议值 `suggest_plan()`（同网段取 `.2`、网关指向模组，
  与自己撞了就取 `.3`）、三组命令 `renew` / `static` / `dhcp`
  （Linux 走 `ip` + `udhcpc`，Windows 走 `netsh`；**OpenWrt 不写 `/etc/resolv.conf`**，
  DNS 交给 dnsmasq）+ OpenWrt 的 UCI 持久化块。
- **命令默认只生成不执行**：改网卡要 root，一键改 `/etc/config/network` 风险太大
  （接口名/防火墙分区得人判断）→ UCI 那批全部 `manual=true`。`execute=true` 时才下发，
  且走 `prog + args`（**不拼 shell**），超时按命令自带的值（DHCP 那条 90 秒）。
- **本机枚举** `interfaces()`：解析 `ip addr show`（失败退 `/sys/class/net` 只列名字），
  去掉环回与明显虚拟网卡（含 OpenWrt 的无线口 `phy*-ap*`），像模组口的排前面；
  只有 `169.254` 的网卡、且**像模组口**时才提示"这个机型要手工配一次"
  （否则蓝牙 / 手机网络那些也挂 169.254，全写一遍会误导）。
- **台账** `HostNicStore`：`hostnic.json`（与配置同目录，原子写），切网卡时把保存过的参数带出来。
- **实机验证**（FM350-GL / ImmortalWrt）：`required=true`、`platform=openwrt`、`elevated=true`、
  `modem_ip=10.42.0.16` → 建议值 `10.42.0.2/24` 网关 `.16`；UCI 持久化 10 条全部 `manual`；
  `apply` 默认 `executed=false`（只生成），非法 IP 时 `commands` 为空只给 `note`；
  `save` 后 `saved=true`、掩码归一成点分、DNS 两条都在。**未执行任何命令**（只生成），
  测试用的 `hostnic.json` 已删除、设备恢复原状。
- **顺带**：`Modem` 加 `dial_state_ipv4()`（不发 AT）与 `query_own_ipv4()`（补一条 `AT+CGPADDR`）。
- **验证**：单测 516 全绿（新增 15）；`cargo build` 零告警；aarch64 静态二进制部署实机跑通。

---

## 29. 模组组网修复：数据口选择、匿名 zone、350 系列 static（2026-10-19）

用户反馈："本插件未拉起创建 OpenWrt 的网络接口，切默认了 eth0，换成 eth2 或者之后的"。
在 FM350-GL 实机（ImmortalWrt / aarch64 / eth0=LAN、eth1=WAN、**eth2=rndis_host 数据口**）
上逐项定位，修掉四个真缺陷，并把 350 系列的组网口径对齐参考实现。

### 29.1 数据网卡选择（`pick_device` / `detect_data_dev`）

- **驱动优先**（`DATA_DRIVERS = rndis_host / cdc_ether / cdc_ncm / qmi_wwan / mbim`），
  多个候选时先挑**模组厂商标**（Fibocom `2cb7`、MediaTek `0e8d`…）再按名字排序；
  兜底名字 `eth2 / eth3 / wwan0 / usb0`（**不含 eth0/eth1**）。
- **排除板载与已占用**：`mtk_soc_eth` / `mt798x-wmac` / `bridge` / `gmac` … 一律不算数据口；
  UCI 里已有接口占用的 device（lan 的 `br-lan`、wan 的 `eth1`）也排除 —— 绝不能把
  用户正在上网的口配成蜂窝口。实机正是靠这条从 eth0/eth1 收敛到 **eth2**。

### 29.2 防火墙 zone 定位（`uci::parse_existing`）

实机的 wan zone 是**匿名 section** `firewall.@zone[1]`（`name='wan'` 只是它的选项），
而旧实现把 zone 名当 section 名 → `uci add_list firewall.wan.network=…` 必失败
（`Invalid argument`），接口建了却进不了 zone。现在：
- 解析 `firewall.<sec>=zone` + `firewall.<sec>.name` + `firewall.<sec>.network`（**列表值按空格拆**）；
- 返回 **zone 名 → 真实 section 路径**（`wan` → `firewall.@zone[1]`），diff 用它生成
  `add_list` / `del_list`；具名 section（`firewall.wan`）没有 `name` 时退回 section 名。

### 29.3 reload 与"netifd 未加载"补刀（`uci::reload_network` / `interface_loaded`）

- 旧实现把 `/etc/init.d/network reload` 当 **uci 子命令**跑（`uci -c /etc/init.d/network reload`）
  → 实机回 `Usage: uci …`，**UCI 写进去了但从不生效**，接口一直 DOWN。
- 新增 `interface_loaded()`（`ifstatus <name>`）：「UCI 已符合计划」但 netifd 里没有该接口时
  **补一次 reload**（否则上次 reload 失败后永远不恢复）。

### 29.4 350 系列（FM350 / RW350）走 static（对齐参考实现）

FM350 的 RNDIS 网卡**不发 DHCP**（实测几十秒到一两分钟才回、或干脆不回）。参考实现
（`luci-app-fm350`）的做法是把接口写成 `proto=static` + **模组上报地址直接落在主机网卡上**。
本仓库现在同口径：`_dhcp` 模式下、机型命中 FM350 且能取到模组 IPv4 时——
`modem0` = `proto=static`、`ipaddr=<CGPADDR>`、`netmask=255.255.255.0`、
`gateway=<同网段 .1>`、`metric` 按 `[net] wan_metric`；`modem_pro` 模式仍交给 netifd 协议。

- 地址来源：`dial_state.ipv4` → `own_ipv4_cache`（网络查询顺手填）→ `AT+CGPADDR` 补查；
  `sync_dial_state` 原来整段被「没拨过号就不动」挡住 → idle 时永远拿不到地址，已解耦。
- `is_ours` 放行 **`modem<N>` 命名空间内的 static**（且必须带 `ipaddr`、不含 `_6` 兄弟接口）：
  否则我们自己写的 static 下一轮会被当成「用户手工接口」而永远不再更新、也清不掉旧静态项。
- 从 static 切回 dhcp 时用新增的 `UciOp::Unset` 清掉 `ipaddr/netmask/gateway` 残留。

### 29.5 实机验证（FM350-GL，`auto_wan=true`）

- 日志：`350 系列不走 DHCP，按模组上报地址配 static（10.42.0.16）`；
- UCI：`modem0.proto=static / device=eth2 / ipaddr=10.42.0.16 / netmask=255.255.255.0 /
  gateway=10.42.0.1 / metric=20 / auto=1`，`firewall.@zone[1].network` 追加 `modem0` `modem0_6`；
- `ifstatus modem0`：`up=true`、`address=10.42.0.16`；`ip -o -4 addr show eth2` 有地址；
- **`ping -I eth2 223.5.5.5` 2/2 通（53 ms）** —— 模组 PDP → 主机 eth2 → 公网整条链路打通；
- **WAN 未被改动**：`ifstatus wan` up、地址仍 `192.168.1.186`；wan zone 的 `network='wan' 'wan6'` 保留。
  设备原本 `wan.defaultroute='0'`（本来就没有默认路由），模组接口以 `metric=20` 提供默认路由；
  若用户把 wan 的默认路由打开（metric 10），优先级自然回到 WAN。

### 29.6 事故记录（如实记）

为清掉上一轮失败建出的接口，我执行了 `uci -q delete firewall.@zone[1].network` ——
`uci delete` 删的是**整个列表**，把 wan zone 的 `wan wan6` 一起删了（等于动了 WAN）。
发现后立刻按原值 `add_list` 恢复并 `commit` + `firewall reload`，`ifstatus wan` 复测 up、
地址未变。教训：**清 zone 成员只能用 `del_list`（逐项），永远不要 `delete` 整个列表**。

- **验证**：单测 524 全绿（新增 8：数据口排除/驱动常量、匿名 zone 列表、static 计划、
  网关推导、static 写入与回退清理、命名空间判定）；`cargo build` 零告警；
  aarch64 静态二进制部署实机跑通（**未重启设备、未碰 IMEI**）。

---

## 30. 350 系列 IPv6 取址：按 `luci-app-fm350` 的 `v6_mode` 落地（2026-10-19）

用户要求「350 系列 V6 获取方式也要参考 `luci-app-fm350`」。该参考实现的 `net/v6.rs`
把 IPv6 明确分成四路，核心结论是：**IPv6 不能照抄 IPv4 的静态方案**，默认要走
`dhcpv6`（odhcp6c 在用户态收 RA），`ra` 模式必须自己打开 `accept_ra=2` 并**主动发 RS**，
`AT+CGPADDR` 的地址在上下文去激活后会变成残值、不能直接写成 `/128`。

### 30.1 新增配置 `[net] wan_ipv6_mode`

| 值 | 谁负责地址 | 本程序做什么 |
| --- | --- | --- |
| `dhcpv6`（默认） | odhcp6c（用户态 raw socket） | `modem<id>_6`：`proto=dhcpv6`、`device=@<主接口>`、`extendprefix=1`（`[net] lan_ipv6` 开着时）；**不写地址、不改 sysctl** |
| `ra` | 内核按 RA/SLAAC | `proto=none`；打开 `accept_ra=2 / accept_ra_defrtr=1 / accept_ra_pinfo=1`；往 `router_solicitations` 写 1 主动发 RS（60 秒节流，进程起来 30 秒后才开始） |
| `static` | 本程序写 `/128` | `proto=static` + `ip6addr=<addr>/128`；reload 后补 `ip -6 route replace default dev <dev> metric <m>`；**地址必须通过 `usable_static_v6` 过滤** |
| `off` | 不托管 | 不建兄弟接口；已有的（`modem<N>_6`，proto 归我们）会被收掉 |

`wan_ipv6 = false` 仍是**总开关**（优先于 `wan_ipv6_mode`，等价于 `off`）。
认不出的取值一律按 `dhcpv6` 处理（与参考实现 1.0.11 起同口径）——早先「不是那三个值就按 RA」
的写法会让配置缺省/拼错的用户掉进一条不通的路径。

### 30.2 与参考实现对齐的四个细节

1. **`device=@<主接口>`**（原来绑裸网卡名）：主接口是 `modem_pro` 时它的设备是 netifd
   动态建出来的（`wwan0_1` 之类），绑裸名会绑错；参考实现同样是 `device=@fm350`。
2. **`extendprefix=1`**：odhcp6c 拿到的 `/64` 要让 odhcpd 继续分给 LAN，否则内网设备
   只有 ULA / 没有公网 IPv6。由 `[net] lan_ipv6` 控制。
3. **无前缀残值过滤**（`v6::is_usable_global_v6`）：实机 `AT+CGPADDR=1` 在只拿到 IPv4 时
   回 `0.0.0.0.0.0.0.0.24.223.154.1.249.130.129.90`，解码是 `::18df:9a01:f982:815a`
   —— 只有接口标识、前缀全零。这种地址写进 `/128` 会配出一条永远不通的默认路由；
   `static` 模式下检测到就**自动退回 `dhcpv6`** 并 warn。
4. **形态对齐只在有差异时发生**：`extendprefix` / `ip6addr` 都进了 `diff_plan`；
   从 `static` 切回 `dhcpv6` 用 `UciOp::Unset` 清掉旧 `/128`（否则会跟 odhcp6c 抢地址），
   从 `dhcpv6` 切到 `ra/static/off` 清掉 `extendprefix`。`proto=none` 的 `modem<N>_6`
   也算「我们建的」（否则 RA 模式下永远无法再改回 dhcpv6）。

### 30.3 实机验证（FM350-GL / ImmortalWrt / `wan_mode = dhcp`）

- **dhcpv6（默认）**：`modem0_6` 变成 `proto=dhcpv6`、`device=@modem0`、`extendprefix=1`、
  无 `ip6addr`；下一轮零写入（幂等）。`ifstatus` 的 `device` 正确解析回 `eth2`。
- **ra**：`modem0_6.proto=none`、`extendprefix` 被清空；`/proc/sys/net/ipv6/conf/eth2/`
  的 `accept_ra=2`、`accept_ra_defrtr=1`、`accept_ra_pinfo=1` 全部生效。
- **static**：日志出现「模组没报可用的全局地址，本轮退回 dhcpv6」，`modem0_6` 回到
  `proto=dhcpv6` —— 残值没有被写成 `/128`（这正是这台设备最容易踩的坑）。
- **本机网络实况**：这张 SIM / APN（`ctnet`，PDP `IPV4V6`）**没有下发可用的全局 IPv6**
  （`AT+CGPADDR` 的 v6 位是上述残值，`ip -6 addr` 只有 `fe80::`），所以 `modem0_6`
  一直是 `pending` —— 这是运营商侧没给地址，不是组网失败；换到下发前缀的网络时
  odhcp6c 会直接接上，`extendprefix=1` 再把 `/64` 分给 LAN。
- **未动 WAN**：`ifstatus wan` up、地址仍 `192.168.1.186`；`ping -I eth2 223.5.5.5` 2/2 通，
  默认路由仍是 `via 10.42.0.1 dev eth2 proto static metric 20`。
- **验证**：单测 536 全绿（新增 12：模式解析/兄弟接口形态/四路计划/残值过滤/地址行解析/
  RS 门控/旧形态升级/切回清理）；`cargo build` 零告警；aarch64 静态二进制部署实机，
  **未重启设备、未碰 IMEI、未改 WAN**。

---

## 31. 厂商模块拆分 + 原生 QMI 拨号器（2026-10-19）

两件事一起做：把散落的厂商差异收成**一厂商一模块**，再把主机侧拨号器从
「外部 C 二进制」补上**Rust 原生 QMI** 这条路。

### 31.1 厂商模块拆分（`modem/vendor/`）

结构对齐上游的 `iface` + `module/<vendor>`：

```text
modem/vendor/
  mod.rs      Vendor 枚举 + VendorSpec 统一接口 + 注册表
  quectel.rs / fibocom.rs / simcom.rs / meig.rs / tdtech.rs / unisoc.rs / unknown.rs
```

`VendorSpec` 是唯一入口，调用方只写 `vendor.spec().xxx()`：

| 能力 | 之前散落在 | 现在 |
| --- | --- | --- |
| AT 预设表 | `at/presets.rs`（6 个函数） | 各厂商模块 `at_presets()` |
| 不支持的标准指令 | `at/presets.rs` 的 `match` | `suppressed_common()` |
| 厂商指令集 | `http/handlers/vendor_cmds.rs` 的 `match` | `vendor_cmds()` |
| 流量候选指令 | `model/traffic.rs` | `traffic_candidates()` |
| 温度指令 | `model/siminfo.rs` | `temperature_cmds(mtk)` |
| USB 网卡写入策略 | `model/usbnet.rs` | `usbnet_write(value)` |
| 组网模式读指令/解析分支 | `model/mode.rs` | `mode_read()` |
| 主动扫小区 | `model/netscan.rs` | `cell_scan_cmd()` |
| 拨号器 / 识别关键字 / 厂商名 | `vendor.rs` | 各模块 |

默认实现一律**保守**（没有就不给）：新增一家厂商只要加一个文件、在
`Vendor::spec()` 挂一行，调用方一处都不用改。所有表都与拆分前逐字一致，
`cargo test` 的厂商用例（识别顺序、展锐抑制 `AT+CGCONTRDP`、预设合并、
流量候选顺序、USB 写入拒绝文案）全部照旧通过。

### 31.2 原生 QMI 拨号器（`runtime/dialer/{qmi,native}.rs`）

`qmi.rs` 是**线协议**：QMUX 分帧、TLV、WDS 请求/应答，编号逐条对齐
`files/third_party/quectel-cm`（`QCQMUX.h` / `QMIThread.c`）：

- 帧头 13 字节（IFType + len(整帧-1) + flags + service + client + qmux flags +
  tid + msg id + payload len），与 libqmi/quectel-CM 同口径；
- `CTL Sync` → `CTL Get Client ID` → `WDS Start Network Interface`
  （v4 service `0x01`、v6 service `0x11`）→ `WDS Get Runtime Settings`；
- 退出发 `WDS Stop Network Interface`（带 start 返回的 handle）；
- `Get Packet Service Status` 做探活（`2` = connected）。

`native.rs` 是**会话层**：从数据网卡 sysfs 反查 `/dev/cdc-wdmN`（逐级找
`usbmisc`，**不猜** `cdc-wdm0`，多模组不会串）、启动前把
`/sys/class/net/<dev>/qmi/raw_ip` 置 `Y`、v4 必须成功 / v6 失败只记 note。

**职责边界与外部拨号器完全一致**：原生只做数据呼叫，地址/DNS/默认路由仍由
netifd 的 `modem_pro`（udhcpc + odhcp6c）取 —— 所以切后端不改变接口形态，
也不和 netifd 抢同一张网卡。

接入方式：`[net] host_dial_bin = "native"`（`rust` / `builtin` 同义）。
只接管 QMI，MBIM 仍交给 quectel-CM（只有它实现了 MBIM 那套）。
supervisor 每轮：同计划 → 探活；掉线 → 重建；失败 → **5 秒退避**
（QModem 的重试循环同节奏），只影响这个模组。`/api/dial` 新增 `host_dialer`：
原生返回 `backend=native-qmi` + handle/MTU/模组侧地址，外部返回 `bin/args/cmd`。

### 31.3 验证

- 单测 **553 全绿**（新增 15：vendor spec 注册/默认能力、QMI 帧 golden bytes、
  半截帧/坏长度、TLV 与结果码、client id 与结果码同号不混淆、start handle、
  runtime settings、packet status、原生开关别名、parent_iface 只切数字后缀）；
  `cargo build` 零告警。
- 实机（FM350-GL / ImmortalWrt，**未重启设备、未碰 IMEI、未动 WAN**）：
  - `vendor= fibocom`，`/api/vendor/cmds` 与 `/api/at/presets`（45 条）都来自新模块；
  - `host_dial_bin = "native"` 时服务正常、FM350（ECM）不受影响（原生只接管 QMI），
    配置已还原为 `""`；
  - `modem0` 仍 up（`10.42.0.16`），WAN 地址未变。
- ⚠️ **原生 QMI 未在 QMI 真机验证**（测试机只有 FM350）：线协议有单测，
  数据呼叫/地址取用需要 QMI 模组；因此它是**显式开关**，默认仍走厂商二进制。

---

## 32. 350 系列识别扩到 RW350（2026-10-19）

参考实现（`luci-app-fm350`）只认 FM350，但 **RW350 与 FM350 在主机侧完全同构**
（同一套 MTK 平台：USB 网卡不发 DHCP、`AT+GT*` 私有族、`AT+GTSENRDTEMP=0` 读温度、
`AT+GTCCINFO?` 读小区、`AT+EGMREXT` 读写 IMEI）。

- 判定收口到一处：`modem::model::is_fibocom_350(model)`（`contains("FM350") ||
  contains("RW350")`，大小写无关）。
- 三个旧入口全部改为调用它：`Modem::is_fibocom_mtk`、`runtime::hostnic::quirk`、
  `model::dial::host_nic_quirk` —— 之前两处各抄了一份 `contains("FM350")`，
  加型号时必漏。
- 厂商识别：广和通关键字补 `RW350`（有些固件 `ATI` 只回型号、不带 `Fibocom`）。
- `/api/host/nic` 的 `required`、`/api/dial` 的主机网卡候选提示都随之覆盖 RW350。
- 验证：单测 553 全绿（新增断言：`quirk("RW350-CN")`、ATI 原文含 RW350、
  `FM160-CN` 不命中）；`cargo build` 零告警。

---

## 33. 收尾巡检：静态检查、死代码、配置与路由闭环（2026-10-19）

按「能实现的全部实现、能验证的全部验证」做了一轮全项目巡检。

### 33.1 静态检查（clippy 清零）

`cargo clippy --all-targets` 从 **1 个 deny 级错误 + 28 条警告** 清到 **0**：

- 错误：`schedule/tests.rs` 的 `6_272_64` 被当成 `i64` 后缀（改成 `627_264`）；
- 死代码/过期注解：删掉 `WsHub::has_clients`、`netselect::is_automatic`、
  `Modem::active_cids` / `sms_read_at`、`SmsCache.read_at`、`netprov::tests::ex`，
  以及 `broadcast` / `client_count` / `active_count` 上过期的 `#[allow(dead_code)]`；
- 写法：`Entry` 替代 `contains_key + insert`、`split(['&','/','-'])`、`div_ceil`、
  `is_some_and`、`vec![]` 初始化、`&mut [Action]`、派生 `Default`；
- 参数过多（`plan_wan` / `plan_dialer` / `wds_start` / `observe_numeric`）标注
  `#[allow(clippy::too_many_arguments)]` 并写明理由（拆结构体会让所有调用点更长）。

### 33.2 占位代码与路由闭环

- `handlers/stub.rs` 与 `is_known` 兜底**删除**：96 条登记路由全部有 handler
  （含两个 WebSocket 升级路径，在 gateway 层分流），不再有 501「尚未迁移」。
- 新增 `router::tests::route_table_has_no_duplicates`：校验 `ROUTES` 无重复、
  非空、且只含 `/api/*` 与两个 WS 路径。
- 全仓 `TODO` / `FIXME` / `unimplemented!` 扫描为 0（只剩测试字符串 `XXX-9:30`）。

### 33.3 配置与构建面

- 删掉一批**从未被 Rust 读取**的配置项（示例配置里却有大段注释，会让人以为生效）：
  `[net] qmi_proto` / `qmi_v6_sibling` / `v6_fix_models` / `lan6_fix_models`、
  `[server] auto_detect_vendor`、`[serial] read_stuck_threshold` / `probe_warmup`、
  `[sms] send_queue_size`、`[modem] enable_nr_signal` / `tty_per_modem`
  （整个空的 `[modem]` 段一并删除）；等价场景由 `wan_mode` + `wan_ipv6_mode` +
  `lan_ipv6` 覆盖，配置里写明「未移植」。
- **修掉一条断掉的调用链**：`POST /api/sms/storage` 一直把选择写回 `[sms] sms_memory`，
  但没有任何代码读它 —— 所谓「重启后沿用」从未生效。现在 `ModemManager` 把偏好注入
  模组，`read_sms` 首次调用时应用一次（成功才缓存，失败只 warn、沿用当前区），
  不再每次读短信都发 `AT+CPMS`；换卡清缓存时重置，下次读短信会重新应用。
- **资源释放**：原生 QMI 会话在停止/启动失败时发 `CTL Release Client ID`
  （QMI 每个服务只有 16 个 client 槽位，反复重拨不释放会把槽位用光），
  v4 起不来也先还 client 再报错。
- **netifd 时序**：`static` IPv6 的默认路由改为每轮确认（地址是 netifd 异步加的，
  第一轮 reload 时可能还没上内核，原来只在「有变更」那轮补一次，路由会永久缺失）。
- **退出可靠性**：init 脚本加 `term_timeout 20`（落盘 + QMI Stop 最多 15 秒，
  procd 默认 5 秒会 SIGKILL）；SIGTERM 钩子先 `store::flush_all()` 再停拨号器。
- 修正两处与实现不符的注释：`lan_ipv6` 实际是 `extendprefix=1`（不是 `ip6prefix`）；
  `wan_metric` 只写进本程序建的接口，不改厂商拨号器自己装的路由。
- 校验 `Makefile` 引用的 17 个仓库路径全部存在；四家拨号器源文件清单与
  `third_party/*/src` 实际文件逐项对得上（`fibocom-dial` 带 `libmnl` 与 PCIe 模式）。
- 配置键与 `rust/src/config/*` 逐项比对：示例配置里的键现在**全部**有对应字段。

### 33.5 代码风格与格式

- `cargo fmt` 全仓跑过一遍（此前从未格式化，384 处 diff）：纯格式变更，
  `cargo test` / `clippy` 全绿；行尾仍为 LF。
- 新增/改动的模块继续沿用「一接口一 handler」「纯策略 + 薄 IO + 编排」的分层，
  没有为了格式改动调整任何调用关系。

### 33.4 验证

- `cargo build` 零告警、`cargo clippy --all-targets` 零告警、
  `cargo test` **556 全绿**（新增路由表一致性用例）；
- aarch64 musl release 交叉编译通过，实机（FM350-GL）部署后服务正常、
  `modem0` up、`/api/host/nic` `required=true`、WAN 未动。

---

## 34. 仓库脱敏与守门脚本（2026-10-19）

仓库是 public 的，而调试记录里天然带着现场值。本轮做了一次全量脱敏，并把规则固化成脚本。

### 34.1 替换范围

| 类型 | 处理 |
| --- | --- |
| 上级路由 / 家庭网段 | 统一改成 `192.168.1.x` 示例段 |
| 模组侧 CGNAT | 统一改成 `10.42.0.x`（RFC 1918 示例段） |
| 运营商 DNS | 改成 `203.0.113.x`（RFC 5737 文档段） |
| 运营商公网 IPv6 前缀 | 改成 `2001:db8::/32`（RFC 3849 文档段） |
| 固件版本串 | 改成 `<固件版本>` 占位 |
| 点分 16 段的 v6 样本 | 按字节重编码成文档段的十进制形态 |

> 具体被替换的原值**不写进文档** —— 写出来等于又泄露一次。守门脚本里保留了
> 需要拦截的网段清单（那是规则，不是数据），文档只描述口径。

**测试样本成对替换**：AT 应答里的地址与断言里的期望值必须一起改，
否则测试会失败（本轮就踩到两处，逐个修好）。非法值/脏值用例也统一到示例段
（`10.42.3.256`、`2001::8::1`），不留真实前缀。

### 34.2 守门脚本

`scripts/check-desensitize.sh`（POSIX sh，无依赖）：

```sh
sh scripts/check-desensitize.sh   # 退出码 0 = 干净
```

- 扫 `*.rs / *.md / *.toml / *.sh / *.html / *.js / *.json / *.uc`；
- 跳过 `.git`、`rust/target`、`third_party`（上游原样代码）、`scripts`（自带规则）；
- 拦五类：历史实机网段、运营商 IPv6 前缀、非文档段公网 IPv4、凭据/穿透域名、真实手机号；
- 公网 IPv4 逐段判 `<=255`（避开 `999.1.1.1` 这类脏值误报），并排除点分 16 段的 IPv6 形态。

`docs/PRIVACY.md` 写明口径与「为什么不能放宽规则绕过」。

---

## 35. 四家拨号器 Rust 重构（2026-10-19）

外部二进制（`quectel-CM` / `fibocom-dial` / `meig-cm` / `simcom-cm`）此前只有
QMI 一条原生路径，MBIM 仍得靠外部程序。本轮把两层协议都内化，并按「厂商策略」
拆成四个模块。

### 35.1 结构

```text
runtime/dialer/
  mod.rs              监督器（分流：链路类型 + 厂商策略）
  proc.rs             外部二进制（保留）
  qmi.rs              QMI 线协议
  mbim.rs             MBIM 线协议（新增）
  native/
    mod.rs            计划 + 会话门面（QMI/MBIM 分派）
    qmi_link.rs       QMI 传输 + WDS 数据呼叫
    mbim_link.rs      MBIM 传输 + Basic Connect（新增）
    vendors/
      mod.rs          Caps + select()
      quectel.rs       QMI 优先 + MBIM 备选
      fibocom.rs      仅 QMI（无 mbim-cm.c）
      meig.rs         MBIM 优先（mbim-cm.c 主线）
      simcom.rs       仅 QMI（无 MBIM 实现）
```

**为什么按「数据面」而不是「AT 方言」拆**：四家的 `QMIThread.c` 与 `mbim-cm.c`
基本同源，真正的差异只有「支持哪条链路 / 优先级 / 几个开关」。协议实现共用一份，
厂商模块只声明 `Caps` —— 修一次协议四家同时受益，也不会出现「给 SIMCom 发 MBIM 命令」。

### 35.2 各家差异（皆有源码依据）

| 厂商 | 链路（首选在前） | 依据 |
| --- | --- | --- |
| Quectel | QMI / MBIM | `quectel-cm` 同时有 `QMIThread.c` 与 `mbim-cm.c`，QMI 是主线 |
| Fibocom | 仅 QMI | `fibocom-dial` **没有** `mbim-cm.c`；有 multi-pdn 与 PCIe 模式判断 |
| MeiG | MBIM / QMI | `meig-cm` 主打 `mbim-cm.c`，QMI 走 `meig-qmi-proxy` |
| SIMCom | 仅 QMI | `simcom-cm` 只有 QMI/GobiNet，参数最简 |

四家都声明 `disable_autoconnect = true`：`QmiWwanInit()` 里先发
`WDS Set Autoconnect(0)`，否则固件可能自己先建一条默认承载，我们的 Start 会被拒
（`result=1, error=0x11`）。

### 35.3 MBIM 实现

- 帧头 12 字节（type / length / transaction），`COMMAND_MSG` 头 48 字节
  （分片头 8 + UUID 16 + CID/type/len 12）；
- **`COMMAND_DONE` 的 body 从 `status` 起**（头 40 字节）—— 按 48 切会把 info 头 4 字节吞掉，
  `activation_state` 就会读到错位值。这条是真 bug，单测 `command_done_body_starts_at_status` 守住；
- `CONNECT` 固定头 **64 字节**（11 个 u32 + UUID 16 + NwError 4），早先按 56 算会让
  所有字符串 offset 偏 8 字节、设备回 `status=0xE`（invalid parameter）。单测
  `connect_set_offsets_are_relative_to_info_buffer` 逐字段断言；
- `IP_CONFIGURATION` 解析：可用性位图（地址/网关/DNS/MTU）+ offset 相对 info 起点；
- `SUBSCRIBER_READY` 失败不致命（部分固件上电早期拒），只记 note；
- 坏长度帧在 `take()` 里**同轮重同步**（丢 4 字节继续找），不把脏帧留给调用方。

### 35.4 分流与回退

`native_takes()` = 链路类型 ∈ {qmi, mbim} **且** 厂商策略支持该链路。所以：

- `host_dial_bin = "native"` + Fibocom 的 MBIM 模组 ⇒ **不交给原生**，继续用外部二进制
  （而不是让原生起一次再失败）；
- 不支持的组合在 `plan_native()` 里就报清楚（「fibocom 的原生后端不支持 mbim（只支持 qmi）：…」）；
- `/api/dial.host_dialer` 的 `backend` 现在是 `native-qmi` 或 `native-mbim`，并多一个 `vendor`。

### 35.5 验证

- 单测 **575 全绿**（本轮新增 19：MBIM 帧/命令/连接/地址解析 10 条、
  厂商策略 4 条、vendor 方言 2 条、模式解析 1 条、其余为替换后的既有用例）；
- `cargo build` 零告警、`cargo clippy --all-targets` 零告警；
- ⚠️ **原生 QMI/MBIM 仍未在真机验证**（测试机 FM350 是 RNDIS/ECM，没有 cdc-wdm）：
  线协议有 golden-bytes 与真实布局断言，数据呼叫需要 QMI/MBIM 模组。

---

## 36. 厂商方言移植：从 6 家到 14 家（2026-10-19）

上游 `module/` 里有 12 家 + 未知兜底；本仓库此前只有 6 家。本轮补齐 8 家，
每家的**私有指令族**都按源码逐条对齐，没有取证的项明确留空。

| 厂商 | 私有族 | 服务小区 / 网络 | 组网模式 | USB |
| --- | --- | --- | --- | --- |
| 华为 Huawei | `^` | `AT^SYSINFOEX` / `AT^HCSQ?` | `AT^SYSCFGEX` | `AT^SETMODE`（只读） |
| 有方 Neoway | `$` | `AT+NETDMSGEX` | `AT$MYSYSINFO` | 无 |
| Telit | `#` | `AT#CAINFOEXT?` / `AT#RFSTS` | `AT+WS46` | `AT#USBCFG`（只读） |
| Thales | `^` | `AT^DEBUG` | `AT^SLMODE` | `AT+SETCONFIG`（只读） |
| Sierra | `!` | `AT!GSTATUS?` | `AT!SELRAT` | `AT!USBCOMP`（只读） |
| 富士康 Foxconn | `!` + NV | `AT^DEBUG` | `AT!SLMODE` | `AT!USBSWITCH`（只读） |
| 高新兴 Gosuncn | `Z` | `AT+ZCELLINFO?` | `AT+ZSNT` | `AT+ZSWITCH`（只读） |
| 合宙 openluat | `*` | `AT+CCED=0,1` | `AT+CTEC` | `AT+SETUSB`（只读） |

### 36.1 三条硬规矩

1. **认错族 = 全回 ERROR**：每家的 `serving_cell_cmd` / `mode_read` 必须是自己那族，
   不能沿用移远的 `QENG`/`QNWPREFCFG`。`mode_read` 的默认值因此改成**空**（不发），
   由各家显式声明；`model/mode.rs` 见到空指令直接判「本机不支持读」。
2. **没取证就不给**：8 家的 USB 切换参数都没有逐档证据（切错会掉 USB），
   一律 `UsbnetWrite::Deny`，只开放查询；富士康的 `AT!nv=550` 只给模板并标「高危」
   （一次改两条 NV、没有回读确认）。
3. **小区字段解析不猜**：`NETDMSGEX`（字段随制式变）、`!GSTATUS?`（自由格式）、
   `^DEBUG`（手册 §15.33 分段）这些都没有移植解析器 —— 宁可 `/api/cells` 回空，
   也不把 WCDMA 的 RSCP 当成 RSRP 显示。

### 36.2 识别顺序

`DETECT_ORDER` 扩到 14 项，顺序有讲究：

- **TDTech 在 Huawei 之前**：`MT5700M` 是两家共线型号，项目原有归属是鼎桥
  （`AT^MONSC` / `AT^CHIPTEMP`），所以华为模块**不认** `MT5700M`；
- **Foxconn 在 Thales 之前**：两家都用 `AT^DEBUG`，靠 `T99W*` / `MV3*` 区分；
- 其余按「先具体后宽泛」排，`Openluat` 只认 `AIR724` / `AIR720` / `OPENLUAT`
  （`LUAT` 太宽，会误伤别的输出）。

### 36.3 模式解析

新增三个解析分支（带单测）：

- `huawei_acq`：`^SYSCFGEX: "00"/"03"` → `AUTO` / `LTE`，其余返回 `None`（不猜）；
- `ws46`：`+WS46: 22` → `LTE`、`28` → `LTE:NR5G`；
- `selrat`：`!SELRAT: 00/06` → `AUTO`、`14` → `LTE`。

---

## 37. NTP 时间同步（2026-10-19）

需求原文的三件事都落了：**支持设置时区 / 插件自己同步时间、同步后本地走时 /
一直失败回退读设备时间**。新增 `rust/src/runtime/ntp.rs`（模块本体 + 9 个单测）
与 `rust/src/config/ntp.rs`（`[ntp]` 段 + 4 个单测），新功能按约定全部用
Rust 独立模块实现。

### 37.1 为什么自带 SNTP 而不用系统 ntpd

路由器固件常把 `ntpd` / `chronyd` 裁了，或者只在 WAN 起来后才跑 —— 而本插件在
**开机后几秒**就要判定定时任务、写日志、算配额周期。自带一个最小 SNTP 客户端
（48 字节 UDP 报文，零依赖）不依赖系统服务是否在场。

### 37.2 校准语义

- **不改系统时钟**（`settimeofday` 要 root 且影响别的进程）：同步成功后把
  `offset = 服务器时间 − 系统时间` 写进 `logging::time::CLOCK_OFFSET`，
  `unix_now()`（全插件唯一时间源）立刻变准 —— 日志、定时任务、配额周期、
  波动记录一起受益，不用逐处改；
- **降级路径**：同步一直不成功偏移保持 0，时间自然回退为设备时间；
- **防自激**：算 offset 用的 T1/T4 取未校准的 `system_now()`，绝不用校准时间；
- **采样可信度**：originate 必须回显客户端 T1（防串包）、kiss-o'-death（stratum 0）
  拒收、往返时延 >5s 判为不可信。

### 37.3 节奏（与需求一一对应）

```text
启动 3s 后首试 ──成功──▶ 常态：每 20 分钟
      │失败
      ▼ 每 3s × 10 次（依次尝试全部服务器）
      ▼ 每 60s × 10 次
      ▼ 常驻：每 20 分钟（不再加速）
```

一个周期内依次尝试 `[ntp] servers`（默认 `ntp.tencent.com` /
`time.cloudflare.com` / `ntp.aliyun.com`），全部失败才算一次失败。

### 37.4 时区

`[ntp] timezone` 接受 IANA 名（`Asia/Shanghai`，内置常用表 + 半小时/45 分钟时区）
或 POSIX 写法（`CST-8`），优先于 `TZ` / `/etc/TZ`。三处一致：

- 启动时由 `spawn()` 应用（`set_configured_tz`，运行期可改的原子量）；
- 页面「改时区」（`POST /api/ntp/set`）解析成功即全局生效并写回 `config.toml`；
- 写不出的写法原样拒绝且不动现有时区。

刻意取舍：IANA 表只做**固定偏移**，不套 DST（要正确 DST 得内嵌 tzdata，体积
与维护成本都不划算；中国没有夏令时）。

### 37.5 接口与前端

- `GET /api/ntp`：状态（来源 / 服务器 / 偏移 / 阶段 / 计数 / 本地时间），与 WS
  `ntp` 消息同形；
- `POST /api/ntp/sync`：置强制标志立即返回，后台最迟 1 秒执行（请求不等应答，
  免得三台挨个超时挂住十几秒）；
- `POST /api/ntp/set`：改时区（运行期生效 + 写回配置）；
- 概览页新增「时间同步」卡片：本机时间 / 来源 / 偏移 / 阶段 / 成败计数 / 服务器，
  「立即同步」「改时区」两个动作，状态变化靠 `ntp` 推送实时刷新。

### 37.6 顺手修的两个测试暴露的 bug

- `iana_offset` 把名字里的下划线误换成斜杠 —— `Asia/Hong_Kong` 整类认不出；
- `query_addr` 的收发错误不带服务器名 —— 多服务器依次失败时日志对不上号。

---

## 12. 编码规范与要求（强约束）

1. **模块拆分**：宁细勿粗，一个接口一个 handler 文件；解析逻辑放 `model/*` 并配单测。
2. **零告警**：`cargo build` 必须无 warning（未用代码用 `#[allow(dead_code)]` 并注明原因）。
3. **单测**：新增解析逻辑必须带 `#[cfg(test)]` 用例，用例尽量取真实报文。
4. **注释**：中文；不写「与旧 Go 后端」「原 modem-pro」这类指向性描述；解释「为什么」而非「做了什么」。
5. **行尾**：`.sh/.py/.lua/.htm/.uc/.js/.json/.toml/.mk/.rs` 一律 **LF**（CRLF 会让
   BusyBox/procd 启动失败）；Windows 侧写文件须显式 LF。
6. **不破坏契约**：任何改动都不得要求修改 `rust/web/*.html` 的前端调用方式。
7. **依赖**：不假设库存在；新依赖必须落到 `rust/Cargo.toml` 并提交 `rust/Cargo.lock`。
   注意 `log` 必须带 `features=["std","alloc"]`。
8. **提交**：只有明确要求时才提交/推送；提交信息用中文，风格 `feat: …`；
   每次推送同步更新 `CHANGELOG.md`（必要时 `README.md`）。
9. **脱敏（硬约束，每次推送前扫一遍）**：仓库是 **public**，任何「只在这台设备上成立」
   的值都不得出现。当前口径：
   - **IMEI**：全仓库统一用示例值 `867698040012349`（Luhn 合法，历史已
     `git filter-repo` 重写强推）。实机 IMEI、`AT+CGSN` 的真实应答、截图里的
     IMEI 一律不得出现。
   - **ICCID / IMSI / 手机号**：同上，用占位值；示例报文里用 `123456789012345`
     这种一眼假的串。
   - **口令 / 令牌 / 私钥**：`bot_token`、`sms_token`、SSH 口令、任何
     `BEGIN … PRIVATE KEY` 都不得进仓库。配置里只留空串，实配值由用户在设备上填。
   - **内网地址**：路由器管理 IP 不写死；文档里用 `<路由器地址>` / `192.0.2.x`
     （TEST-NET-1）这类保留段。
   - **`/display/ws` 的固定 key**（`rust/src/http/ws.rs` 的 `DISPLAY_KEY`）
     是**已知待办**：它免密、硬编码、且在三份文档里明文记录。属 LAN 内展示通道，
     危害有限，但正确的做法是改成配置项 + 首次启动随机生成。**未擅动**（见 §26）。
   - 扫描方式：`git grep -n -I -E '<pattern>' -- .`，把上面几类各跑一遍。

---

## 13. 提示词要求（给后续模型 / 会话的 Prompt 规格）

> 用途：把下面的规格作为系统/任务提示词交给下一个模型，保证它不改口径、不跑偏。

### 12.1 通用系统提示词（每轮都带上）

```
你是本仓库（modem-pro-unified）的 Rust 后端工程师。目标：把原 Go 后端逐字对齐地迁移为 Rust，
前端 rust/web/*.html 一行都不改即可工作。

硬性要求：
- 中文回复；代码注释中文；注释里不得出现「与旧 Go 后端」「原 modem-pro」等指向性描述。
- 先读 docs/PROGRESS.md、docs/PORTING.md，再动手；每完成一个接口就更新 PORTING.md 勾选。
- 响应信封固定 {"code":i64,"data":any,"msg":string}，成功 code:0；业务失败默认 HTTP 200；
  所有登记接口必须有 handler（含 WebSocket 两个升级路径）；新增接口先登记 ROUTES 再实现，
  禁止 501 / 空实现；未登记 /api/* 返回 404。
- 参数惯例 ?modem_id=N（缺省 0）；一律 Cache-Control: no-store。
- 保留同端口网关（http/gateway.rs）+ WebSocket 升级（http/ws.rs）方案，不得改成两个端口。
- 一个接口一个 handler 文件；解析逻辑放 modem/model/* 并配单元测试。
- 必须：cargo build 零告警；cargo test 全绿；新增解析逻辑必须有 #[cfg(test)] 用例。
- 行尾一律 LF；不硬编码系统路径；Linux/Windows 双兼容。
- 不新增依赖除非必要；若新增须更新 Cargo.toml 并提交 Cargo.lock（log 依赖需 std+alloc）。
- 不修改前端调用方式；不写与本次任务无关的文档或重构。
- **脱敏**：仓库是 public，IMEI 用示例值 `867698040012349`；ICCID/IMSI/手机号用占位值；
  口令、令牌、私钥一律不进仓库；内网地址用 `<路由器地址>` 或 `192.0.2.x` 保留段。
  每次推送前用 `git grep -n -I -E '<pattern>' -- .` 把这几类各扫一遍（口径见 PROGRESS.md §12 第 9 条）。
- 未经明确要求不要 git commit / push。
```

### 12.2 单接口迁移任务提示词（模板）

```
任务：实现 <METHOD> <PATH>（<一句话说明>）。

步骤：
1. 在 docs/PORTING.md 找到该路由条目与前端 app 中对应的调用，确认字段名与类型。
2. 若涉及解析，先在 modem/model/ 下新建（或扩展现有）模块，写纯解析函数 + 单测；
   先用真实报文做用例，再写 AT 采集逻辑。
3. 新建 http/handlers/<name>.rs，走 response::ok / response::fail；从 query 取 modem_id。
4. 在 http/router.rs 的 ROUTES 与分发里接线。
5. cargo build（零告警）+ cargo test；用临时 config（require_luci_login=false）起服务，
   确认该接口返回 200 且信封正确。
6. 更新 docs/PORTING.md 状态；在 CHANGELOG.md「未发布」段追加一行。
交付：改动文件清单、测试结果、实机响应示例、PORTING.md 的勾选变化。
约束：遵守 §11 全部规范；不得引入与其它接口不一致的字段命名。
```

### 12.3 验收标准（DoD）

- 路由、方法、JSON 键与 `PORTING.md` 一致；每个登记项都有 handler，未登记路径 404。
- `cargo build` 零告警；`cargo test` 全绿；新增解析逻辑有单测。
- 实机（或临时 config 无模组）请求该接口返回 200 且信封正确。
- 文档同步：`PORTING.md` 勾选 + `CHANGELOG.md`。
- 前端文件零改动。

### 12.4 反面清单（出现即视为跑偏）

- ❌ 修改 `rust/web/*.html` 的接口调用方式，或新增前端字段以迁就后端。
- ❌ 把同端口网关拆成 HTTP / WS 两个端口；或让 `tiny_http` 直接处理升级。
- ❌ 登记过的接口返回 404 / 500 / 501（说明 handler 缺失或调用链断了）。
- ❌ 注释里出现「旧 Go 后端」「原 modem-pro」等指向性描述。
- ❌ 提交 `rust/target/`、`legacy-go/`、构建产物；缺失 `Cargo.lock`。
- ❌ 引入 CRLF；硬编码 `/dev/ttyUSB0`、`/etc/...` 等平台相关路径到核心逻辑里。
- ❌ 未经要求擅自 `git commit` / `git push`，或大范围无关重构。
- ❌ 用「猜测」替代「按量纲/按报文解析」，导致 AT 解析口径回退。

---

## 14. 关键链接

- 仓库：https://github.com/LianXia233/modem-pro-unified
- 接口清单：[`PORTING.md`](./PORTING.md) ｜ 交接表：[`HANDOFF.md`](./HANDOFF.md)
- 契约：[`API.md`](./API.md) ｜ 整合方案：[`INTEGRATION.md`](./INTEGRATION.md) ｜ 编译：[`BUILD.md`](./BUILD.md)

### 上游与致谢

本项目的方向、接口契约与真机调参经验来自以下上游工程，特此致谢：

- [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro) — 模组侧守护进程 `modemd`：
  多模组管理、厂商指令方言、锁网/小区解析、短信与拨号托管。本仓库的 HTTP/WS 接口、JSON 字段、
  AT 解析口径与配置项都以其为对齐基准。
- [ouyangzq/luci-proto-modem-pro](https://github.com/ouyangzq/luci-proto-modem-pro) —
  `modem_pro` netifd 协议：一张接口同时承载 IPv4 + IPv6 的 `mp-client.sh` wrapper 与 LuCI 协议表单，
  并给出 `accept_ra=2`、QMI 数据面、前缀漂移等实机注意事项。
- [LianXia233/luci-app-fm350](https://github.com/LianXia233/luci-app-fm350) — **350 系列模组**
  （广和通 FM350 / RW350）的真机参考：模组识别与 AT 方言、锁频/锁小区写法、IMEI 的 MFG 块备份与
  恢复等口径参考了该工程。

本仓库在其基础上做「整合为单仓库 + 后端 Rust 重写」，目标是与上述契约逐字兼容。
上游仓库未声明开源许可证，本仓库的 MIT 许可证仅覆盖本仓库自身的代码。
