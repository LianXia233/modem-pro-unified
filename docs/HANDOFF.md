# 进度交接表（HANDOFF）

本文件面向接手者：记录**目标、当前进度、剩余工作、硬性要求与验证方式**。
接口粒度的勾选清单见 [`PORTING.md`](./PORTING.md)，本表只做阶段级汇总。

- 交接日期：2026-10-18
- 当前分支：`main`
- 版本：见仓库根 `VERSION`（`1.0.0-1`）
- 状态：**只读域与写操作域均已收口**（地基完成；短信只读域 + 密码闸门、波动记录 + 采样历史、
  流量配额域、主动扫网、`netinfo`、USB/PCIe 视图已完成；写操作域已完成流量配额、短信、参数与开关、
  IMEI 写、重启/校时、锁网写、AT 直发域、拨号域、定时器域、**USB/PCIe/atport 绑定**；
  仅 `/api/host/nic/*`（前端已下线、保留）未迁移）

---

## 1. 目标与范围

把原本分离的 LuCI 前端插件与模组后端**整合为单一仓库、单一包 `modem-pro`**，
后端为 **Rust**（`rust/`），接口契约见 `docs/API.md`，做到：

1. 路由、方法、JSON 契约与原后端**逐字对齐**，前端 `rust/web/index.html` / `mini.html` **不改也能跑**；
2. 依赖关系、包名、版本号统一（单一来源 `VERSION`）；
3. OpenWrt SDK / feeds 下可编译、可安装；拨号全部由 `modemd` 内置实现，无外部二进制。

