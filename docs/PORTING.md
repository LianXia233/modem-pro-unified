# Go → Rust 全量重构清单（PORTING）

目标：Rust 后端 `modemd` 对原 Go 后端做**逐字对齐**（路由、方法、JSON 契约一致），
前端 `rust/web/index.html` / `mini.html` 不改也能正常工作。

- 响应信封：`{"code":int,"data":any,"msg":string}`，成功 `code:0`，业务失败 `code:-1`；
  鉴权失败 HTTP 401 + `{need_login:true,login_url}`；网关错误 400/429。
- 参数惯例：几乎都吃 `modem_id`（query 或 JSON body）。
- 鉴权：cookie `modemd_sess`（跟随 LuCI 登录）；短信令牌 HTTP 头 `X-SMS-Token`、
  WS 握手 `?sms_token=`。
- 前端的接口全集另见 `index.html` 内置 `RAW_APIS`（56 条子集）。

状态图例：`[x]` 已迁移并可用 · `[~]` 部分可用 · `[ ]` 待迁移。

---

## 阶段划分

1. **地基**（已完成）：自有同端口网关（HTTP 透传 + WebSocket 升级）、路由总表、
   推送中枢 `WsHub`（`broadcast` / `has_clients` / `sms_token` 预留）。
2. **只读域**：version / modems / signal / network / cells / lock(status,caps) / sms(read,storage) /
   modem(info,status,clock) / params(读) / usbnet(读) / imei(读) / mode(读) / quota(读) /
   schedule(读) / changes / logs / modules / at(presets) / usb / pcie / atport / dial(读) / airplane / roam。
3. **写操作域**：at(发,port) / sms(send,delete,storage 切换,auth,unlock,lock,password) /
   params(apn,pdp,netselect) / lock(band,cell,mode,unlock) / dial(connect,disconnect,enable,mode,ipv6) /
   usbnet(set) / imei(set,restore) / mode(set) / airplane(set) / roam(set) / quota(set,reset,unblock) /
   schedule(set,delete,toggle,run) / traffic/reset / modem(reset,clock) / usb(bind,clear) / pcie(bind,reset) /
   atport/bind / netscan(,stop) / changes/clear。
4. **推送**：WebSocket `/ws`、`/display/ws` 与 `push_*` 全部消息类型。
   **已收口**：各面板推送函数齐备，写操作成功后顺手广播（见上文「推送触发点」）；
   **屏端动作协议已完成**（`snapshot` / `ping` / `sms_read` / `sms_delete` / `mode_get` /
   `mode_set` / `sms_send` / `net_get`，见 `PROGRESS.md` §26，实机已验证）；
   URC 异步推送已接线（`+CUSD` / `+CMTI`，见 `PROGRESS.md` §22），
   `+CPIN` 刻意不在推送范围（与查询应答同形，语义不可信）。
5. **后台循环**：信号/网络/流量采样、配额评估、定时任务、SIM 热插拔、USB 热插拔、组网（netprov）、
   企业微信登录与推送、历史记录。
   **已接线**：push / traffic / quota / change / hotplug / sim / logs / urc 八类循环
   （`http/loops.rs`）；其中 **urc 循环**（`spawn_urc`）消费 `at/urc.rs` 暂存的主动上报，
   按类别折算成「SIM / 网络置脏」并把巡检加速到下一拍，`+CUSD` 直接推页面、
   `+CMTI` 触发一次短信重读（见 `PROGRESS.md` §21 / §22）。
   USB 热插拔推送、~~组网（netprov）~~（已完成，见 §23）、~~企业微信~~（已完成，见 §24）待补。
   （~~主机网卡 `/api/host/nic*`~~ 已完成，见 `PROGRESS.md` §28。）
6. **拨号器托管**：`hostdial`（按厂商启动/回收拨号器进程，含 udhcpc）—— ✅ 已完成，见 `PROGRESS.md` §25。

---

## 路由总表

