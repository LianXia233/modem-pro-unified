# 拨号生命周期：审计、统一状态机与迁移记录（2026-10-21）

> 对象：`main` @ `7653c8b`（本次改动前）的实际源码。
> 结论先行：**可以统一，而且不需要重写任何后端**。统一的是「谁决定连、什么时候重连、
> 状态归谁写、失败怎么退避」；QMI/MBIM 的协议会话、AT 的厂商配方、netifd 的接口
> 与地址配置各自保留，只通过明确的回调点接入。

---

## 1. 旧的调用图（事实）

### 1.1 会「发起连接」的入口

| # | 入口 | 代码位置 | 执行线程 | 走哪个后端 |
| --- | --- | --- | --- | --- |
| E1 | `POST /api/dial/connect` | `http/handlers/dial_connect.rs::handle` | HTTP 线程（同步） | 由 `dial_inner` 内部按 kind 分派 |
| E2 | `POST /api/tasks {kind:"dial.connect"}` | `http/handlers/tasks.rs` 的 `"dial.connect"` 分支 | 任务线程 | 同上 |
| E3 | 自动拨号守护 | `http/loops.rs::spawn_auto_dial`（30s） | 独立线程 | **仅 AT**（`cached_dial_kind` 命中 qmi/mbim 才跳过） |
| E4 | 拨号器托管 | `runtime/dialer/mod.rs::DialerSupervisor::reconcile`（15s，由 `netprov::provision::run_once` 调） | 组网循环线程 | **仅原生**（`native_takes`） |
| E5 | 进程启动 | `http/loops.rs::spawn_all` → E3/E4 的首轮 | 同上 | 同上 |

### 1.2 会「终止连接」的入口

| # | 入口 | 位置 | 做了什么 |
| --- | --- | --- | --- |
| D1 | `POST /api/dial/disconnect` | `dial_disconnect.rs::handle` | `stop_modem()` → `hangup()` → `flush_stale()` |
| D2 | `POST /api/tasks {kind:"dial.disconnect"}` | `tasks.rs` | 同上 |
| D3 | 拨号总开关关 | `dial_enable.rs` + E4 的 `stop_reason` | 只停原生会话；AT 靠 E3 的 `dial_enabled` 判定 |
| D4 | 自动拨号关 | `dial_auto.rs` + E3 | 只影响 AT 自动路径 |
| D5 | 飞行模式 | `airplane_set.rs` / `quota.rs::enforce` | 直接 `AT+CFUN=4`，**不碰 `DialState`** |
| D6 | 模组重启 | `modem_reset.rs` / `tasks.rs "reboot"` | 直接 `AT+CFUN=1,1`，**不碰 `DialState`** |
| D7 | 配额断网 | `http/quota.rs::enforce` | `AT+CGACT=0`（或 CFUN=4），**不碰 `DialState`** |
| D8 | 设备拔出 | `manager::reap_gone` + E4 的 gone 回收 | 停原生会话；`Modem` 整个对象消失 |

### 1.3 状态持有者（互不知情）

| 状态 | 位置 | 谁写 | 谁读 |
| --- | --- | --- | --- |
| `DialState`（state/phase/manual_stop/steps…） | `Modem.dial_state` | **只有** `dial_inner` / `hangup` | HTTP、WS、snapshot、E3、E4 |
| 原生会话 | `DialerSupervisor.native` | E4 | `/api/dial` 的 `host_dialer` |
| 原生退避 | `native_retry` / `native_fails` | E4 | E4 |
| AT 退避 | `spawn_auto_dial` 的局部 `last_fail` | E3 | E3 |
| 接口/地址/路由 | netifd + 内核 | `netprov` | `dial_status` 的 ②③ |

---

## 2. 问题清单（每条都有代码证据）

### P1【严重】原生后端建好会话后，没有任何人把状态写成 `connected`

- 证据：`DIAL_CONNECTED` 的唯一写入点是 `model/dial.rs::dial_inner` 结尾；
  而 `dial_inner` 在 `wan_host_dial_by_daemon(kind) == true` 时**提前 return**
  （「交办给主机侧拨号器」），状态停在 `dialing`。
- 后果：QMI/MBIM 用户点「拨号」后，页面永远显示「拨号中」，即使 `wwan0`
  早已拿到地址；`phase` 也只按 `dialing` 映射。

