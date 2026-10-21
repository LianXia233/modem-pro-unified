# HTTP / WebSocket 接口契约

> 本文由 `rust/src/http/router.rs` 的 `ROUTES` 表 + 分发 `match` 臂**逐条导出**，
> 与代码一一对应；新增/删除接口必须同步改这里（`ROUTES` 有测试校验无重复项）。
>
> 最后核对：2026-10-20（`main`）：103 条 `/api/*` + 2 个 WebSocket，共 105 条。

## 1. 通用约定

### 1.1 响应信封

```json
{"code": 0, "data": null, "msg": "ok"}
```

- `code == 0` 成功，非 0 失败；HTTP 状态码通常仍是 **200**（与前端既有契约一致）；
- 鉴权失败用 401，并带 `code` 与 `msg`；
- 失败时 `data` 可能为 `null`，也可能带状态快照（拨号失败带 `DialState`，短信闸门带 `{"locked":true}`）。

### 1.2 鉴权

`[ui] require_luci_login`（默认开）打开时，除放行项外都要带 LuCI 会话 cookie
`modemd_sess`，有效性由 rpcd（ubus `session.get`）判定：

- 放行：静态资源、`/api/version`、`/api/session`、`/api/login`、`/api/logout`、本机（127.0.0.1）请求；
- ubus 不可用（非 OpenWrt / rpcd 未起）时**放行并记日志**，避免把整机用户挡在门外。

### 1.3 模组选择

多数接口接受 `modem_id`（query 或表单）。**不给**时回落到第一台实际存在的模组；
**给了但不存在**时返回错误、不回落（写操作错口比失败更危险）。

### 1.4 参数来源

`GET` 从 query string 取；`POST` 同时接受 query string 与表单/JSON body（`merge_form`）。
各接口字段见对应 handler 文件顶部 `//!` 说明。

## 2. 接口全表

### `/api/version`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/version` | `version::handle` |  |

### `/api/snapshot`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/snapshot` | `snapshot::handle` | 一个请求拿齐「概览要显示的东西」，而且**一条 AT 都不发** |

### `/api/tasks`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/tasks` | `tasks::handle` | 异步任务接口：`/api/tasks`（提交 / 查询 / 取消） |
| POST | `/api/tasks` | `tasks::handle` | 异步任务接口：`/api/tasks`（提交 / 查询 / 取消） |
| GET | `/api/tasks/get` | `tasks::handle` | 异步任务接口：`/api/tasks`（提交 / 查询 / 取消） |
| POST | `/api/tasks/cancel` | `tasks::handle` | 异步任务接口：`/api/tasks`（提交 / 查询 / 取消） |

### `/api/login`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| POST | `/api/login` | `session::login` | 会话相关：`/api/session`、`/api/login`、`/api/logout` |

### `/api/logout`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| POST | `/api/logout` | `session::logout` | 会话相关：`/api/session`、`/api/login`、`/api/logout` |

### `/api/session`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/session` | `session::session` | 会话相关：`/api/session`、`/api/login`、`/api/logout` |

### `/api/modems`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/modems` | `modems::handle` |  |

### `/api/modem`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/modem/info` | `modem_info::handle` |  |
| GET | `/api/modem/status` | `modem_status::handle` |  |
| POST | `/api/modem/clock` | `modem_clock::handle` | 返回体与 `GET /api/modem/status` 同构（前端拿它直接刷新 SIM/温度/时钟卡片）， |
| POST | `/api/modem/reset` | `modem_reset::handle` |  |

### `/api/sim`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/sim` | `sim::handle` |  |

### `/api/vendor`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/vendor/cmds` | `vendor_cmds::handle` |  |

### `/api/signal`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/signal` | `signal::handle` | 查询结果顺手喂给波动记录与采样历史（见 `modem/metrics.rs`），不额外发 AT |
| GET | `/api/signal/history` | `signal_history::handle` | 时间升序，供前端画曲线。空值字段缺省表示该次未上报，前端据此断开曲线， |