> 迁移参照副本仅作**本地参考**，已从版本控制移除、不参与构建，也不推送到远端
> （见第 6 节「交接注意事项」）。
>
> 来源与致谢见 `README.md`「致谢」与 `PROGRESS.md` §13（
> [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro)、
> [ouyangzq/luci-proto-modem-pro](https://github.com/ouyangzq/luci-proto-modem-pro)）。

---

## 2. 阶段进度

| # | 阶段 | 状态 | 说明 |
| --- | --- | --- | --- |
| 1 | 地基 | ✅ 完成 | 自有同端口网关（HTTP 透传 + WebSocket 升级）、路由总表 `ROUTES`、推送中枢 `WsHub`、静态页（`/`、`/ui`、`/mini`）、鉴权闸门 |
| 2 | 只读域 | ✅ 完成 | 见下表；短信域、波动记录、采样历史、流量配额、主动扫网、`netinfo`、USB/PCIe 视图均已完成 |
| 3 | 写操作域 | ✅ 完成 | 已完成：流量配额、短信、参数与开关（`params/apn|pdp|netselect`、`mode/set`、`usbnet/set`、`airplane/set`、`roam/set`）、`imei(set/restore)`、`modem(reset/clock)`、`lock(band/cell/mode/unlock)`、`at`(发/port/presets)、`changes/clear`、`dial`(connect/disconnect/enable/mode/ipv6 + GET)、`schedule`(GET + set/delete/toggle/run)、usb/pcie/atport 绑定 |
| 4 | WebSocket 推送 | ✅ 完成 | 握手 + `snapshot` + 各面板 `push_*` 全部接线；写操作成功后顺手广播（cells / lock / sms+smscount / quota / traffic / ussd）；短信推送过密码闸门（带令牌才收 `sms`，`smscount` 不过闸门） |
| 5 | 后台循环 | ✅ 完成 | push / traffic / quota / change / hotplug / sim / logs / **urc** / **netprov** 九类循环已接线（SIM 巡检 2s 一拍、到点或 URC 置脏才发 AT；换卡清短信缓存并补推空 `sms`+`smscount`；**urc 循环**消费 `at/urc.rs` 暂存的主动上报，折算成「SIM / 网络 / 短信置脏」并把巡检加速到下一拍，`+CUSD` 直接推页面、`+CMTI` 触发一次短信重读，见 `PROGRESS.md` §21 / §22；**netprov 自动组网**见 §23、**企业微信推送**见 §24） |
| 6 | 内置拨号 | ✅ 已完成（代码）/ ⚠️ 真机待验证 | `hostdial`：QMI/MBIM 数据呼叫由 `modemd` 内置实现（含 QMAP 绑 mux）；`/api/dial/connect` 对这两类从「拒绝」改为「委托」（见 `PROGRESS.md` §25）。**QMI/MBIM 真机均未验证**：测试机为 FM350（RNDIS，无控制口） |

### 2.1 只读域明细

| 接口 | 状态 | 备注 |
| --- | --- | --- |
| `/api/version`、`/api/session`、`/api/login`、`/api/logout` | ✅ | |
| `/api/modems` | ✅ | `?discover=1` 触发重探 |
| `/api/modem/info`、`/api/modem/status`、`/api/sim`、`/api/vendor/cmds` | ✅ | |
| `/api/signal` | ✅ | `CSQ` + `QENG`/`CESQ`（含广和通 SSB 扩展） |
| `/api/network` | ✅ | 基础注册状态 |
| `/api/network/status` | 🚧 | plmn/tech/band/apn/ipv4/cs·ps_reg/roaming 已有；traffic/ca/serving/ambr 待补 |
| `/api/cells`、`/api/cells/all` | ✅ | `+QENG`/`+FENG`、芯讯通 `+CPSI` |
| `/api/lock/status`、`/api/lock/caps` | 🚧 | 方言覆盖 QNW 与芯讯通；其余厂商方言待补 |
| `/api/mode`、`/api/usbnet`、`/api/imei`、`/api/airplane`、`/api/roam` | ✅ | 只读（写侧见 2.2：`mode/set`、`usbnet/set`、`imei/set|restore`、`airplane/set`、`roam/set` 均已完成） |
| `/api/params` | ✅ | 读 + 写（`apn` / `apn/delete` / `pdp` / `netselect`）均已完成 |
| `/api/ussd` | ✅ | GET 读最近一次应答 `{modem_id, result}`（可为 null）；POST 下发（校验字符集 + `AT+CPIN?` 判 PIN 锁）；`+CUSD` 应答由 URC 消费循环自动推送（同步到达的那条由下发接口当场收下并推，两条路不重复） |
| `/api/logs`、`/api/modules`、`/api/at`、`/api/at/presets` | ✅ | `at` 的 `resp` 为应答行数组、失败也回 `data:{cmd,resp}`；`presets` 按厂商合并清单（无模组回 code 0 兜底）；`modules` 的 `?model=`/`?modem_id=` 待补 |
| `/api/sms/auth`、`/api/sms/unlock`、`/api/sms/lock`、`/api/sms/read`、`/api/sms/storage`(GET) | ✅ | `model/sms.rs` PDU/文本解码 + `http/smsauth.rs` 密码闸门 |
| `/api/sms/send`、`/api/sms/delete`、`/api/sms/password`、`/api/sms/storage`(POST) | ✅ | 文本模式 `AT+CMGS`（UCS2 + 分段）/ `AT+CMGD` / `AT+CPMS`；写回 config.toml 走 `config/patch.rs`（行级替换，注释保留） |
| `/api/signal/history`、`/api/changes`、`/api/changes/clear` | ✅ | `modem/metrics.rs`：波动记录 + 采样历史（进程内环形缓冲）；记录点挂在已有查询上，不额外发 AT |
| `/api/quota`、`/api/quota/set`、`/api/quota/reset`、`/api/quota/unblock`、`/api/traffic/reset` | ✅ | `http/quota.rs` + `runtime/store.rs`（`quota.json`）+ `model/traffic.rs` / `model/quota.rs` |
| `/api/netscan`、`/api/netscan/stop` | ✅ | `model/netscan.rs`：`AT+COPS=?`（可中断）+ 芯讯通 `AT+CNWSEARCH` |
| `/api/imei/set`、`/api/imei/restore` | ✅ | `model/imei.rs`：先备份原始值到 `imei.json` → `AT+EGMR=1,7,"…"` → `AT+CGSN` 回读确认；仅移远/美格可写 |
| `/api/modem/reset`、`/api/modem/clock` | ✅ | `model/device.rs`：`AT+CFUN=1,1`（等不到确认按已下发上报）/ `AT+CCLK` 校时（带回退），返回整份模组状态 |
| `/api/lock/band`、`/api/lock/cell`、`/api/lock/mode`、`/api/lock/unlock` | ✅ | `model/lock.rs` 写侧：QNW 频段/小区下发 + 回读确认；**解除频段不写 0**（读 `policy_band` 全量写回）；芯讯通小区锁定与两种解锁写法未验证、明确拒绝 |
| `/api/at`、`/api/at/port` | ✅ | `model/at/presets.rs` 清单 + `atport.json` 台账（身份键：`imei:` 优先）；`resp` 行数组口径；`at/port` 探活后切口 / 恢复自动探测 |
| `/api/dial`（GET + connect/disconnect/enable/mode/ipv6） | ✅ | `model/dial.rs`：USB 类型三路识别（AT/描述符/主机驱动，30s 缓存）；AT 拨号（ECM/RNDIS/NCM 等）；QMI/MBIM **不发** `AT+CGACT`（会占掉拨号器的唯一数据承载），改为把 APN 落到状态、由下一轮组网循环拉起 modemd 托管的厂商拨号器（`runtime/dialer/`，15s 一轮，MBIM 无 APN 仍报错）；失败也回 `DialState`；开关写回 `[net] dial_enabled|wan_ipv6|wan_mode`（`config/wan_mode.rs`）；`auto_wan` 返回真实报告（netprov 循环 15s 一轮，见 `netprov/`） |
| `/api/schedule`（GET + set/delete/toggle/run） | ✅ | `model/schedule.rs` + `runtime/store.rs`（`schedule.json`）+ `http/schedule.rs`；本机时区判定、15 分钟补做窗口、槽位去重；执行复用锁网/模式/飞行模式实现 + 能力预检 |
| `/api/netinfo` | ✅ | `model/network.rs::net_info`：按厂商发 `AT+QNWINFO`（TDTech `AT^SYSINFOEX`；Unisoc 回空），原始行 `|` 连接 |
| `/api/usb`、`/api/usb/clear`、`/api/usb/bind` | ✅ | `modem/usbdev.rs`：已知身份表 + 事件环形缓冲；视图含 `platform/auto_bind/can_bind/bind_note/devices/events/advice`；手动补 usb-serial 驱动（`new_id`） |
| `/api/pcie`、`/api/pcie/reset`、`/api/pcie/bind` | ✅ | `modem/pcidev.rs`：模组驱动/设备判定、AT/QMI/MBIM 候选口；remove+rescan 复位；绑驱动（driver 空则挨个试）；`catalog_models` 字段暂缺 |
| `/api/atport/bind`（GET/POST） | ✅ | `AtPortBindStore`（`atport_bind.json`）：按 `vid:pid` 钉死命令口；`apply_saved_at_port` 先按 vid:pid 再按模组身份；非 unix `supported=false` |

仅 `/api/host/nic/*` 未迁移（前端已下线、保留；`/api/wechat/*` 已完成，见 `PROGRESS.md` §24）。未迁移接口当前统一返回
**所有登记接口都有 handler**（`stub.rs` 已删除）；未登记的 `/api/*` 返回 404。

---

## 3. 仓库结构

```
Makefile            顶层单包构建（含 rust-package.mk 接入、Cargo.lock 锁定；
                    包内 config 只剩 MODEM_PRO_HOST_RUST）
VERSION             版本单一来源；打包注入 MODEMD_VERSION
rust/               Rust 后端（modemd）：http / modem / config / logging / ubus / runtime
  └ web/            静态调试台（index.html / mini.html，编译期嵌入）
luci/               LuCI 前端：ucode/JS 世代（JS 视图 / ucode 控制器 / menu.d / ACL / 协议 JS）
files/              安装到设备的文件：netifd 协议、mp-client 等
packaging/          init.d / control 脚本
docs/               INTEGRATION / BUILD / API / PORTING / PROGRESS / HANDOFF（本文件）
```

---

## 4. 硬性要求（迁移时必须保留）

以下为真机经验所得，**不得**因重构而丢失（与 `PORTING.md` 末节一致，此处列出重点）：

1. **响应契约**：`{"code":i64,"data":any,"msg":string}`，成功 `code:0`；业务失败默认 HTTP 200；
   鉴权失败 HTTP 401；未知接口 404；一律 `Cache-Control: no-store`。
2. **鉴权**：cookie `modemd_sess`（跟随 LuCI 登录），放行静态页与
   `/api/version|session|login|logout`；loopback 免登录；ubus 不可用则放行并记 warn。
   WS 侧同款闸门，另有短信令牌 `sms_token`；`/display/ws` 免密固定 key `h29k-tft-9f3c1a7b`。
3. **同端口 WebSocket**：前端硬编码 `location.host + /ws`，而 `tiny_http` 不支持协议升级 ⇒
   必须保留自建 TCP 网关（`http/gateway.rs`）+ 升级处理（`http/ws.rs`）。
4. **命名**：包名 `modem-pro`；二进制 `/usr/bin/modemd`（QMI/MBIM 内置拨号，
   不再有 `/usr/lib/modemd/` 下的厂商二进制）；netifd 协议名 **`modem_pro`**
   （连字符在 shell 函数名非法）。
5. **配置**：`/etc/modemd/config.toml`（conffile）。
6. **构建**：`feeds/packages/lang/rust/rust-package.mk`；`RUST_PKG_LOCKED` 默认 1 ⇒ **必须提交
   `rust/Cargo.lock`**；`log` 依赖须带 `features=["std","alloc"]`。
7. **行尾**：`.sh/.lua/.htm/.uc/.js/.json/.toml/.mk/.rs` 一律 **LF**（CRLF 会导致 BusyBox/procd 启动失败）；
   `.gitattributes` 已强制。
8. **真机口径**：QMAP 模组必须发 WDA `Set Data Format` 并 `Bind Mux`（否则数据口永远
   NO-CARRIER）；`AT+CMGL` 有「标已读」副作用；
   `/api/sms/storage` 同路径 GET 读 / POST 切；一个 AT 口 = 一个模组；
   AT 解析按量纲定位（`+CESQ` 换算、`+QENG` NR/LTE 下标、Fibocom SSB 扩展等）不得回退。
9. **短信密码闸门**：`require_luci_login` 开启 / `sms_password_required=false` / `sms_password`
   为空串时**不需要**密码；令牌 TTL 30 分钟、最多 32 个、连错 5 次锁 60 秒、默认密码 `password`；
   令牌优先 `X-SMS-Token` 头、其次 query `sms_token`；未解锁访问返回 HTTP 401 + `locked`。
10. **前端不动**：任何改动不得要求修改 `rust/web/*.html` 的接口调用方式。
11. **流量配额**：用量按差分/积分累计（首次采样只建基线），台账写**配置文件同目录**的
    `quota.json`（重启不丢），config.toml 只给默认值；超限时**只停上网用的 PDP 上下文**
    （跳过 ims/sos/mms），`block_rf` 必须先查 `AT+CFUN?` 能力。
12. **主动扫网必须可中断**：`AT+COPS=?` 会独占串口几分钟，`/api/netscan/stop` 要能随时打断，
    中断后已收到的部分照常解析并标 `stopped`。
13. **写回 config.toml 只能行级替换**：配置文件的注释比键还多，序列化写回会把真机注释冲掉；
    用 `config/patch.rs`，改前备份 `.bak`。改密码后要同步 `SmsAuth` 的运行期覆盖值（免得要重启）。
    ⚠️ **字符串值必须走 `set_string`**（自动加引号并转义），`set_raw` 只给 `true`/`false`/数字：
    误用一次就把配置写成非法 TOML，服务从此起不来（真机事故，见 `PROGRESS.md` §5.20）。
    配套兜底：`config/loader.rs` 解析失败时回落 `.bak` → 内置默认值，不再一坏就退出。
14. **LuCI 只有 JS + ucode 世代**（2026-10-18 起，`luci/app/lua/**` 已删除）：
    - ucode dispatcher 只扫 `menu.d/*.json` 与 `luci/controller/*.lua`；ucode 控制器
      **必须在 menu.d 注册 `type:function` 节点**才能分发，否则端点 404；
    - ucode **没有 `typeof`**，只有 `type()`；写错 = `require` 时 Syntax error，整个 500；
    - `rpc.declare` 的 `expect` 是**单键选择器**（`ret = ret[第一个键]`，取不到就用默认值顶上），
      不是校验器；`luci.getInitList`/`service.list` 返回按名字建键的对象，**不要写 expect**；
    - 运行态只来自 procd `service list`（`luci.getInitList` 只给 `enabled`）；
    - 菜单标题保持英文（无 `po2lmo` 造不出 `.lmo`），界面文案由 JS 视图 `T()` 翻；
    - i18n **每个视图内联一份**（`modemdLang()` + `I18N` + `T()`），刻意不做共享模块。

---

## 5. 环境与验证

- 开发机：Windows，`pwsh`；工具：`git`、`cargo`/`rustc`；
  未安装 `rg`，检索请用编辑器/Grep 工具。
- 构建与测试：
```
cargo build          # 需零告警
cargo test           # 当前 575 项全通过
```
- 运行自测（无模组也可验证路由接线）：
  ```
  modemd.exe -config <临时 config.toml>
  # 例：listen=127.0.0.1:19527, [ui] require_luci_login=false
  # 逐个 curl 各 /api/*，登记接口应 200；未登记的路径应 404（没有 501 占位了）
  # 短信接口在未解锁时应返回 401 + {"locked":true,"required":true}
  ```
- 打包验证：见 `docs/BUILD.md`（SDK 编译 + 安装 + init 启动）。

### 5.1 完成定义（DoD）

- 接口：路由、方法、JSON 键与 `PORTING.md` 清单一致；新增接口先登记 `ROUTES` 再写 handler，禁止占位实现。
- 代码：`cargo build` 零告警、`cargo test` 全绿、单测覆盖新增解析逻辑。
- 文档：每完成一批接口，同步更新 `PORTING.md` 勾选与 `CHANGELOG.md`。
- 每次推送：按仓库约定同步更新 `CHANGELOG.md` / `README.md`。

---

## 6. 交接注意事项

1. **本地参考副本不在版本控制内**（已从版本控制移除并写入 `.gitignore`）：
   它**仅供迁移时对照**，既不构建也不推送。
   接手者若需要，应自行获取原始实现。
2. 代码注释中**不写**「与旧 Go 后端」「原 modem-pro」等指向性描述；对照关系集中在
   `docs/PORTING.md` 与本文件。
3. 未提交改动若存在，请先 `git status` 复核再提交；提交信息沿用
   `feat: 整合为单一包 modem-pro（…）` 的中文风格。
4. 下一步建议顺序：~~`quota(读+写)`~~、~~`netscan(+stop)`~~、~~短信写操作域~~、
   ~~参数与开关写操作域~~、~~`imei` 写操作域（set/restore）~~、~~重启/校时~~、
   ~~锁网写操作域（band/cell/mode/unlock）~~、~~`at` 发与响应字段对齐 + `at/port` + `at/presets`~~、
   ~~`dial` 域（connect/disconnect/enable/mode/ipv6 + GET）~~、~~`schedule` 域（GET + set/delete/toggle/run，
   含后台循环接线）~~、~~`usb/pcie/atport` 绑定~~、~~**阶段 4 全量推送**（写操作成功即广播 +
   短信闸门过滤）~~ **已完成**；
   ~~再补阶段 5 后台循环剩余部分（屏端 `/display/ws` 动作协议、插卡后重读短信）~~
   **已完成**（见 `PROGRESS.md` §27，实机验证过）；
   ~~`/api/host/nic`（主机网卡）~~ **已完成**（见 `PROGRESS.md` §28，实机验证）；
   ~~350 系列组网（数据口选择 / 匿名 zone / static）~~ **已完成并实机验证**（见 `PROGRESS.md` §29，
   模组接口 `eth2` + static 地址，`ping -I eth2` 通；WAN 未动）；
   ~~350 系列 IPv6 取址（`[net] wan_ipv6_mode`：dhcpv6 / ra / static / off，对齐 `luci-app-fm350`）~~
   **已完成并实机验证**（见 §30；dhcpv6 形态 + RA 的 sysctl/RS + static 残值回退）；
   ~~厂商模块拆分（`modem/vendor/<厂商>.rs` + `VendorSpec` 统一接口）~~
   **已完成**（见 §31.1，调用方只问 `vendor.spec()`）；
   ~~Rust 原生 QMI 拨号器（`[net] host_dial_bin = "native"`）~~
   **已完成**（见 §31.2；线协议有单测，**QMI 真机未验证**）；
   ~~四家拨号器 Rust 重构（QMI + MBIM 双协议 + `native/vendors/` 四模块）~~
   **已完成**（见 §35；MBIM 线协议有真实布局单测，**MBIM 真机未验证**）；
   ~~彻底去 C 依赖（删除 `third_party/`、默认 `host_dial_bin=native`、补 QMAP/MHI/GobiNet）~~
   **已完成**（见 §41；**真机验证仍是唯一缺口**）；
   ~~厂商方言 6 家 → 14 家~~ **已完成**（见 §36；8 家新增，未取证的项明确留空）；
   ~~仓库脱敏 + 守门脚本~~ **已完成**（见 §34 与 `docs/PRIVACY.md`）；
   ~~NTP 时间同步（自带 SNTP + 时区 + 三段退避 + `/api/ntp*` + 概览页卡片）~~
   **已完成**（见 §37；协议与退避逻辑有单测，**真机待验证**：路由器能否出网到
   三台 NTP 服务器、防火墙是否放行 UDP 123 出站）；
   剩下的是阶段 6 拨号器托管的真机验证（设备没装拨号器二进制）。
   （**URC 巡检加速与屏端推送已完成**：`at/urc.rs` 暂存的主动上报由 `spawn_urc` 消费，
   折算成「SIM / 网络 / 短信置脏」，`+CUSD` 直接推页面、`+CMTI` 触发一次短信重读，
   见 `PROGRESS.md` §21 / §22。）
   （**自动组网 netprov、企业微信推送、拨号器托管均已完成**：见 `PROGRESS.md` §23 / §24 / §25。）
   待真机校正：`+CNWSEARCH` 的字段量纲、速率式积分的实际偏差、`AT+EGMR` 在不同高通系
   模组上的实际应答（美格尚未实机验证）、`AT+CCLK` 校时后各厂商的 `clock_skew_s` 表现、
   芯讯通 `AT+CSYSSEL` 频段写的分隔符与解锁写法（当前拒绝盲写）、
   **鼎桥 `AT^SYSCFGEX` 与展锐 `AT+SP5GRAN` 组网模式的写路径**（代码已实现、未经真机）、
   拨号域在 ECM/RNDIS/NCM 真机上的完整链路（虚拟串口已验证，尚缺实机）、
   **内置 QMI/MBIM 拨号在真机上的数据面**（QMAP / 非 QMAP / MBIM / PCIe-MHI 四种形态，
   控制口反查与 WDA 绑 mux 无实机；线协议只有 golden-bytes 单测）、
   ~~定时任务在真机上的到点执行~~ **已验证**（2026-10-19，FM350-GL：本地 00:57 的任务
   在下一拍下发并记录 `last.ok=true`，槽位去重生效）；`AT+QNWLOCK="save_ctrl"` 写 NV 仍缺实机、
   USB 热插拔补驱动与 PCIe 复位/绑驱动在真机上的表现（sysfs 逻辑已实现，尚缺实机）。