### 基础 / 会话 / 版本
| 状态 | Method | Path | 说明 |
| --- | --- | --- | --- |
| [x] | GET | `/api/version` | 版本 + 运行时 + platform |
| [x] | POST | `/api/login` | 转发 ubus `session.login`，下发 cookie |
| [x] | POST | `/api/logout` | 清 cookie |
| [x] | GET | `/api/session` | 登录态 |

### 模组 / 信号 / 网络
| 状态 | Method | Path | 说明 |
| --- | --- | --- | --- |
| [x] | GET | `/api/modems` | 列表，`?discover=1` 触发重探 |
| [x] | GET | `/api/modem/info` | vendor/model/at_port/imei/firmware |
| [x] | GET | `/api/modem/status` | ICCID/IMSI/号码/温度/电压/时钟/选网/漫游（`?refresh=1` 缓存待补） |
| [x] | POST | `/api/modem/clock` | 校准模组时钟（`AT+CCLK` 写 UTC，回退不带时区；返回整份模组状态） |
| [x] | POST | `/api/modem/reset` | 重启模组（`AT+CFUN=1,1`；未收到确认也按已下发上报） |
| [x] | GET | `/api/sim` | SIM 在否 / 身份 / 运营商 |
| [x] | GET | `/api/vendor/cmds` | 探测出的厂商指令集 |
| [x] | GET | `/api/signal` | 信号（CSQ + QENG/CESQ 的 RSRP/RSRQ/SINR） |
| [x] | GET | `/api/signal/history` | 采样历史（曲线）；`?modem_id=-1&minutes=30&limit=2000` |
| [x] | GET | `/api/network` | 注册状态（基础） |
| [~] | GET | `/api/network/status` | plmn/tech/band/apn/ipv4/cs_reg/ps_reg/roaming + `traffic`（累计/速率，ca/serving/ambr 待补） |
| [x] | GET | `/api/netinfo` | 网络接入信息（按厂商发 `AT+QNWINFO` / `AT^SYSINFOEX`；Unisoc 回空） |
| [x] | GET | `/api/airplane` | 飞行模式状态 |
| [x] | POST | `/api/airplane/set`（`{enabled}`，`AT+CFUN=4` / `=1`） | 设置飞行模式 |
| [x] | GET | `/api/roam` | 漫游状态 |
| [x] | POST | `/api/roam/set`（`{on}`，策略开关：写回 `[net] allow_roaming` 并即时生效） | 写 `[net] allow_roaming` |

### 基站 / 锁网
| 状态 | Method | Path |
| --- | --- | --- |
| [x] | GET | `/api/cells`（`?cache=1`） |
| [x] | GET | `/api/cells/all` |
| [x] | GET | `/api/netscan`（`?tech=lte|nr|both&minutes=3~10`；可中断） |
| [x] | POST | `/api/netscan/stop` |
| [~] | GET | `/api/lock/status`（`?refresh=1`；QNW/芯讯通方言）|
| [~] | GET | `/api/lock/caps`（QNW/芯讯通方言）|
| [x] | POST | `/api/lock/band` | 锁/解频段（QNW 冒号列表；解锁读 policy_band 全量写回，芯讯通解锁未验证拒绝盲写） |
| [x] | POST | `/api/lock/cell` | 锁小区/频点（`lock_type` 0/1/2，QNWLOCK；芯讯通拒绝） |
| [x] | POST | `/api/lock/mode` | 旧接口：组网模式偏好（复用 `mode` 域下发 + 回读确认） |
| [x] | POST | `/api/lock/unlock` | 一键解除全部（频段回 policy + 小区置 0；芯讯通拒绝） |
| [x] | GET | `/api/mode`（`?refresh=1`） |
| [x] | POST | `/api/mode/set`（`{value}`，写完回读确认） |