### P2【严重】两套退避、互不知情

- `spawn_auto_dial`：`FAIL_BACKOFF = 300`（固定）。
- `reconcile_native_one`：`5 → 10 → 20 → 40 → 60`（指数，失败计数独立）。
- 同一台模组切换后端时退避状态不继承；用户在页面上重开开关也不会清掉旧窗口。

### P3【中】两套「该不该连」的判定，条件各写一遍

- E3 判：`dial_enabled` / `auto_dial` / `connected|dialing` / `manual_stop` / 失败窗口。
- E4 判：`host_dial` / `dial_enabled` / `manual_stop`。
- E3 用 `cached_dial_kind()` 跳过原生模组 —— 但**新插入、没拨过号的模组**这个值是空串，
  第一轮会把 QMI 模组也喂给 `Modem::dial()`（虽然只是「交办」，但会写状态、清 `manual_stop`）。

### P4【严重】没有代次：拨号进行中点「断开」会被旧流程覆盖

- `dial_inner` 是 3~60 秒的慢流程；`hangup` 不做任何互斥（只用 `dial_busy` 防两路拨号）。
- 时序：拨号线程发出 `AT+CGACT=1`（成功）→ 用户点断开 → `hangup` 写 `idle + manual_stop=true`
  → 拨号线程醒来写 `connected`（且不清 `manual_stop`）→ 状态变成
  「已连接 + 用户手动断开」这种自相矛盾的组合。

### P5【中】模组重启后状态停在 `connected`，自动重拨永不触发

- `reboot()` 只发 AT，不清状态；E3 看到 `connected` 直接跳过。
- 后果：重启后网卡没有地址，页面仍显示已连接，直到用户手动点一次拨号。

### P6【中】飞行模式 / 配额断网不在生命周期内

- D5/D7 直接发 `AT+CFUN=4` / `AT+CGACT=0`，`DialState` 不动：
  - 页面显示已连接（实际射频已关）；
  - E3 会继续尝试重拨（注定失败），把退避推到 300s。

### P7【记录】AT 路径的 `connected` 只代表「命令序列走完」

- `dial_inner` 在网卡没地址、没默认路由时也写 `connected`（只把缺口写进 `steps`）。
- 这是**有意**的契约拆分：`state` 说任务，`phase` 说链路（`online` / `iface_pending` /
  `route_pending`）。本次**不改** `state` 语义（前端依赖），但让协调器与诊断都以
  `phase` 为准。

### P8【轻】AT 拨号阻塞组网循环

- E3 在自己的线程里串行拨所有模组（每台最多几十秒），期间其它模组的接口配置
  只能等；E4 又在组网循环里同步建 QMI 会话（数秒），同样会推迟 UCI 对账。

### P9【记录】配额断网与拨号是两条正交策略

- `quota::enforce` 断网后每 `block_interval` 重压一次（注释写明「模组可能自己又拨上了」）。
- 本次保留该行为：配额是**策略层**，它有自己的执行/恢复动作，不并入拨号状态机；
  见 §8「有意保留」。

---

## 3. 新架构

### 3.1 职责划分

```
                    ┌──────────────────────────────┐
   手动/任务 ────────→│  Modem::dial / hangup        │  AT 后端（同步，代次门禁）
   自动调度 ────────→│  （vendor::datacall 配方）    │
                    └──────────────────────────────┘
                    ┌──────────────────────────────┐
   自动调度 ────────→│  native::Session::start/stop │  原生后端（QMI/MBIM）
                    └──────────────────────────────┘
                              ▲  回调状态
                    ┌─────────┴────────────────────┐
                    │  DialerSupervisor.reconcile  │  唯一决策点（15s + kick）
                    │  · 意图判定（auto_connect_allowed）
                    │  · 后端选择（native_takes）
                    │  · 统一退避表（RetryTable）
                    │  · 会话回收 / 策略停止
                    └──────────────────────────────┘
                              │ 只读
                    ┌─────────┴────────────────────┐
                    │  netprov / netifd             │  接口、地址、路由
                    └──────────────────────────────┘
```

- **协调器不做连接操作**：AT 走 `Modem::dial`，原生走 `Session::start`；
- **协调器不写 `DialState`**：只调用后端的三个回调（`note_native_up/down`、
  `note_stopped`），写入点集中、语义可审计；
- **netprov 不发起连接**：只建/改/删接口并回报结果。