### `/api/at`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/at/sched` | `at_sched::handle` | ## 这个接口回答什么问题 |
| GET | `/api/at` | `at::handle` | · 只允许**单条**指令（换行会拼出多条指令，容易误改模组状态） |
| POST | `/api/at` | `at::handle` | · 只允许**单条**指令（换行会拼出多条指令，容易误改模组状态） |
| POST | `/api/at/port` | `at_port::handle` | · 键用「模组身份」（IMEI 优先，其次型号；未识别时用当前口自己当兜底键）， |
| GET | `/api/at/presets` | `at_presets::handle` | · 厂商还没识别出来时给纯 3GPP 那一份 —— 宁可少，也不要给用户看别家的私有指令 |

### `/api/traffic`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/traffic` | `traffic::handle` | ## 数据来源 |
| POST | `/api/traffic/reset` | `traffic_reset::handle` | 清的是**模组那一侧**的累计（`AT+QGDCNT=0`）；速率式机型没有可清的计数器， |

### `/api/timezone`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/timezone` | `timezone::get` | ## 与 `/api/ntp/set` 的分工（两个不同的东西，别混） |
| POST | `/api/timezone` | `timezone::get` | ## 与 `/api/ntp/set` 的分工（两个不同的东西，别混） |

### `/api/network`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/network` | `network::handle` |  |
| GET | `/api/network/status` | `network_status::handle` | 查询结果顺手喂给波动记录（见 `modem/metrics.rs`），不额外发 AT |

### `/api/netinfo`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/netinfo` | `netinfo::handle` |  |

### `/api/airplane`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/airplane` | `airplane::handle` |  |
| POST | `/api/airplane/set` | `airplane_set::handle` | 不支持 `AT+CFUN?` 的机型（展锐 SIPC 那一族）会被拒绝：关射频会把设备自身的管理通道一起断掉 |

### `/api/roam`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/roam` | `roam::handle` |  |
| POST | `/api/roam/set` | `roam_set::handle` | 这是个**策略开关**（不是给模组下 AT）：写回 `config.toml` 的 `[net] allow_roaming`， |

### `/api/cells`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/cells` | `cells::handle` | 扫描完成后顺手记录「服务小区 / 邻区数量」的波动（见 `modem/metrics.rs`） |
| GET | `/api/cells/all` | `cells_all::handle` |  |

### `/api/netscan`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/netscan` | `netscan::handle` | 慢接口：最久要等 `minutes` 分钟（夹到 1~10），期间独占该模组的 AT 口 |
| POST | `/api/netscan/stop` | `netscan_stop::handle` | 只置标志、立即返回：真正的结果由那个还没返回的 `/api/netscan` 渲染 |

### `/api/lock`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/lock/status` | `lock_status::handle` |  |
| GET | `/api/lock/caps` | `lock_caps::handle` |  |
| POST | `/api/lock/band` | `lock_band::handle` | 请求体：`{modem_id, tech, bands: [78], unlock?}` |
| POST | `/api/lock/cell` | `lock_cell::handle` | 请求体：`{modem_id, tech, lock_type, earfcn, pcid?, scs?, band?}`。`scs`/`band` 仅 NR 用；`band` 留空时服务端用缓存驻网频段兜底；NR 只接受 `lock_type=1`（5G 锁小区语法没有「仅频点」档位） |
| POST | `/api/lock/mode` | `lock_mode::handle` | 请求体：`{modem_id, mode}`。返回最新锁网状态（同 `GET /api/lock/status`） |
| POST | `/api/lock/unlock` | `lock_unlock::handle` | 请求体：`{modem_id}`。返回最新锁网状态（同 `GET /api/lock/status`） |

### `/api/mode`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/mode` | `mode::handle` |  |
| POST | `/api/mode/set` | `mode_set::handle` |  |

### `/api/dial`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/dial` | `dial::handle` | 页面进「拨号上网」先 GET 一次（状态 + 类型 + 表，一次拉全，不用发三次请求） |
| POST | `/api/dial/enable` | `dial_enable::handle` | 这是个**策略开关**（不是给模组下 AT）：写回 `config.toml` 的 `[net] dial_enabled`， |
| POST | `/api/dial/auto` | `dial_auto::handle` | 写回 `config.toml` 的 `[net] auto_dial`（fm350 同款语义、默认 true），并同步 |
| POST | `/api/dial/mode` | `dial_mode::handle` | 三种模式（`[net] wan_mode`） |
| POST | `/api/dial/ipv6` | `dial_ipv6::handle` | 这是个**策略开关**（不是给模组下 AT）：写回 `config.toml` 的 `[net] wan_ipv6`， |
| POST | `/api/dial/connect` | `dial_connect::handle` | apn 留空 = 沿用模组里已有的配置；reconnect = 已激活的先断开再连（让刚写入的 APN 生效） |
| POST | `/api/dial/disconnect` | `dial_disconnect::handle` | cid 省略（或 0）时用当前拨号的 cid，再不行用数据默认 cid |