### 拨号 / USB 网卡 / 主机网卡
| 状态 | Method | Path |
| --- | --- | --- |
| [x] | GET | `/api/dial`（`?force=1`） | USB 类型三路识别（AT/描述符/主机驱动）+ 拨号状态 + 开关三件套；`auto_wan` 返回自动组网报告（`netprov/`，见 `PROGRESS.md` §23） |
| [x] | POST | `/api/dial/enable`（`{on}`） | 拨号总开关：写回 `[net] dial_enabled` 并即时生效（缺 `on` 报错） |
| [x] | POST | `/api/dial/mode`（`{mode}`） | WAN 模式（modem_pro / dhcp / both）：写回 `[net] wan_mode` + Label + 全量列表 |
| [x] | POST | `/api/dial/ipv6`（`{on}`） | IPv6 开关：写回 `[net] wan_ipv6` 并即时生效（缺 `on` 报错） |
| [x] | POST | `/api/dial/connect`（`{modem_id,cid,apn,pdp_type,reconnect,force}`） | AT 拨号（ECM/RNDIS/NCM 等）：写 APN → 激活 PDP → 激活 IMS → 读本机地址；QMI/MBIM **不发** `AT+CGACT`（会占掉拨号器的唯一数据承载），改为把 APN 落到状态、由组网循环拉起 modemd 托管的厂商拨号器（`runtime/dialer/`，见 `PROGRESS.md` §25）；失败也回 `DialState` |
| [x] | POST | `/api/dial/disconnect`（`{modem_id,cid}`） | `AT+CGACT=0,<cid>` + 回读确认；QMI/MBIM 先按进程树停掉托管中的拨号器（连 udhcpc）再重置状态 |
| [x] | GET | `/api/usbnet`（`?refresh=1`） |
| [x] | POST | `/api/usbnet/set`（`{value}`，仅移远/美格可写；广和通 RNDIS 不允许执行） |
| [x] | GET | `/api/host/nic`（前端面板已下线、接口保留：FM350 这类机型要靠它手工配主机网卡） |
| [x] | POST | `/api/host/nic/apply`（`{modem_id,iface,ip,mask,gateway,dns[],mode,execute}`；`mode`=renew/static/dhcp，命令默认只生成不执行） |
| [x] | POST | `/api/host/nic/save`（写 `hostnic.json`，掩码归一、DNS 去空去重，校验不通过不落盘） |

### 短信 / 微信
| 状态 | Method | Path |
| --- | --- | --- |
| [x] | GET | `/api/sms/auth`（返回 `required`/`unlocked`/`default`/`ttl_s`/`outer_auth`） |
| [x] | POST | `/api/sms/unlock`（`{"password"}` → 令牌；错 5 次锁 60s） |
| [x] | POST | `/api/sms/lock` |
| [x] | POST | `/api/sms/password`（改密码 / 开关保护，写回 config.toml 并吊销旧令牌） |
| [x] | GET | `/api/sms/read`（PDU/文本解码；`AT+CMGL` 副作用保留） |
| [x] | POST | `/api/sms/send`（`{phone,content}`；中文走 UCS2，超长自动分段） |
| [x] | POST | `/api/sms/delete`（`{index}` 单条 / `{all:true}` 全部） |
| [x] | GET | `/api/sms/storage`（读各存储区用量与当前区） |
| [x] | POST | `/api/sms/storage`（切存储区，回读确认并写回 config.toml） |
| [x] | GET | `/api/wechat` | 读配置 + 登录态（不回传 token）；过短信密码闸门 |
| [x] | POST | `/api/wechat/login` | 拿登录二维码（未过期的复用，不重复拿） |
| [x] | POST | `/api/wechat/verify` | 轮询扫码结果；成功则凭据写回 config.toml |
| [x] | POST | `/api/wechat/set` | 改配置：行级原地写回 + 内存覆盖即时生效 |
| [x] | POST | `/api/wechat/test` | 发一条测试消息（同步 curl，超时 15s） |
| [x] | POST | `/api/wechat/logout` | 登出并清空配置里的三份凭据 |