### 3.2 后端适配接口（现状即接口）

| 能力 | AT 后端 | 原生后端 |
| --- | --- | --- |
| 前置条件 | `dial_readiness` + `vendor::datacall` 配方 | `native::plan_native`（控制口/网卡/APN/PDN） |
| 建立 | `Modem::dial` → 私指令 / CGACT | `native::Session::start` |
| 探活 | 模组侧状态（`AT+CGACT?`，按需） | `Session::alive()`（QMI/MBIM 包业务状态） |
| 拆除 | `Modem::hangup`（对称配方） | `Session::stop` |
| 失败分类 | `steps` + `last_error` | `PlanError` / `Session::start` 错误串 |

### 3.3 状态迁移表

`state` 是**对外契约**（页面/WS 依赖），`phase` 是链路事实；两者都由明确定义的事件驱动。

| 事件 | 触发者 | 前置 | 结果 `state` | 结果 `phase` 依据 | 同时发生 |
| --- | --- | --- | --- | --- | --- |
| 连接请求（AT） | `dial_inner` | `dial_enabled`；非 busy | `dialing` → `connected` / `failed` | iface/addr/route | `manual_stop=false`，代次 +1 |
| 连接请求（原生） | `dial_inner` | 同上 | `dialing`（交办） | 同上 | `backend=native-*`，代次 +1 |
| 原生会话建立 | `reconcile` → `note_native_up` | 代次未变 | `connected` | 同上 | 清 `last_error` |
| 原生会话掉线 | `reconcile` → `note_native_down` | 代次未变 | `dialing`（准备重拨） | — | `manual_stop` 不变 |
| 用户断开 | `hangup` | — | `idle` | — | 代次 +1、`manual_stop=true`、`backend` 清空 |
| 外部断链（重启/飞行/切网卡） | `note_external_break` | — | `dialing` | — | 代次 +1 |
| 策略停用（开关关） | `reconcile` → `note_stopped` | 有会话 | `disconnected` | — | `manual_stop` 不变 |
| 自动重拨 | `reconcile` → 工作线程 `dial` | `auto_connect_allowed` | 同上「连接请求」 | — | 退避记录清除/更新 |

**门禁规则**：任何「写成功态」的动作都必须持有效代次；代次变了只返回错误，
不改状态（见 `dial_stale`）。检查点：每次 `AT+CGACT=1` 前、每条私有激活指令前、
写入「数据通道已建立」前、写入 `connected` 前、原生会话建好后。
慢操作本身不打断（发出去的字节收不回来），只作废结果；把检查点铺到每条激活指令前，
是为了避免「用户已断开、下一条指令又把承载激活起来」的窗口。

### 3.4 所有入口的归属（迁移后）

| 入口 | 迁移后行为 |
| --- | --- |
| `/api/dial/connect` | 直接调 `dial_inner`（代次门禁）；原生路径随后由 `kick` 立即协调 |
| 任务 `dial.connect` | 同上 + `supervisor().kick()`（不等 15 秒节拍） |
| `/api/dial/disconnect` / 任务 | `stop_modem`（停会话 + 清退避）→ `hangup`（bump 代次 + 对称配方 + 清路由） |
| 自动重拨（AT） | **唯一入口**：`reconcile_at_one`，工作线程执行，退避 30→300s |
| 自动重拨（原生） | **唯一入口**：`reconcile_native_one`，退避 5→60s |
| 开关变化 | `dial_enable` / `dial_auto`：打开时 `reset_backoff_all()`；关闭由判定函数拦住 |
| 飞行模式 | 开：`note_external_break`；关：`reset_backoff` |
| 模组重启 | 同步/任务两条都 `note_external_break` + `reset_backoff` |
| 设备拔出 | `reconcile` 回收会话 + 退避 + in-flight 标记 |

---

## 4. 修改文件清单