`DialState`（`/api/dial` 的 `state` 字段、任务 `dial.connect` 的结果）新增可选字段
`backend`：`at` 表示芯片侧 AT 配方，`native-qmi` / `native-mbim` 表示主机侧内置拨号器，
空串表示没连过或链路不是本程序拨的。**新增字段、旧前端可忽略**；其余字段与语义未变。
生命周期（谁决定连、何时重连、退避）见 `docs/DIAL_STATE_MACHINE.md`。

`GET /api/dial` 另有新增字段 `wan_dns`（字符串，空串 = 不强制下发）：本次组网实际会
写进 `network.modem<id>.dns` 的那串地址 —— `[net] wan_dns` 经「IPv6 开关」裁剪后的结果
（`wan_ipv6=false` 时只留 IPv4 两条）。页面拿它显示「接口下发 DNS」，排障不必翻配置文件。

### `/api/usbnet`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/usbnet` | `usbnet::handle` |  |
| POST | `/api/usbnet/set` | `usbnet_set::handle` | 只有移远 / 美格的 `AT+QCFG="usbnet"` 可写；广和通 RNDIS 不允许执行，其余厂商只能看 |

### `/api/sms`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/sms/read` | `sms_read::handle` |  |
| POST | `/api/sms/send` | `sms_send::handle` | 短信密码已取消（2026-10-19）；中文 / emoji 自动走 UCS2， |
| POST | `/api/sms/delete` | `sms_delete::handle` | 短信密码已取消（2026-10-19）；删除是**不可恢复**的，所以参数要卡死：只认数字位置 |
| GET | `/api/sms/storage` | `sms_storage::handle` | 切换存储区是 `POST /api/sms/storage`（写操作域） |
| POST | `/api/sms/storage` | `sms_storage::handle` | 切换存储区是 `POST /api/sms/storage`（写操作域） |

### `/api/wechat`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/wechat` | `wechat::handle` | \| 接口 \| 作用 \| |
| POST | `/api/wechat/login` | `wechat::login` | \| 接口 \| 作用 \| |
| POST | `/api/wechat/verify` | `wechat::verify` | \| 接口 \| 作用 \| |
| POST | `/api/wechat/set` | `wechat::set` | \| 接口 \| 作用 \| |
| POST | `/api/wechat/test` | `wechat::test` | \| 接口 \| 作用 \| |
| POST | `/api/wechat/logout` | `wechat::logout` | \| 接口 \| 作用 \| |

### `/api/params`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/params` | `params::handle` |  |
| POST | `/api/params/apn` | `params_apn::handle` | 写完回读确认 + 自动重新激活该上下文 + 顺手激活 IMS；每一步的结果放在 `msg` 里 |
| POST | `/api/params/apn/delete` | `params_apn_delete::handle` |  |
| POST | `/api/params/pdp` | `params_pdp::handle` | 内部上下文（IMS），被拒时跳过断开直接激活 |
| POST | `/api/params/netselect` | `params_netselect::handle` |  |

### `/api/quota`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/quota` | `quota::handle` | 这个接口会**顺手评估超限动作**：后台采样循环起来前，页面每次打开/刷新配额卡片 |
| POST | `/api/quota/set` | `quota_set::handle` | 请求体：`{modem_id, enabled, count_up, count_down, action, reset_day, |
| POST | `/api/quota/reset` | `quota_reset::handle` |  |
| POST | `/api/quota/unblock` | `quota_unblock::handle` |  |

### `/api/ussd`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/ussd` | `ussd::handle` | · `GET  /api/ussd?modem_id=N` 读最近一次应答：`{modem_id, result}`（`result` 可能是 null） |
| POST | `/api/ussd` | `ussd::handle` | · `GET  /api/ussd?modem_id=N` 读最近一次应答：`{modem_id, result}`（`result` 可能是 null） |