### 参数 / 配额 / USSD / IMEI / 模式
| 状态 | Method | Path |
| --- | --- | --- |
| [~] | GET | `/api/params`（`?refresh=1`） |
| [x] | POST | `/api/params/apn`（`{cid,pdp_type,apn}`：回读确认 + 重新激活 + 激活 IMS） |
| [x] | POST | `/api/params/apn/delete`（`{cid}`，回读确认） |
| [x] | POST | `/api/params/pdp`（`{cid,active,reconnect}`） |
| [x] | POST | `/api/params/netselect`（`{mode,plmn}`，`AT+COPS=0` / `=1,2,"<plmn>"`） |
| [x] | GET | `/api/quota`（`?modem_id=N`；采样 + 累计 + 超限评估） |
| [x] | POST | `/api/quota/set`（`{enabled,limit_bytes|limit_mb|limit_gb,count_up,count_down,action,reset_day,used_gb?}`） |
| [x] | POST | `/api/quota/reset`（本周期清零 + 新周期 + 恢复连接） |
| [x] | POST | `/api/quota/unblock`（本周期不再限制 + 恢复连接） |
| [x] | POST | `/api/traffic/reset`（清零模组累计计数器，返回流量视图） |
| [x] | GET/POST | `/api/ussd`（GET 读最近一次应答 `{modem_id,result}`；POST 下发 `{code}`，校验码字符集 + `AT+CPIN?` 判 PIN 锁；异步 `+CUSD` 推送属阶段 5 URC 读协程，同步到达的那种当场收下）|
| [x] | GET | `/api/imei`（`?refresh=1`） |
| [x] | POST | `/api/imei/set`（`{modem_id,imei}`：首次改写前用同族只读命令（广和通 MTK 为 `AT+EGMREXT=0,7`，其余 `AT+CGSN`）快照备份原始值到 `imei.json`，下发写入后回读确认——广和通 MTK 需重启模组才生效，回读对不上时带 `warning` 提示而不判失败） |
| [x] | POST | `/api/imei/restore`（一键写回备份的原始值，并记录 `restored_at`；备份原值为空或从未备份时拒绝） |

### 定时器 / 波动 / 日志 / 清单 / 调试
| 状态 | Method | Path |
| --- | --- | --- |
| [x] | GET | `/api/schedule`（视图一次拉全：`tasks`+`kinds`+`modems`+`max_tasks`+`supported`；支持性预检看**真被固件接受过**的方言探测） |
| [x] | POST | `/api/schedule/set`（Task 校验归一化 + `modem_id` 回退 + 旧版 `start_action`/`end_action` 单动作兼容；成功返回视图并广播） |
| [x] | POST | `/api/schedule/run`（立即执行开始动作并标记当前窗口槽位；模组拒绝时 `fail_data` 带视图与错误文本） |
| [x] | POST | `/api/schedule/toggle`（`{"id","enabled"}`；不存在回「任务不存在（可能已被删除）」） |
| [x] | POST | `/api/schedule/delete`（同上；成功返回视图并广播） |
| [x] | GET | `/api/changes`（`?limit=200&modem_id=-1`；返回 `entries` + `current`） |
| [x] | POST | `/api/changes/clear`（`{modem_id}`；连同采样历史一起清） |
| [x] | GET | `/api/logs`（`?lines=`） |
| [x] | GET | `/api/modules`（`?model=`/`?modem_id=` 待补） |
| [x] | GET | `/api/pcie`（`catalog_models` 字段暂缺，其余对齐） |
| [x] | POST | `/api/pcie/reset`（remove + rescan；非 unix 回「不支持」） |
| [x] | POST | `/api/pcie/bind`（driver 空则按已知驱动挨个试） |
| [x] | GET | `/api/usb`（平台/能力/设备/事件/建议） |
| [x] | POST | `/api/usb/clear` |
| [x] | POST | `/api/usb/bind`（手动补 usb-serial 驱动） |
| [x] | GET | `/api/atport/bind`（`{supported,path,ports,binds}`；键归一 `vid:pid`） |
| [x] | POST | `/api/atport/bind`（`{vid,pid,port}`；port 空=解绑，即时重新发现） |
| [x] | GET/POST | `/api/at`（`{modem_id,cmd}` → `{cmd,resp:[行数组]}`；单条 ≤128 校验，失败也回半截应答） |
| [x] | POST | `/api/at/port`（身份键台账 `atport.json`；探活后切口/恢复自动探测） |
| [x] | GET | `/api/at/presets`（按厂商合并清单；无模组回 code 0 兜底） |