| 文件 | 变更 |
| --- | --- |
| `rust/src/modem/model/modem.rs` | 新增 `dial_epoch`（连接代次）与 `dial_epoch()/bump_dial_epoch()/dial_epoch_is()/dial_in_progress()` |
| `rust/src/modem/model/dial.rs` | `DialState` 新增 `backend` 字段；`dial_inner` 持代次并对每次成功提交做门禁；`dial_stale`、`dial_fail_guarded`；`hangup` 首步 bump 代次；新增 `note_native_up/down`、`note_external_break`、`note_stopped`；新增 4 个单测 |
| `rust/src/runtime/dialer/mod.rs` | 重写编排：统一 `Backend` + `retry_delay`（AT 30→300s，原生 5→60s）、`RetryTable`、`auto_connect_allowed`、`reconcile_at_one`、原生路径代次校验与状态回调、`kick()`、`reset_backoff*()`、`InFlight`/`Starting` 两个 RAII 门锁；删除旧的 `native_retry/native_fails` |
| `rust/src/http/loops.rs` | 删除 `spawn_auto_dial`（30s 独立循环）；线程计数 8→7 |
| `rust/src/http/handlers/tasks.rs` | `dial.connect` 成功后 `kick`；`reboot` 后 `note_external_break` + 清退避 |
| `rust/src/http/handlers/dial_auto.rs` | 打开时清退避 |
| `rust/src/http/handlers/dial_enable.rs` | 打开时清退避；更新注释 |
| `rust/src/http/handlers/airplane_set.rs` | 开：`note_external_break`；关：清退避 |
| `rust/src/http/handlers/modem_reset.rs` | 重启后 `note_external_break` + 清退避 |
| `rust/src/http/mod.rs` / `model/params.rs` / `model/mod.rs` / `luci/.../dial.js` | 注释与页面文案同步（30 秒 → ≤15 秒统一节拍） |
| `docs/INTEGRATION.md` | ④/⑤b/⑥ 改为协调器口径 |
| `docs/DIAL_STATE_MACHINE.md` | 本文件 |

---

## 5. 兼容性检查

| 约束 | 结论 |
| --- | --- |
| HTTP 路径 / JSON 字段 | 不变。`DialState` **新增可选字段 `backend`**（空串时不下发），旧前端忽略即可 |
| `state` 字符串语义 | 不变（`connected/dialing/failed/idle/...`）；只是原生路径现在会**正确**进入 `connected` |
| 厂商拨号配方 | 不变：`vendor::datacall` 一行未改 |
| `AT+CGACT=1` 不得用于 QMI/MBIM | 保持：`dial_inner` 仍提前 return，协调器也从不发 AT 激活 |
| `manual_stop` 语义 | 保持且加强：只有 `hangup`（用户断开）置位；后端掉线/策略停用都不置 |
| FM350/RW350 静态地址 | 未触碰（`netprov` 与 `hostnic` 原样） |
| 主 WAN / LAN / 防火墙 / metric | 未触碰 |
| 拨号总开关 / 自动拨号 / host_dial | 语义不变；新增「重开时清退避」 |
| 配额断网 | 行为不变（仍每 `block_interval` 重压）；见 §8 |

---

## 6. 测试

### 6.1 离线自动化（本机执行）

| 命令 | 结果 |
| --- | --- |
| `cargo test --locked` | **746 passed / 0 failed**（改动前 740） |
| `cargo clippy --all-targets --locked` | **0 warning** |
| `cargo fmt --check` | 干净 |
| `cargo build --bin modemd` | 通过 |

新增用例与覆盖点：

| 用例 | 钉住的行为 |
| --- | --- |
| `retry_delays_share_shape_but_differ_in_scale` | 两种后端共用一套策略函数；AT 30/60/120/240/300，原生 5/10/20/40/60 |
| `retry_table_clears_on_changed_error_and_success` | 同一错误持续退避；错误变化重数；成功清零 |
| `auto_connect_rejects_each_reason` | 开关、`manual_stop`、已连接、拨号中（有人跑/无人跑）、退避未到 |
| `hangup_bumps_epoch_even_when_the_at_command_fails` | 用户断开的意图优先于 AT 结果 |
| `native_up_is_rejected_after_an_intent_change` | 过期代次不能提交 `connected` |
| `native_up_and_down_keep_the_authoritative_state` | 原生后端写状态；掉线回 `dialing` 且不置 `manual_stop` |
| `external_break_invalidates_in_flight_dial` | 重启/飞行模式作废在途拨号 |

### 6.2 必须实机验证（本沙箱无模组、无 OpenWrt，未执行）

1. **QMI/MBIM 首次连接**：`/api/dial/connect` 后 ≤3 秒内 `state` 从 `dialing` → `connected`
   （`kick` 生效），且 `backend=native-qmi|mbim`。