### `/api/imei`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/imei` | `imei::handle` |  |
| POST | `/api/imei/set` | `imei_set::handle` | 请求体：`{modem_id, imei}`（15 位数字） |
| POST | `/api/imei/restore` | `imei_restore::handle` | 请求体：`{modem_id}`。没有备份（这台模组从未被本程序改写过）时拒绝执行 |

### `/api/schedule`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/schedule` | `schedule::handle` |  |
| POST | `/api/schedule/set` | `schedule_set::handle` | body 另可带 `modem_id`，用于目标模组留空时的回退；兼容早期前端的 |
| POST | `/api/schedule/run` | `schedule_run::handle` | 执行了但模组拒绝时按 `fail_data` 返回（`code=-1` + 视图 + 错误文本） |
| POST | `/api/schedule/toggle` | `schedule_toggle::handle` |  |
| POST | `/api/schedule/delete` | `schedule_delete::handle` |  |

### `/api/changes`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/changes` | `changes::handle` | 返回波动记录（最新在前）+ 各字段当前值。`modem_id` 省略或为负时返回全部模组的 |
| POST | `/api/changes/clear` | `changes_clear::handle` | 请求体 `{"modem_id":0}`；`modem_id` 省略或为负时清空全部模组。清空会连同基线 |

### `/api/logs`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/logs` | `logs::handle` |  |

### `/api/modules`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/modules` | `modules::handle` | \| 请求 \| 返回 \| |

### `/api/pcie`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/pcie` | `pcie::handle` |  |
| POST | `/api/pcie/reset` | `pcie_reset::handle` |  |
| POST | `/api/pcie/bind` | `pcie_bind::handle` |  |

### `/api/usb`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/usb` | `usb::handle` |  |
| POST | `/api/usb/clear` | `usb_clear::handle` |  |
| POST | `/api/usb/bind` | `usb_bind::handle` |  |

### `/api/atport`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/atport/bind` | `atport_bind::handle` | · GET  列出全部绑定（含「这个口现在还在不在」）+ 本机串口 |
| POST | `/api/atport/bind` | `atport_bind::handle` | · GET  列出全部绑定（含「这个口现在还在不在」）+ 本机串口 |

### `/api/host`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/host/nic` | `host_nic::handle` | 面板在前端已下线（350 系列之外的机型不需要），但接口按迁移清单保留 |
| POST | `/api/host/nic/apply` | `host_nic_apply::handle` | 请求体 `{modem_id, iface, ip, mask, gateway, dns[], mode, execute}` |
| POST | `/api/host/nic/save` | `host_nic_save::handle` | 请求体 `{modem_id, iface, ip, mask, gateway, dns[]}`；校验不通过直接报错不落盘 |

### `/api/ntp`

| 方法 | 路径 | handler | 说明 |
| --- | --- | --- | --- |
| GET | `/api/ntp` | `ntp::handle` | 字段与 WS `ntp` 消息同形（`Status::to_json`），见 runtime/ntp.rs |
| POST | `/api/ntp/set` | `ntp_set::handle` | 接受 IANA 名（`Asia/Shanghai`）或 POSIX 写法（`CST-8`）；解析成功就 |
| POST | `/api/ntp/sync` | `ntp_sync::handle` | 只置一个「强制」标志：后台循环每秒醒一次，最迟 1 秒后执行（见 runtime/ntp.rs） |

## 3. WebSocket

| 路径 | 用途 |
| --- | --- |
| `/ws` | 页面推送：`snapshot` / `dial` / `changes` 等事件（见 `http/ws.rs`） |
| `/display/ws` | 屏端（小屏）推送：信号、短信、任务角标 |

握手走同一端口（`Upgrade: websocket`），鉴权与 HTTP 相同（cookie）。

## 4. 兼容性红线

- 路径、方法、`code` 语义、字段名都是对外契约，改动必须同步 `rust/web/index.html`、`luci/` 与本文；
- 新增字段允许（前端忽略未知字段）；**删除/改名不允许**，除非同时提供兼容期；
- `ROUTES` 反映真实注册表：未登记路径一律 404，不存在「待迁移」兜底。