### 推送 / 静态
| 状态 | Method | Path |
| --- | --- | --- |
| [x] | WS | `/ws` |
| [x] | WS | `/display/ws`（免密 key `h29k-tft-9f3c1a7b`） |
| [x] | GET | `/`、`/ui`、`/mini`、`/mini.html`、`/layouts/*.css` |

---

## WebSocket 消息（`{type, modem_id, data}`）

`modems`、`signal`、`traffic`、`quota`、`network`、`dial`、`cells`、`lock`、`schedule`、
`sms`、`usb`、`sim`、`wechat`、`ussd`、`changes`、`changes_clear`（无 modem_id）、`logs`、
`smscount`（mini）。

握手后客户端发 `{action:'snapshot'}` 索取初始快照；无客户端连接时不发 AT。

推送触发点（写操作成功后顺手广播，免得各个页面各查一遍）：
`/api/cells`（真扫后）、`/api/lock/{band,cell,mode,unlock}`、`/api/sms/read`（`sms` + `smscount`）、
`/api/quota/{set,reset,unblock}`、`/api/traffic/reset`、`/api/ussd`（POST 同步收到 `+CUSD` 时）、
SIM 热插拔事件（`sim`，并清短信缓存后补推空 `sms` + `smscount`）。

- `sms` 带正文，**过短信密码闸门**（与 `/api/sms/read` 同一把令牌：没配密码时人人可收）；
  `smscount` 只有数字、不含正文，因此不过闸门（锁着也能看到有几条未读）。
- **屏端动作协议已完成**（客户端 → 服务端，一律 `{action, modem_id, ...}`）：
  `snapshot`（再来一份快照）/ `ping`（回 `pong`）/ `sms_read`+`index`（`AT+CMGR` 读一次即标已读，
  随后重读列表并推 `sms` + `smscount`）/ `sms_delete`+`index`（删完重读再推）/
  `mode_get`（推 `mode`：`value`/`name`/`short`/`choices`/`screen`/`read_only`）/
  `mode_set`+`value`（下发 + 回读确认，成功推「已切换到 …」、失败推「设置失败：…」）/
  `sms_send`+`number`+`text`（发完推 `sendsms`：`number`/`text`/`ok`/`msg`）/
  `net_get`（推 `network`）。
  ⚠️ 除 `snapshot` / `ping` 外全部丢独立线程执行：发短信要等提示符、切模式要等回读，
  卡住 WS 读循环会让这一路的心跳与推送全停。`index` 是**存储位置**（不是列表下标）。
  `mode` 消息里 `screen` 是屏上那三个按钮（`LTE`→4G / `NR5G`→5G / `LTE:NR5G`→4G+5G），
  本机档位表里没有的档 `writable=false`（置灰，别让用户点了必定失败）。

---

## 关键约束（真机经验，迁移时必须保留）

- netifd 协议名固定 `modem_pro`（连字符会导致 shell 函数名非法）。
- 四家拨号器 **不可互相顶替**（QMI 初始化序列不同）。
- `AT+CMGL` 会把未读标已读 ⇒ `/api/sms/read` 有副作用，前端口径依赖此行为。
- `/api/sms/storage` 同路径 GET（读用量）/ POST（切存储区）。
- 波动记录 / 采样历史只在**进程内环形缓冲**（波动 500 条、每模组 1800 段），不落盘；
  记录点挂在已有查询上（`/api/signal`、`/api/network/status`、`/api/cells`），**不额外发 AT**；
  采样值没变时只把最后一个点的时间往后推，曲线上仍是一条画到「现在」的平线。
- 短信访问密码闸门：`require_luci_login` 开启、或 `sms_password_required=false`、或
  `sms_password` 显式写成空串时**不需要**密码；令牌 TTL 30 分钟、最多 32 个、连错 5 次锁 60 秒、
  默认密码 `password`；令牌优先取 `X-SMS-Token` 头，其次 query `sms_token`。