2. **拨号中打断**：连接过程中点断开 → 最终 `state=idle`、`manual_stop=true`，
   不会在几秒后跳回 `connected`。
3. **重启恢复**：`/api/modem/reset` 后模组回来，`state` 从 `dialing` 自动转 `connected`。
4. **飞行模式**：开 → `state=dialing`（随后失败退避）；关 → 30 秒内自动恢复。
5. **AT 失败退避**：拔卡后观察日志间隔为 30/60/120/240/300 秒，插回卡后恢复。
6. **原生重拨**：`AT+CFUN=1,1` 或断网后，原生会话按 5→60 秒重建。
7. **多模组**：一台 AT、一台 QMI 同时工作，AT 慢拨不推迟另一台的接口对账。
8. **FM350 静态地址**：拨号后 `phase=online`，`ipv4` 由 netprov 静态写入（无 DHCP）。

---

## 7. 遗留问题

| 项 | 现状 | 建议 |
| --- | --- | --- |
| AT 链路「掉线」探测 | 不做主动轮询；依赖模组会话 + netifd。已覆盖显式断链（重启/飞行/配额） | 需要时加低频 `AT+CGACT?` 探活（注意串口占用） |
| 运行中改 APN（AT） | `set_apn` 就地重激活，不重建会话 | 与产品确认是否需要「改完自动重拨」 |
| `dial_status` 的 netifd 反推 | `state=idle` 时按地址推断为 `connected`（link_source=netifd） | 保留（用户手工建链路时的唯一可见来源） |
| `describe()` 只暴露原生后端 | AT 后端没有等价的结构化诊断 | 页面已能从 `steps` 看到，暂不扩展 |
| 原生探活在**全局**会话表锁内执行 | 一台模组的 QMI 查询会短暂挡住其它模组的 `reconcile`（每轮一次，量级毫秒~秒） | 需要时改成「每会话一把锁」：取出→查→放回，改动集中在 `native` map |
| 后台 AT 拨号线程不可取消 | 受 `AtPort` 超时约束（单条 8~20 秒），最坏几十秒后自行结束 | 代次门禁已保证结果作废；如需秒级取消，给 `dial_inner` 传取消标志 |

## 8. 有意保留（不是遗漏）

1. **配额断网不进状态机**：`quota::enforce` 是策略层，自己断、自己恢复、
   自己按间隔重压。并入拨号会让「超量」与「用户意愿」两种意图互相覆盖。
2. **`auto_dial` 只管 AT 路径**：QMI/MBIM 的自动连接由 `host_dial` 管辖（关掉它
   等于「不托管拨号器」），`auto_dial` 不参与原生判定 —— 这是既有语义，改动会让
   「关自动拨号但插上 QMI 卡仍要能上网」的用户断网。统一的是决策**位置**与退避**记账**，
   不是把两个开关合并。
3. **AT 的 `connected` 不等价于 `online`**：`state` 是任务语义，`phase` 才是链路事实；
   改 `state` 会破坏既有前端与历史曲线。
4. **`spawn_netprov` 的 15 秒节拍**：每轮两个 `uci show` 是固定成本；拨号决策搭车，
   不额外增加权限与配置写入。
5. **原生 5→60s 比 AT 30→300s 快**：失败性质不同（瞬时 vs 持续前置条件），
   统一的是策略与记账方式，不是具体秒数。

## 9. 回滚方案

改动全部集中在拨号域，按依赖顺序回滚即可（`git revert` 单个提交亦可）：

1. `http/loops.rs` 恢复 `spawn_auto_dial` 与 `spawn_all` 中的调用、线程计数 8；
2. `runtime/dialer/mod.rs` 恢复 `native_retry/native_fails` 与旧 `reconcile`；
   删除 `reconcile_at_one` / `kick` / `retry_delay(Backend, ..)`；
3. `model/dial.rs` 删除 `dial_stale`/`dial_fail_guarded`/`note_*` 与 `backend` 字段，
   `hangup` 去掉 bump，`dial_inner` 去掉代次；
4. `model/modem.rs` 删除 `dial_epoch`、`dial_in_progress`；
5. handlers 中去掉 `kick` / `reset_backoff` / `note_external_break` 调用。

数据面无需迁移：`DialState` 是内存态、`quota.json` 等台账格式未变；
`backend` 是新增可选字段，回滚后旧代码不会读到它。