- 一个 AT 口 = 一个模组；同模组多 ttyUSB 合并待补。
- 流量配额：用量按「当前累计 − 上次累计」累加（模组重启归零时把当前值当增量），首次采样只建基线；
  广和通只有瞬时速率 ⇒ 按 `速率 × 间隔` 积分（标 `estimated`）；台账写 `quota.json`（配置同目录）。
  超限时**只停上网用的 PDP 上下文**（`AT+CGACT=0,<cid>`，跳过 ims/sos/mms），`block_rf` 走 `AT+CFUN=4`。
- 主动扫网会独占串口几分钟，必须能被 `/api/netscan/stop` 打断：中断后已收到的部分照常解析并标 `stopped`。
- 短信写操作（发 / 删 / 切存储区）都过同一道短信密码闸门；改密码 / 开关保护要**再输一次当前密码**，
  成功后吊销全部旧令牌并给当前页面补发新的。写回 `config.toml` 只改目标那一行，注释原样保留。
- 推送闸门：`sms` 推送过密码闸门（与 `/api/sms/read` 共用令牌）；`smscount` 只推
  `{total,unread}` 且**不过**闸门。
- LuCI 界面（2026-10-18 起）**只有 JS 视图 + ucode 控制器**世代，`luci/app/lua/**` 已删除：
  - ucode dispatcher **只扫** `/usr/share/luci/menu.d/*.json` 与
    `/usr/lib/lua/luci/controller/*.lua`；ucode 控制器要能被分发，必须在 menu.d 里
    注册 `type:function` 节点（`require(module)` 后调 `function`），否则端点 404。
  - ucode **没有 `typeof` 运算符**，判断类型只能用 `type()`；写错会在 `require` 时
    Syntax error，整个请求 500。
  - `rpc.declare` 的 `expect` 是**单键选择器**而非校验器（`ret = ret[expect 的第一个键]`，
    取不到或类型不符就用默认值顶上）：`luci.getInitList` / `service.list` 的返回都是
    **按名字建键的对象**，写 `expect:{result:[]}` 会被削成空数组。
  - 运行态只在 procd 的 `service list` 里（`luci.getInitList` 只给 `enabled`）。
  - 菜单标题保持英文：`load_catalog` 在 `parser.so`（C 模块）里，只认 `.lmo` 二进制，
    构建期需要 `po2lmo`（设备与单包 Makefile 均无）；界面内文案由 JS 视图的 `T()` 翻。
- **广和通 MTK 平台（FM350 / RW350 系列）专属口径**，详见 `PROGRESS.md` §15：
  - 温度只认 `AT+GTSENRDTEMP=0`（量纲 0.001 ℃）；**读形式 `AT+GTEMP?` 在该平台回
    `phone failure`，不要用**。
  - 注册态必须用 `AT+CEREG?`（`AT+CREG?` 恒 `0,0`）。
  - `AT+CGACT?` **只回 `OK`、不列 `+CGACT:` 行** ⇒ 判据必须三态，空转即误判「已去激活」。
  - 短信走 **PDU 模式**整条序列（`AT+CMGF=0` → `AT+CMGS=<len>` → `>` → PDU + Ctrl-Z），
    发完切回 `AT+CMGF=1`；TEXT 模式在该平台中文与长短信都不可靠。
  - IPv6 未分配时回 `0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.1`（16 段点分的 `::1` 占位），
    必须判为无效，否则会配出一条永远不通的 v6 默认路由。
  - URC 过滤**只吞明确是主动上报的行**：`+CPIN` 不吞（与查询应答同形）、
    本程序查询过的前缀不进表（`AT+GTDNS` 的响应就长 `+GTDNS:`）、
    洪水阈值只对 URC 行计数（`AT+COPS=?` 一次能回上百行）。
   `+CUSD` 靠**形态判据**进表（查询应答 `+CUSD: (0-2)` 要放行），`+CMTI` 直接进表；
   暂存后的**消费**（`model/urc.rs` + `http/loops.rs::spawn_urc`）见 `PROGRESS.md` §21 / §22。
