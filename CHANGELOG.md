# 变更记录

本项目采用 [语义化版本](https://semver.org/lang/zh-CN/)；版本号单一来源为仓库根 `VERSION`。

> **文档状态（2026-10-29 起）**：`docs/` 已整体移除（2026-10-20 曾重建一轮，
> 之后与代码状态再次脱节，不再随仓库维护）。本文件历史条目中提到的任何
> `docs/*.md`（含 `PORTING.md` / `PROGRESS.md` / `HANDOFF.md` / `PRIVACY.md` /
> `DIAL_AUDIT.md` / `INTEGRATION.md` / `BUILD.md` / `API.md` / `UPSTREAM_AUDIT.md` /
> `SUPPORT_MATRIX.md` / `DIAL_STATE_MACHINE.md`）**均只是历史记录**，
> 不得作为当前设计依据；当前事实以源码、`config.toml` 注释与 git 历史为准。

## [未发布]

### 变更：移除 docs/ 文档集，仓库内不再维护文档

**原因**：文档与代码状态脱节（WebUI 退役、拨号器多次演进后，`docs/` 里的
架构图、接口契约、审计结论已对不上当前实现），维护者决定不再随仓库维护文档，
以源码、`config.toml` 逐项注释与 CHANGELOG 作为事实来源。

**删除**：`docs/`（`README.md` / `INTEGRATION.md` / `BUILD.md` / `API.md` /
`DIAL_STATE_MACHINE.md` / `UPSTREAM_AUDIT.md` / `SUPPORT_MATRIX.md`，共 7 个文件）。

**改动**：清除全部指向 `docs/*` 的引用 —— `README.md`（文档节改为「事实来源」
清单：路由表 / config.toml 注释 / CHANGELOG）、`RELEASE_NOTES.md`、`Makefile`
两处构建提示、`scripts/check-desensitize.sh`（指向早已删除的 `PRIVACY.md`）、
`rust/Cargo.toml` 与源码注释约 15 处「契约见 docs/API.md」类指引（语义保留，
指针移除）；指向**上游仓库**文档的引用（QModem `support_list.md` 等）与
CHANGELOG 历史条目原样保留。

### 变更：对照上游移植拨号器能力（generic MHI 数据面 + 拨号失败自愈复位）

**对照基线**（全部以源码为准，`e00a306 → 4c11622` 与 Go `397e014`）：
`FUjr/QModem` 的 generic MHI / 嵌套 QMI 数据接口系列提交
（`mhi_generic.h`、`QMIThread.c:324-357,774-935`、`modem_netdev.sh`）
与 `ouyangzq/modem-pro` 的 `hostdial_sync.go` 复位节流。

**移植 1：generic MHI（主线 PCIe 驱动）数据面** —— 此前控制口发现只认
`cdc-wdm` / `GobiQMI` / `mhi_QMI` 三种布局，`mhi_pci_generic` / `mhi_net` /
`mhi_wwan_*` 这类主线 MHI 驱动的模组（控制口在 `/dev/wwan*`）算不出计划，
MBIM 还被硬性限定 cdc-wdm。现在：

- `native/mod.rs`：新增 `ControlKind::MhiGeneric` 与 wwan 类控制口发现
  （`/sys/class/wwan/*/type` 精确匹配 `QMI`/`MBIM`、端口必须落在数据网卡
  device 的 `mhi<N>` 控制器子树内、`/dev` 节点存在、**候选数 ≠ 1 直接放弃**
  —— 上游 `cm_mhi_detect` 的 fail-closed 口径；`_IP_SW` 软件通道排除）；
- `qmi.rs`：`wda_set_data_format_rawip`（裸 IP、无聚合，TLV 布局照抄
  quectel-CM generic-MHI 分支）+ `wds_bind_mhi`（与 BindMux 同消息号 0x00A2
  但**省略 mux id TLV**）+ `validate_data_format_rawip`（应答强校验：设备
  报非裸 IP 或仍聚合即失败）；
- `qmi_link.rs`：generic MHI 会话顺序 = WDA(裸 IP) → BindMhi(v4/v6 各一次)
  → Start；MBIM 解锁 wwan(MBIM) 控制口；
- 假设备顺序测试覆盖完整帧序列（绑定帧无 0x11、WDA 无 0x15/0x16）。

**移植 2：`[net] reset_before_dial` 自愈复位**（此前配置注释明示「未实现」，
Go 上游已落地）：原生路径对同一模组连续失败 ≥5 次（错误变化清零重数）且距
上次复位超过冷却期（`reset_before_dial_cooldown`，默认 300s、钳 60~3600），
在工作线程里发 `AT+CFUN=1,1` 并等 AT 口恢复（≤150s，3s 轮询），完成后清退避
立即重拨。复位只作用于 QMI/MBIM 原生路径 —— AT 路径的持续失败（没插卡/
无服务）不触发；用户断开 / 重开开关 / 模组消失都会清账，不会冷不丁重启模组。
`reset_before_dial` 默认关（复位会中断数据面 30 秒以上）。

**不移植**（本轮核对后主动放弃，均有源码依据）：QModem 的拨号凭据体系
（`MGAUTH`/`AUTHDATA`/QMI auth TLV + PAP/CHAP 互换重试）—— 本仓库 `[net]`
与 APN 参数都没有 user/pass/auth 字段，凭据面不存在，孤立的指令移植无处
落地；双 SIM 槽 apn2 回退（SIM 槽管理是独立功能面）；主线 `qmi_wwan` 的
`add_mux` 内核侧建口（本地 QMAP 路径面向厂商驱动 `qmi_wwan_q/f`、GobiNet、
`pcie_mhi`，BindMux 即通，改内核侧无真机可验）。

（`rust/src/runtime/dialer/qmi.rs`、`native/mod.rs`、`native/qmi_link.rs`、
`runtime/dialer/mod.rs`、`modem/model/device.rs`、`config/net.rs`、
`files/etc/modemd/config.toml`）

### 变更：退役内置 WebUI，前端只保留 LuCI

**背景**：1.0.0-3 定的口径就是「LuCI 验证成熟后 WebUI 可退役」（见历史条目
「WebUI/LuCI 共存口径定稿」）。LuCI 现已 8 大页面 + 10 项工具全功能覆盖并长期稳定，
按计划执行退役；`modemd` 从此只提供 `/api/*` 接口层，不再托管任何静态页面。

**删除**：

1. `rust/web/`（`index.html`、`mini.html`、`layouts/*.css`）—— 编译期嵌入的
   调试台页面资产整体移除；
2. `rust/src/http/static_ui.rs` —— 静态页托管模块（`include_str!` 嵌入 + 分发）随资产一起删除。

**改动**：

- `http/router.rs`：非 `/api/` 路径的兜底从「交静态页」改为直接 404（`/api/*` 未登记
  路径仍 404 不变）；`http/mod.rs`、`lib.rs` 的模块清单同步更新；
- `http/auth.rs`：放行名单不再包含「静态页」（非 `/api/` 路径本就无内容可放行），
  `require_luci_login` 语义不变 —— 仍保护 `/api/*`，LuCI 走 127.0.0.1 代理天然免登录；
- LuCI：删掉 TAB 栏右侧的「`WebUI ↗`」跳转按钮（`fmt.js` `renderTabBar`），随之删除
  概览页 / 主入口从 config.toml 解析端口、拼跳转 URL 的死代码（`overview.js`、`main.js`
  不再读 config.toml）；
- LuCI「服务控制」页指标「内置 WebUI 监听端口」改称「modemd API 监听端口」
  （端口本身保留：LuCI 代理仍按它转发）；
- `config.toml`、`README.md`、`docs/API.md`：同步口径 —— `[server] listen` 是 API
  监听地址，兼容性红线的同步对象只剩 `luci/`；
- 各文件注释里指向 `rust/web/index.html` 的引用改为后端统一口径的表述。

**不受影响**：`/api/*` 契约一字未动；WebSocket 推送（`/ws`、`/display/ws`）、
企业微信短信转发、会话鉴权、定时任务全部保留；历史配置里 `[ui]`、`[server]` 的
键完全兼容。

### 变更：CI 每次编译都发布 Release（手动触发不再只出 artifact）

**问题**：`build.yml` 的 release job 条件是 `startsWith(github.ref, 'refs/tags/v')`，
手动 `workflow_dispatch` 编译只把产物留在 artifact（14 天过期），拿不到可下载的
Release；实测 run #39（手动触发）三个矩阵档全部 success，但 Release 一条没建。

**改动**（`.github/workflows/build.yml`）：

1. **release job 不再限定标签**：条件改为 `always() && needs.build.result == 'success'`
   —— 无论标签触发还是手动触发都发布 Release；矩阵任一档失败则不发布（避免半成品）。
2. **发布目标按来源区分**：
   - 标签触发（`refs/tags/v*`）→ 同名版本标签，说明取仓库的 `RELEASE_NOTES.md`，
     `make_latest=true`（维持「最新版本」指向正式版）；
   - 手动触发 → CI 标签 `build-<run_number>`（不存在则按当前提交自动创建），
     说明按提交 / 分支 / 运行号 / 产物清单自动生成，`make_latest=false`
     （不抢占正式版的「最新版本」标记）。
3. **校验和改为合并后重算**：三个矩阵档各带一份 `SHA256SUMS.txt`，
   `merge-multiple` 合并时同名文件互相覆盖，最终只剩最后一档的清单，
   与实际发布文件对不上；现在按合并后的全部文件重算一次再上传。
4. **新增陈旧清理**（release job 末步）：既然每次编译都出 Release，Release 会越积越多。
   现在每次编译结束自动清理：删除**已结束** run 的 artifact（内容已随 Release 发布），
   并把旧的 CI Release（`build-*` 标签）只保留最近 3 个（`KEEP_CI_RELEASES`）；
   正式版本 Release（`v*` 标签）不动 —— 那是版本历史。需 `actions: write` 权限。
   实测首次清理：旧 run artifact 68 个全部删除。
（`.github/workflows/build.yml`、`docs/BUILD.md`）

### 修复：FM350 NR 驻网时信号质量页面 RSRP/RSRQ 系统性偏低（CESQ 误按 LTE 表换算）

**现象**：FM350-GL（F22 固件 81600.0000.00.29.24.02，实机 192.168.10.1）NR 驻网（n41）时，
「信号质量」卡片显示 RSRP=-103 dBm / RSRQ=-6 dB / SINR=25 dB，而「基站信息」页
`AT+GTCCINFO?` 服务小区行同时刻给出 RSRP=-79 dBm / RSRQ=-11 dB / SINR=25 dB ——
RSRP 偏低 24 dB、RSRQ 偏高 5 dB，SINR 反而正确。

**根因**：`parse_cesq`（`modem/model/signal.rs`）按「标准格 `<rsrq>`/`<rsrp>`（f[4]/f[5]）
非 255 → LTE 表」隐式判制式。该固件在 NR 驻网时标准格**不回 255**（实测恒为 28 / 38，
按 LTE 表换算恰为 -6 dB / -103 dBm，与页面显示逐值吻合），真正的 NR SSB 测量在广和通
扩展格 f[6..8]；旧逻辑因 rsrp≠0 永远走不到 SSB 分支，只在 SINR 缺格时才兜底发
`AT+GTCCINFO?` 补 SINR —— 这解释了「RSRP/RSRQ 错、SINR 对」的组合。

**修复**：`parse_cesq` 改为 **NR SSB 扩展格优先** —— `<ss_rsrp>`（f[7]）非 255 即认为
模组给出了 NR SSB 测量，三值全部取扩展格（与 `AT+GTCCINFO?` 服务小区行逐格一致，
实机验证）；扩展格缺失（LTE 驻网时全 255）才退回 LTE 标准格，制式判定由「猜」改为
「看扩展格在不在」。新增实机回归用例 `cesq_nr_serving_reports_lte_scale_fields`
（同时刻 CESQ + GTCCINFO 抓包交叉验证）；既有 LTE 样例行为不变。
（`modem/model/signal.rs`）

## [1.0.8] - 2026-10-24

### 修复：QMI/MBIM 模式下内置原生拨号后端显示「未运行」（对照上游 modem-pro 全量移植补缺）

「拨号上网」页的「内置原生拨号后端」卡片按 `/api/dial` 的 `host_dialer` 是否有会话
显示徽标；QMI/MBIM 模组上它长期显示「未运行」。逐项对照上游
[LianXia233/modem-pro](https://github.com/LianXia233/modem-pro) 的主机侧拨号链路
（`hostdial*.go`）后定位到四处缺口，全部按上游实现移植修复：

1. **`raw_ip` 在运行中的网卡上写不进（QMI 主根因）**。主线内核 qmi_wwan 的
   `raw_ip_store()` 对 `netif_running()` 的设备直接返回 `-EBUSY`
   （"Cannot change a running device"），而 netifd 的 `modem_pro` 接口把数据网卡
   常年拉在 up —— 原实现直接 `write(raw_ip, "Y")`，**每一次都失败**，会话起不来、
   5→60s 退避循环，页面永远「未运行」。移植上游 `prepareQMIWwanRawIP`：
   down → 写 → up，5 次 × 800ms 重试（覆盖模组刚重新枚举后的短暂 EBUSY 窗口），
   已是 Y 则不动、没有该属性（GobiNet/MHI）视为不需要。
   （`runtime/dialer/native/mod.rs`）
2. **缺「拉起前释放模组侧数据上下文」**（移植上游 `releaseHostDialDataContext`）。
   模组只有一个数据承载：固件自动连接、换卡/复位后的残留会话、用户在页面里
   激活过的上下文都会把它占住 —— 原生拨号器的 WDS Start / MBIM CONNECT 随后被模组拒
   （上游真机记录 QMUXError 0xe / CONNECT Status=2 FAILURE），会话永远建立不起来。
   现在每次拉起会话前对非 IMS 的激活上下文发 `AT+CGACT=0`（每个 cid 重试 3 次、
   间隔 1.5s，与上游 hostDialReleaseAttempts/Gap 一致）；手动拨号的交办路径同样先
   释放并记一步「释放模组侧数据承载」。释放失败不拦拉起（与上游一致，由退避兜底）。
   （`modem/model/dial.rs`、`runtime/dialer/mod.rs`）
3. **APN / PDP 管理缺「承载独占」闸门**（移植上游 `modemUsesHostDialer` 口径）。
   QMI/MBIM 模组上 `set_apn` 原来会「先断后连」重新激活上下文（`AT+CGACT=1`）并激活
   IMS —— 模组侧一激活就把唯一数据承载占住，把正在跑的原生会话顶掉；又因没有第 2 条
   的释放步骤，之后再也恢复不了。现在：`set_apn` 对 QMI/MBIM 只写配置、不激活
  （连 IMS 也不发），并在返回说明里写明原因（上游 apiSetAPN 同款 autoNote）；
   `set_pdp` 对 QMI/MBIM 只允许**停用**（上游 apiSetPDPActive 同款），激活/重连被拒。
   ECM/RNDIS/NCM 的行为不变。
4. **缺「会话在跑但一直没有地址」的掉线自愈**（移植上游 `hostDialRestartDue` 口径）。
   会话探活（QMI 包状态 / MBIM 激活态）之外，每轮再看链路事实：数据网卡无 IPv4 地址、
   或有地址但载波为 0，跑满 60s 就重开会话（两次间隔 120s、最多 10 次，拿到地址即清零；
   重开只停不原地起，下一轮拉起）—— 对模组重新枚举后 raw_ip 被复位这类「协议层已连接、
   承载早已废了」的情况能自愈。

测试：新增 6 个单测（PDP 激活闸门按类型分派、释放无上下文时空操作、自愈的
宽限/限速/限次/清零、`native_link_ok` 对不存在网卡返回 false、无 raw_ip 属性时
`ensure_raw_ip` 视为不需要）；`cargo test --locked` 805 passed（改动前全量基线同为
799+2 个与本改动无关的 `http::ws` 环境相关用例，单跑即过）；改动文件 clippy 0 新告警
（存量告警与基线完全一致）。`docs/INTEGRATION.md` ⑤b 同步。

## [1.0.7] - 2026-10-24

### 修复：内核 6.x 下一台模组被登记成多台（实机 Hiveton H5000M / ImmortalWrt SNAPSHOT / kernel 6.18.55 / RG520F-EB）

`discovery::ports::describe()` 假设 `/sys/class/tty/<tty>/device` 直接指向 USB 接口目录
（`2-1:1.2`），只取 `file_name()` 再按冒号切分。但内核 6.x 的 usb-serial 指向的是接口下的
**端口目录**（`2-1:1.2/ttyUSB2`），名字里没有冒号 —— `rsplit_once(':')` 返回 `None`，
函数提前返回，`usb_dev` / `iface` / `vid` / `pid` 全为空。

连锁后果：

- `PortInfo::group_key()` 退化成 `port:<路径>`，`plan()` 变成「每个串口一台设备」，
  一台两个 AT 口的模组被登记成两台，自动组网给同一张网卡建两个重复接口；
- `vid/pid` 为空 → `modem_ports.json` 里既有的 `2c7c:0800` 规则（`at_interfaces` 1.2/1.3）
  永远匹配不上。

真机日志：

```
发现模组 id=0 at=/dev/ttyUSB2 group=port:/dev/ttyUSB2 规则=未命中
发现模组 id=1 at=/dev/ttyUSB3 group=port:/dev/ttyUSB3 规则=未命中
```

两个拨号器随后抢同一个 `/dev/cdc-wdm0`，QMI CTL Sync 超时（service 0x00 message 0x0027），
始终拨不上号。

修法：新增 `split_usb_interface()`，从 canonical 路径逐级上溯找第一个
`<bus>:<配置>.<接口>` 形状的目录，返回 `(接口目录, bus, 接口号)`；`describe()` 用它取
`iface_dir` / `bus` / `iface`，`vid/pid` 从接口目录的父目录（USB 设备目录）读，
`driver` 优先取接口目录的（端口目录未必有 driver 链接）。两种布局都兼容，
非 USB 仍返回 `None`。

验证：新增 3 个单测（端口子目录 / 接口目录 / 非 USB）；`cargo test discovery` 全绿
（18 passed）。按真实路径推演，`1.0/1.2/1.3` → 同组 `usb:2-1` → 规则命中 →
`primary=1.2`、`fallback=1.3` → 只登记 1 台。

### 修复：350 系列「模组有 V6、接口没 V6」（实机 FM350-GL / 中国移动）

实机现象：网络卡片显示模组有一个 IPv6，`ip -6 addr` 里接口却只有 link-local，
`modem0_6` 一直 pending。抓下来是三个叠在一起的问题。

**① `AT+CGCONTRDP` 把 DNS 服务器当成了本机地址（根因）**

字段下标按 3GPP TS 27.007 §10.1.24（= 广和通手册 §12.2.10）是
`cid, bearer_id, apn, <local_addr>, <gw_addr>, <DNS_prim>, <DNS_sec>, …`。
原实现从第 3 位往后「扫第一个像地址的字段」，而 FM350-GL 的 `<local_addr>` /
`<gw_addr>` 两列常年是空串 —— 于是扫进了 DNS 列，页面把运营商那台 IPv6 DNS
显示成「模组本机 IPv6」。实测 `AT+GTDNS=1` 回的正是同一条 CGCONTRDP 里的那两个
地址，可确认不是本机地址。同一处还把 IPv4 的 DNS 服务器当成了本机
IPv4 显示。

- `netprov` 侧没有可配的地址，`usable_static_v6()` 又是「结构合法就收」，所以
  一旦用户把 v6 模式改成 `static`，这个 DNS 地址会被写成 `/128` 并配出一条
  永远不通的默认路由。
- 修法：只读 `f[3]`（`f[4]` 仅在「另一个地址家族」时收，否则那是网关），
  新增 `split_local_addr()` 处理 `地址+掩码` 连写（IPv4 8 段 / IPv6 32 段），
  DNS 列一律不参与。

**② 350 系列地址源改为 CGPADDR 权威、CGCONTRDP 只作兜底**

对齐参考实现 `luci-app-fm350` 的 `modem/pdp.rs`：该固件地址只在 `AT+CGPADDR`
里，`::1` 占位即「无 IPv6」，绝不回落到 CGCONTRDP 的残留值。此分支只对
`is_fibocom_350()` 生效，其它厂商的取值路径不变。

**③ v6 兄弟接口每 15 秒被拆一次，客户端永远跑不完一次交涉（实机 v6 反复重启）**

`should_kick` 原本「接口没 up ⇒ 立刻重拉」，而 `kick_sibling` 是 ifdown+ifup、
odhcp6c 的 soltimeout 是 120 秒。组网循环 15 秒一轮，于是日志表现成
「已运行 0s 仍无全局 IPv6（第 N 次重拉）」每轮 +1，重拉 60 次也没等到地址。

- 接口没 up 也走退避，只有第一次（`since_last` 为空）立刻拉起。
- 节奏定为 **15 秒 ×10 → 61 秒 ×10 → 180 秒（无限兜底）**：头 10 次快速重试覆盖
  PDP 刚激活 / RA 还没到的窗口；第二挡 61 秒略短于 odhcp6c 的 soltimeout(120s)，
  保证一次完整协商能跑完不被自己打断；之后每 3 分钟巡检一次、**不封顶** ——
  纯 IPv4 环境下接口拿不到地址也要保持巡检，运营商补开 v6 或换卡插回来才能自愈。

### 实机验证（FM350-GL，RNDIS，ImmortalWrt SNAPSHOT aarch64）

- 网络卡片：`ipv4=10.x`（真实地址）、`ipv6` 为空 —— 不再出现运营商 DNS 假地址；
- v6 重拉节奏 15s ×10 → 61s ×10 → 180s（不封顶）；
- IPv4 仍为 static（模组上报地址），上网正常。

> 该卡（cmiot5g）实测**没有 IPv6**：`AT+CGDCONT` 为 `IPV4V6`、`AT+CGACT` 已激活，
> 但 `AT+CGPADDR` 的 v6 位恒为 `::1` 占位；另建 `IPV6`-only 上下文激活同样只回
> `::1`。所以此机型上「模组有 V6」本身就是上面 ① 造成的误报。

## [1.0.6] - 2026-10-23

### 修复：350 系列 static IPv4 下 IPv6 接口不创建

`wan_mode=modem_pro`（默认）时，FM350 / RW350 的 v4 因模组内 DHCP 无响应会被转成
`proto=static`。此前只有 `dhcp` / `both` 模式才建 `modem<id>_6`，而 `modem_pro`
协议只在 `proto=modem_pro` 时运行 `mp-client.sh`（odhcp6c 在里面）—— 结果主接口
转 static 后既没有 odhcp6c、也没有 v6 兄弟接口，模组明明有 IPv6，主机侧却永远取
不到，表现就是「插件认为没有 IPv6、不建 v6 接口」。

- `netprov::plan_wan`：主接口为 `static` 时，只要 `wan_ipv6 != off` 就建
  `modem<id>_6`（`dhcpv6` / `ra` / `static` 三种形态照旧），不再依赖
  `wan_mode` 的兄弟接口开关；默认 `modem_pro` 且没有 static 覆盖时仍是单接口
  （mp-client 自己跑 odhcp6c），行为不变。
- 新增 `plan_wan` 回归测试：ModemPro + static v4 必须带 v6 兄弟接口（dhcpv6/RA
  两路），无 static 覆盖时保持单接口。

### 短信：MT5700 / FM350 深度适配

- **MT5700 / MT5710 发送改走 PDU 一次性形态**：`AT+CMGS=<TPDU 长度>` + 字面
  `\r`（反斜杠 + r）+ PDU + CR，整条一次写完。该固件不实现 `>` 提示符两段式，
  真 CR 只会静默。仅当模组在本地命令层回 `ERROR` / `+CME ERROR` 时退回标准
  两段式重试一次；`+CMS ERROR`（网络侧已拒）与超时（结果不明）一律不重试，
  避免重复投递。
- **MT5700 读短信改走 PDU 模式**（与 FM350 一致）：正文、号码、时间、UDH 分条
  都从 PDU 解出，不再把文本模式回的十六进制 PDU 当正文展示。
- **长短信按 UDH 拼接发送**：超长内容不再拆成互不相关的多条短信；统一 8 位
  参考号 + 连接头，GSM7 每段 153 septet（UDH 与正文共用比特流并正确填充）、
  UCS2 每段 67 个 UTF-16 单元，单段 TP-UD 上限 140 字节。
- **`AT+CPMS?` 六字段形态**：接受 `+CPMS: 3,50,3,50,1,30`（无区名）并回填真实
  用量；同时接受 `^CPMS:` 前缀。六字段下用刚下发的存储区记「已生效」，避免每次
  读短信都重切一次。
- **`+CMTI` 上报**：MT5700 / FM350 会话内首次碰短信时检查并补
  `AT+CNMI=2,1,0,2,0`（MT5700 掉电不保存，默认全 0），新短信仍能触发自动重读
  与推送；模组复位或切口后重新确认。
- LuCI 发送框的分段预估改为「首条 160/70，长短信每段 153/67」。

### 修复：350 系列 IPv6 判定早于模组拿到地址，且此后不再重识别 / 不再重拉接口

上一节只解决「static 主接口下不建 v6 接口」，还剩两个洞：

1. **判定时刻太早**：`modem<id>_6` 的建与不建取决于「这一轮有没有 IPv4 / 有没有
   v6 地址」，而 350 系列的 PDP 是模组侧晚于接口建立的，早那一轮判「没有」
   就永远不再建；
2. **判一次就不再判**：`AT+CGPADDR` 的 v6 位即使只有 IPv4 也会回一个无前缀残值
   （`::18df:...`），旧代码只在缓存为空时才重查，非空的残值把后续识别永久挡住；
   接口建起来后如果没拿到地址，也没人再碰它。

改法（三层各司其职）：

- **无条件建**（`netprov::V6Plan::dedicated`）：350 系列只要 `wan_ipv6 != off`
  就建 `modem<id>_6`，不再等 IPv4 地址、不再看 `wan_mode` 的兄弟接口开关。
- **不重复跑客户端**（新 UCI 选项 `mp_no_v6`）：主接口还是 `modem_pro` 且确有
  兄弟接口接手 IPv6 时写 `mp_no_v6=1`，`modem_pro.sh` 据此导出 `MP_NO_V6`，
  `mp-client.sh` 跳过 v6 上报脚本生成 / odhcp6c 启动与巡检，`accept_ra` 也不再
  被打开 —— 否则同一张网卡上两个 DHCPv6 客户端抢 RA，v6 接口反而一直空着。
  `wan_ipv6=off`（不建兄弟接口）时**不写**这个键：让位给一个不存在的接手方，
  等于这台机器彻底没有 IPv6。计划退回普通形态时 diff 会下
  `uci delete network.modem<id>.mp_no_v6` 清掉它（换非 350 模组不会永久少跑
  odhcp6c）。
- **运行期再识别 + 重拉**（`netprov::v6::iface_live` / `maybe_rekick`）：已符合
  计划的轮次里读一次 `ifstatus modem<id>_6`（`up` / `uptime` / data 里的
  `ipv6-address`、`ipv6-prefix-assignment`、`ipv6-prefix`），接口 up 满 20 秒仍没有
  全局地址或委派前缀 ⇒ `ifdown`+`ifup` 让它重发 RS/DHCPv6，退避
  30/60/120/240/300 秒封顶；拿到 v6 立即清零计数。仅对 350 生效，不动别的机型。
- **地址缓存不再挡路**：`query_own_ipv6()` 现在同步回写 `DialState.ipv6`
  （原来只写旁路缓存，而 `dial_state_ipv6()` 先读状态 —— 旧值永远盖住新结果）；
  组网侧判据从「缓存为空才重查」改成「缓存不是可用全局地址就重查」。

### 短信：四项补强（号码归一化 / CSMP 复位 / 主动 arm CNMI / 状态报告不占位）

- **收件号码归一化**（`sms::normalize_destination`）：剥空格与 `-()`、认手写的
  `00` 国际接入码、不带 `+` 的 `8613800138000` 仍按国际格式、裸写的 11 位手机号
  自动补 `86`、TOA 取 `0x91`/`0x81`、长度限 2~20 位。此前 `+86 138-0013-8000`
  这类常见输入会被判成国内格式并把分隔符当地址字符，文本模式同样先剥分隔符。
- **`AT+CSMP` 复位**：文本模式发英文那条分支补 `AT+CSMP=17,167,0,0`。
  TP-DCS 是模组全局状态，上一条中文把它设成了 `8`（UCS2），不复位时
  这条英文短信会按 UCS2 发出去，对方收到乱码。
- **主动 arm `+CMTI`**：URC 循环每轮调 `ensure_sms_urc()`，打破「没 arm 就没有
  `+CMTI`、没 `+CMTI` 就没人读短信、没人读就永远不 arm」的死锁 —— MT5700 掉电
  后 CNMI 回到全 0，此前重启后新短信既不进页面也不推微信，直到用户手动点开短信页。
  arm 只在 MT5700/FM350 上做，确认成功（已是 `2,1,0,2` 或 set 回 OK）才置
  `sms_urc_ready`，未确认按 30 秒限速重试，不会每秒占串口；顺带 best-effort
  `AT+CMEE=2` 让失败原因是可读的 `+CME ERROR: …` 而不是裸 `ERROR`。
- **状态报告不再产生鬼条目**：`parse_pdu_list` 改成「头行先缓存、正文解成功才落地」，
  已发送区里的 SMS-STATUS-REPORT（MTI=2，本解码器不认）不再留下一条
  没号码没正文的空短信；`parse_pdu_concat_list` 增加 MTI 判定，避免按
  DELIVER 布局去读状态报告的 UDH 而凑出假的「第 1/2 段」。
- **LuCI 存储区显示如实化**：`AT+CPMS?` 只回六个数字（无区名）时，
  徽章显示「未报区名」而不是硬猜 `SM`，下拉框加一条禁用占位并挡住空值提交。

测试 786 → 794（dedicated 兄弟接口与 `mp_no_v6` 的建/清、`diff_plan` 零写入、
`ifstatus` 的 v6 判定、重拉退避与门禁、号码归一化、松散号码出同一份 PDU、
状态报告被丢弃）；`cargo build --release` 0 warning，`sh -n` / `node --check` 通过，
`scripts/check-desensitize.sh` exit 0（286 文件；新增测试样例里的运营商前缀
已换成 RFC 3849 文档段）。未实机验证：350 系列 `mp_no_v6` 下主接口 IPv4 与
兄弟接口 IPv6 并行、`maybe_rekick` 在真实 RA 延迟下的重拉次数（本沙箱无模组与
netifd）。`cargo fmt --check` / `cargo clippy` 因发版环境缺组件**未执行**，
详见 [1.0.6] 发版说明的已知边界。

## [1.0.5] - 2026-10-21

### 新增：运营商感知的 APN 自动配置（识别 → 匹配预制 APN → 确认生效 → 再拨号）

部分模组**必须显式下发 APN** 才能拨号：模组里没有可用上下文时，自动拨号「沿用模组
配置」拨上去就是不通，而页面上明明显示着运营商名称 / PLMN 甚至一个在用 APN
（`AT+CGCONTRDP`）—— 之前没有任何路径把这些信息用于拨号。逐项依据见
`docs/DIAL_STATE_MACHINE.md` §11：

- **识别**（`model/carrier.rs`，纯表驱动，四级证据）：
  运营商名称（中英文变体：CMCC / China Mobile / 中国移动 / CHN-CT / CHN-UNICOM /
  中国广电…）→ PLMN（MCC 460 + 在网 MNC：移动 00/02/04/07/08、联通 01/06/09、
  电信 03/05/11、广电 15）→ ICCID 发卡行前缀（只作兜底）。
  名称与 PLMN 冲突以 PLMN 裁决；全失败 ⇒ 保留现状**绝不**动现有 APN；
  `COPS mode=2/3`（注销/被拒）直接判识别失败。
- **决策**（`model/apn_auto.rs::decide`，纯函数）：用户显式配置永远最高；
  模组上下文与识别运营商同家 ⇒ 沿用（跳过写入）；是别家预制 APN（插过别家卡 /
  运营商已切换）且识别足够强 ⇒ 校正；**自定义 APN（企业卡/专用套餐）绝不覆盖**；
  模组无上下文但 `CGCONTRDP` 有在用 APN ⇒ 直接用；都没有 + 识别成功 ⇒ 下发
  预制 APN（移动 `cmnet` / 联通 `3gnet` / 电信 `ctnet` / 广电 `cbnet`）。
- **落地（不假设所有模组用同一种 APN 设置方法）**：
  AT 路径（ECM/RNDIS/NCM）在拨号线程内识别 → 按需 `AT+CGDCONT`（`set_apn`
  幂等闸门 + 回读确认，回读成功才记「已生效」）→ 才拨号；QMI/MBIM 走拨号器
  参数（WDS Start / MBIM CONNECT），APN 直接进计划、会话建立即生效 ——
  协调器线程**不发 AT**，只用采样循环顺手缓存的运营商证据（10 分钟有效）。
- **换卡 / 切换**：SIM 更换作废「自动下发」记录与证据缓存，下一次自动拨号按新卡
  重新识别校验；过程全记进页面拨号步骤（「运营商识别」「APN 自动配置」）。
- 手动拨号表单 `cid/apn/pdp_type` 全空时同样走这条链（任一项显式填写则完全不介入）；
  映射表四张全部集中在 `carrier.rs`，补充运营商变体改表即可，不在调用点写 if。

测试 764 → 777（新增 13 项：四级识别、冲突裁决、ICCID 兜底、优先级表逐条、
IMS 不算可用上下文等）。未实机验证：各固件 `COPS`/`QNWINFO` 变体的识别表现、
`AT+CGDCONT` 写入后部分固件的生效延迟、广电实卡（46015 / 898615 按公开资料整理）。

## [1.0.4] - 2026-10-21

### 优化：接口默认下发 DNS 改为「v4 首选/备选 + v6 首选/备选」四条

`[net] wan_dns` 的默认值从 `223.5.5.5 119.29.29.29`（只有两条 v4）换成：

| 族 | 首选 | 备选 |
| --- | --- | --- |
| IPv4 | `119.29.29.29`（DNSPod） | `223.5.5.5`（阿里） |
| IPv6 | `2402:4e00::`（DNSPod） | `2400:3200::1`（阿里） |

- 顺序即优先级；两家都是 Anycast 双栈，任何一族不可用都有同族的备选顶上。
  选国内而不选海外公共 DNS：蜂窝出口在国内，延迟与劫持风险都更低。
- netifd 的 `interface_add_dns_server()` 对 v4 / v6 是同一个入口，IPv6 地址无需特殊写法。
- **`wan_ipv6 = false` 时自动不下发 v6 那两条**（新增 `netprov::effective_wan_dns`）：
  链路没有 IPv6 时写进 `resolv.conf.auto` 只会多两个永远不回应的 nameserver。
- 默认值收敛到 `config::net::DEFAULT_WAN_DNS`；`config.toml` 里**故意写成注释模板** ——
  它是 conffile（升级不覆盖），写死会让老机器永远拿不到新的默认值。没这一行也按默认值下发。
- `GET /api/dial` 新增 `wan_dns` 字段（本次实际会写进 `network.modem<id>.dns` 的那串），
  页面「自动组网报告」卡片加一行显示，排障不必翻配置文件。

### 修复：第二轮拨号/接口自检（5 项，详见 `docs/DIAL_STATE_MACHINE.md` §10）

从头走了一遍「模组插入 → 自动拨号 → 接口拉起 → 地址/DNS 落地」，统一状态机本身没再发现
设计问题，问题都在它的**接入点**与**接口侧**：

1. **同步接口 `/api/dial/connect` 拨 QMI/MBIM 后不 kick** —— 只有异步任务 `dial.connect`
   调了 `kick()`，页面上点「拨号」要干等最多 15 秒才从 `dialing` 变 `connected`
   （§3.4 写的行为没落地）。给 `DialerSupervisor` 加 `arm(Arc<AppState>)`，
   `kick()` 不再需要入参（同步 handler 只拿得到 `&AppState`），`spawn_all` 装配后挂上。
2. **接口 `device` 会漂到别的网卡** —— `pick_device` 的排除名单把**本模组自己的接口**
   也算「被占用」：第一轮写完 `modem0.device=wwan0` 后，第二轮起 wwan0 被自己占着 ⇒
   跳过 ⇒ 落到全局兜底 `detect_data_dev`，随手抓一张 `eth2` 或**另一台模组的** `wwan1`
   ⇒ diff 下发 `set modem0.device=…`，接口漂走直接断网。新增 `occupied_by_others()`。
3. **网卡还没枚举出来就建了一张没有 `device` 的接口** —— netifd 判 `NO_DEVICE`，
   `modem_pro.sh` 随即 `proto_block_restart`：一张起不来、还会挡后续 ifup 的坏接口。
   新增 `skip_without_device()`（接口已存在时仍沿用现状，不把已有配置晾着）。
4. **从未拨号的 QMI 模组点「断开」失败** —— `hangup` 用 `cached_dial_kind()`，
   没拨过号时 kind 是空串 ⇒ 走 AT 分支发 `AT+CGACT=0` ⇒ 模组侧没我们激活的上下文、
   指令被拒 ⇒ 状态不置 `idle`、也不置 `manual_stop`，协调器下一轮又把会话建回来。
   改用 `ensure_dial_kind()`。
5. **断开后页面又跳回「已连接」** —— `dial_status` 只看「网卡有没有地址」，
   而断开后旧地址要等 netifd 收尾才掉，期间 `idle` 被改判成 `connected`。
   改判（③）与补地址（②）都加 `!manual_stop` 门禁。

顺带把「拨号器与组网必须拨/绑同一张网卡」从注释落成机制：组网每轮
`remember_device()`、拨号器改走 `device_for()`（模组回收时 `forget_device`，
避免旧网卡名留给复用 id 的下一位）。两边入参不同，QMAP 下算错就是父口永远 NO-CARRIER。

测试 759 → 764（新增 5 项：占用名单不含自己、空 device 保护、DNS 的 v6 裁剪与默认值一致、
拨号器跟随组网选中的网卡）；clippy 0 warning、`cargo fmt` 干净、release 构建通过。
实机待验（本沙箱无模组）：多网卡机器上接口 device 的稳定性、点拨号 ≤3 秒 connected、
从未拨号的 QMI 模组的断开行为。

### 新增：MT5700M-CN 私有命令链（信号 / 流量 / URC）与 FM350 双卡预设

参考 [LianXia233/luci-app-mt5700](https://github.com/LianXia233/luci-app-mt5700)（GPL-3.0，仅参考未拷贝）与两份厂商 AT 手册
（MT5700M-CN V200R001C20、Fibocom FM350 v2.2），完整审计记录见
`docs/UPSTREAM_AUDIT.md` §11：

- **鼎桥信号三值**：`Modem::signal()` 对 `Vendor::TDTech` 先走私有链 ——
  `AT^MONSC`（dB 直出）+ `AT^HCSQ?` 补缺项。新增 `parse_hcsq()`：
  LTE `<rssi>,<rsrp>,<sinr>,<rsrq>`（RSRP 下标 2）、NR `<rsrp>,<sinr>,<rsrq>`（下标 1）；
  SINR 下标 0..251 步长 0.2 dB（`0`=<-20、`251`=≥30），RSRP 步长 1 dB、RSRQ 步长 0.5 dB 三表分开；
  **255=未知不与 0 混同**。此前 MT5700M 只有 `AT+QENG` / `AT+CESQ` 一条路，而这两条在它身上全回
  ERROR，信号恒为空。
- **鼎桥流量**：`AT^DSFLOWQRY`（6 字段 hex，取 total 组）；默认的 `QGDCNT` / `GTSTATIS`
  在 MT5700M 全回 ERROR，`vendor/tdtech.rs` 覆写 `traffic_candidates()` 只发这一条。
- **URC 过滤**：`^RSSI` / `^CERSSI` 进入 `PRIVATE_URC_PREFIXES`（`AT^CURC` 默认全开）。
  `^HCSQ` 绝不能入表——它既是 URC 又是 `AT^HCSQ?` 应答，安全边界记录在 `urc.rs` 模块 doc。
- **FM350 双卡**：`at_presets` 新增 `AT+GTDUALSIM=0` / `=1` / `?`（0=SIM1 默认、1=SIM2，
  持久且立即生效）；4 条 FM350 的 `usb_at.note` 注明 `AT+GTUSBMODE` 40/41 同属 RNDIS 族
  （40 少 AP LOG/META 口）。

测试 754 → 759（`signal.rs` ×3、`traffic.rs` ×2；`urc.rs` 扩既有用例）。
`AT^NDISSTATQRY` 连接态对账探活设计在案未实施（§11.4：手册口径 `<cid>,<stat>`，cid 在前）。

### 重构：拨号生命周期统一到一个协调器（AT 与 QMI/MBIM 共用意图、状态与退避）

**问题（都有代码证据，详见 `docs/DIAL_STATE_MACHINE.md`）**：

1. QMI/MBIM 会话建好后**没有任何人**把 `DialState.state` 写成 `connected` ——
   `dial_inner` 对这两类只做「交办」就返回，页面永远停在「拨号中」；
2. 两套互不知情的退避：`spawn_auto_dial` 固定 300 秒（仅 AT），
   `reconcile_native_one` 5→60 秒指数（仅原生）；
3. 两套「该不该连」的判定各写一遍，新插入的 QMI 模组第一轮会被自动拨号
   当作 AT 模组喂给 `dial()`（`cached_dial_kind` 为空时）；
4. **没有代次**：拨号进行中点「断开」，旧流程醒来会把状态写回 `connected`
   （且与 `manual_stop=true` 并存）；
5. 模组重启 / 飞行模式 / 配额断网都不碰拨号状态：重启后状态停在 `connected`，
   自动重拨永远不触发；飞行模式下还会继续尝试重拨并退避到 300 秒。

**改造**（适配器式统一，不重写任何后端）：

- `Modem` 新增**连接代次** `dial_epoch`：`dial_inner` 开始时取号，每次「宣布成功」
  前比对；`hangup` 第一步 bump（作废在途拨号）；原生后端建会话前记代次、
  建好后比对，过期立即拆会话。只挡提交，不打断在途 AT（发出去的字节收不回来）。
- `DialerSupervisor` 升级为**唯一决策点**：`reconcile` 每 15 秒（及 `kick` 立即触发）
  对每台模组二选一 —— 原生走 `Session::start`，其余走 `auto_connect_allowed` +
  工作线程 `Modem::dial`（不再阻塞组网循环）。
- **统一退避表**（每模组一条）：AT 30/60/120/240/300s（持续前置条件），
  原生 5/10/20/40/60s（瞬时故障）；同一错误持续退避，错误变化清零，成功清空。
- **状态写入点收敛**：原生后端通过 `note_native_up/down` 写权威状态；
  策略停用通过 `note_stopped` 落 `disconnected`；外部断链（重启/飞行模式）通过
  `note_external_break` 拉回 `dialing` 并作废在途流程。
- 删除 `http::loops::spawn_auto_dial`（30 秒独立决策循环），线程数 8→7。
- 开关/重启/飞行模式 handler 接入：打开开关、关飞行模式、重启后清退避
  （不必等完一个已无意义的 5 分钟窗口）；`dial.connect` 任务成功后立即 `kick`。
- `DialState` 新增可选字段 `backend`（`at` / `native-qmi` / `native-mbim`），
  其余字段与语义不变。

**兼容性**：HTTP 路径、JSON 字段名、`state` 字符串、厂商配方、`AT+CGACT` 禁用
约束、FM350 静态地址、netifd/防火墙/metric 逻辑全部未变；`backend` 为可选新增字段。

**测试**：`cargo test --locked` **746 passed**（新增 6 个用例：统一退避序列、
退避清零规则、自动连接的全部拒绝理由、断开时代次作废、原生状态写入与掉线回退、
外部断链作废在途拨号）；clippy 0 warning、fmt 干净。真机验证清单见
`docs/DIAL_STATE_MACHINE.md` §6.2（本沙箱无模组，未执行）。

### 修复：5G NR 锁小区命令参数整体错位（QNWLOCK PCI/频点/SCS 顺序）

**现象**：页面上对 5G 模组「锁小区」选 NR、填频点 + PCI 下发，模组回 OK 但锁不上
（或锁到一个莫名其妙的小区）；`GET /api/lock/status` 回读也看不懂锁的是什么。

**根因**：移远 QNWLOCK 的 4G 与 5G 是**两套语法**。4G 是
`AT+QNWLOCK="common/4g",<lock_type>,<earfcn>[,<pci>]`，而 5G 是
`AT+QNWLOCK="common/5g",<pci>,<arfcn>,<scs>[,<band>]` —— **没有 lock_type，
且 PCI 在最前**（依据 MikroTik ROS 文档实测回读样本
`+QNWLOCK: "common/5g",138,628032,30,78`）。原实现把 4G 的
`1,<earfcn>,<pci>[,<scs>]` 原样搬到了 5G 上，在 5G 固件眼里变成
`PCI=1、频点=<earfcn>、SCS=<pci>`，最后一个可选格还串到 band 上——
每个参数都错一位。

**修复**（`rust/src/modem/model/lock.rs`）：

- `parse_qnwlock` 新增 NR 分支：PCI 取第 1 格、频点第 2 格、SCS 第 3 格，
  与 4G 分支（lock_type 在前）彻底分开；
- `set_cell_lock` 新增 `band` 参数，NR 命令改为
  `AT+QNWLOCK="common/5g",<pcid>,<earfcn>[,<scs>][,<band>]`；
- **NR 只接受 `lock_type=1`（频点+PCI）**：5G 语法没有「仅频点」档位，
  保存定时任务时就在校验层拦掉（`model/schedule.rs`），不是等到发命令才失败；
- `band` 留空时用**扫描缓存里的驻网频段**兜底（`cached_serving_band`：
  只读 `cells_cache()`，60 秒内新鲜，**不发 AT**）——QNWLOCK 5G 语法第 5 格
  是 band，缺了它部分固件不认；
- 回读解析同步修正，`unlock_all` 等调用点补齐签名。

全链路同步：`http/handlers/lock_cell.rs` 收可选 `band` 入参；
`luci/app/js/view/modemd/lock.js` 的「仅频点」选项在 NR 时禁用并提示、
新增 5G 频段输入框（仅 NR 显示）、提交前拦截 NR+ltype=2；
`docs/API.md` 的 `/api/lock/cell` 契约行更新。新增测试
`qnwlock_parse_nr_puts_pci_first`（含 PCI=0 合法值场景）。

> 注：QModem 脚本里另有按平台分的 QNWLOCK 变体（unisoc 用 `common/lte` scope、
> `lte` 平台的 lock_type 1/2 语义与文档相反），单一来源不可证，本轮不改，
> 按「未验证不盲写」保留现状并在代码注释里声明。

### 对齐上游：华为展锐挂断前先探自拨状态（QModem `auto_dial_hang_huawei_unisoc`）

QModem `usr/share/qmodem/modem_dial.sh:1546-1568` 的华为展锐（海思平台）
挂断流程是**先问 `AT^SETAUTODIAL?`**：模组开了自拨时走 `AT^NDISDUP=0,0`，
没开自拨时不能碰射频、要退通用路径。原实现对所有厂商一律直发挂断指令，
在「未开启自拨」的海思机型上会把射频状态打乱。

**修复**：`vendor/datacall.rs` 新增 `AutodialProbe { query, fallback }` 挂在
`DataChannel` 上（10 个厂商文件 12 处字面量同步补 `autodial_probe: None`）；
`vendor/huawei.rs` 的 unisoc 分支填
`query="AT^SETAUTODIAL?"` / `fallback="AT^NDISDUP=0,0"`；
`model/dial.rs` 的 hangup 路径先发 query，仅当应答含 `SETAUTO` 且 `:1`
（自拨开着）才走厂商配方，否则整组步骤替换为 fallback 并记 `dial_step`。
新增测试 `huawei_unisoc_probes_autodial_before_hangup`（含「其它厂商
不被波及」的遍历断言）。

### 对齐上游：发现退避的「年轻期」失败不计入退避（Go `nonATNextProbe`）

Go 原版 `module/kernel/modem_serial.go:307-343` 的非 AT 口退避分两段：
设备刚出现（年轻期）失败不积累，出预热期后才从基值重数。原实现把年轻期
的失败也计入 `tries`，一个新插入的慢启动模组如果头几轮没应答，一进
成年期退避就直接跳到很高的档位。

**修复**（`discovery/scanner.rs`）：`tries` 改为「年轻期恒为 1，
出预热期从基值重数」。新增测试
`young_failures_do_not_leak_into_adult_backoff`（4 次年轻失败后 tries
仍为 1，成年失败后才累加）。

### 重构：9 处重复实现收敛为一处

| 重复点 | 原来 | 现在 |
| --- | --- | --- |
| sysfs 小工具 | `usbdev.rs` / `pcidev.rs` 各存一份 `read_sysfs_line` / `read_sysfs_id` / `is_dir_or_symlink` 等（且已开始漂移） | 共享 `modem::sysfs`（185 行含 3 个测试；usbdev −110 行、pcidev −23 行） |
| JSON 台账读写 | 6 个 store（quota / imei / atport / atport_bind / sched / hostnic）各写一遍「读文件→解析→坏账降级」「tmp+rename」 | `runtime::store::{load_json, write_json_atomic}` |
| 速率文案 `fmt_bps` | `ifstat.rs`（主机侧）与 `model/traffic.rs`（模组侧）两份实现 | 只留 `model::fmt_bps`，ifstat 复用（文案必须一致，有测试钉住） |
| 锁频段入参 | `lock_band.rs` 与定时任务 `lock.band` 各解析一遍 `bands` | `handlers::{parse_bands, band_lock_params}` |
| IMEI 收尾 | `imei_set.rs` / `imei_restore.rs` 各写一遍「落备份→回状态→回包」 | `handlers::imei_done` |
| 串口写字节 | `at/command.rs` 两个写入口各写一遍 `write_all`+flush | `AtPort::write_bytes` |
| `DialRequest` 构造 | `dial_connect.rs` 与 `tasks.rs` 的 `dial.connect` 分支口径各一份 | `dial_connect::dial_request_from_params` |
| 会话校验 | `auth.rs` 与 `ws.rs` 各判一次 ubus session | `auth::session_valid`（ubus 不可用时统一放行 + warn，防认证半边缺口） |
| 死代码 / 未用变量 | `ca.rs::empty_note`、`mbim.rs` 的 `v6_gw_off`、`params.rs` 的 `was_active` | 删除 |

### 注释：上游依据落到行号，有意差异显式声明

- **上游引用精确化**：`model/dial.rs` 原来引用的 `cmds/modem_dial.sh:454-481`
  指向的是 14 行 helper 存根，实际依据在 `usr/share/qmodem/modem_dial.sh`——
  已改并核对（`check_dial_prepare` = `:570-624`、`at_dial` = `:1285`、
  `AT+COPS=0,0` = `:1291`、华为探针 = `:1546-1568`）；
- **逐字段补上游依据**：`netprov/mod.rs`（Go `netprov_proto.go:102-111`）、
  `model/schedule.rs`（Go `sched_view.go:110-145` 六个常量逐个对行号）、
  `model/cells.rs` / `model/ca.rs` / `model/signal.rs`（Go `cell.go` 的
  `ParseServingCell` / `FindRSRPToken` / `ParseCESQSignal` / `cell.go:36`）、
  `model/network.rs`（Go `net.go:35/157/651/667-678/679-687/773`）、
  `model/params.rs`（Go `data_validate.go:23/43`）、`netprov/uci.rs`
  （QModem `modem_dial.sh:690-708` 的 zone 名还原）、`dialer/mbim.rs`
  （MBIM 1.0 §10.5.29 偏移量表）、`runtime/store.rs`（六本台账的并发 /
  降级 / 原子性约定）；
- **有意差异写进注释**（不擅自改口径，改一行全体机型的取值就变）：
  CESQ 换算与上游差 1 dB（取下界不上界）、QENG 服务小区信号用固定列
  不随上游改量纲定位、`wan_mode` 没有上游的 `modemProtoOK` 开关、
  QCAINFO 带宽列无量纲手册不猜、拨号不发 `AT+COPS=0,0`（会冲掉手动锁网）
  与 `AT+QCFG="nat"`、体检只读 `AT+CFUN?` 不补 `=1`（飞行模式是显式用户动作）；
- 顺带把 `NetworkStatus` / `CellInfo` / `SignalInfo` 等对外 JSON 的
  **每个字段**补了量纲与「0 = 未上报」口径说明。

### 质量：fmt / clippy / 测试全绿

- `cargo fmt`：本批改动文件（含脚本注入的代码块）全部统一；
- `cargo clippy --all-targets --locked`：**0 warning** —— 清掉 4 条存量
  （`cells.rs` 的 doc 注释空行 / 懒连续、`route.rs` 空串比较、
  `siminfo.rs` map 恒等、`dial.rs` manual_contains）；
- `cargo test --locked`：**740 passed / 0 failed**（起点为 HEAD 的 735；
  新增 7 个：sysfs 模块 3、华为探针 / 退避清零 / QNW NR 解析 / fmt_bps
  单位各 1；删除 2 个：随 `parse_bands` 去重移除的重复用例
  `bands_accept_json_and_colon`、fmt_bps 搬迁后遗留的空测试桩）；
- 全部改动文件 LF 换行（仓库红线）。

### 新模组：百旺 QDC507（LTE 移远方言 + 通用 CGACT 拨号）

来源 [ainuoyan/QDC507-Modem-Support](https://github.com/ainuoyan/QDC507-Modem-Support)
@ `bb57bb7`（GPL-3.0，**只取事实依据、按指令重实现，未复制代码**）。实机口径：
VID `2c7c`、实机 `2c7c:0125`、型号 `QDC507GLEFM21`、ECM + DHCP 供址、
拨号只有 `AT+CGACT=1,1`。

- `vendor/baiwang.rs`（新增）：识别关键字 `QDC507` / `BAIWANG`，**排在移远之前**
  （同 VID `2c7c`，ATI 可能只回 Quectel，CGMM 先给出型号）；`data_channel`
  不覆盖，走 3GPP 通用 `AT+CGACT`；`usbnet` 读走 QCFG 方言、写只认 ECM
  （参考实现 `modes=["ecm"]`）；`nwscanmodeex` 位标志读写
  （2G=4 / 3G=8 / 4G=16）+ 读回轮询；能力面 12 条只读 AT 预设，
  IMEI 写入按参考实现口径置 off。
- 数据表：`modem_catalog.json` 追加 `qdc507` 扩展条目（ecm-only、`vid:2c7c`）；
  `modem_ports.json` 追加 `rg501q-eu` 规则（`2c7c:0800`，AT 口按接口号
  1.2/1.3，参考实现 `common.sh` 的接口路径匹配；上游 QModem 端口表无此条，
  属有依据的 documented extension）。
- `vendor_is_exclusive("2c7c")` 翻 **false**：QDC507 与移远共享 VID，
  VID 只能当提示、不能钉死厂商；识别测试随之更新
  （`2c7c:0125` 空 ATI → unknown + candidates；CGMM 报型号 → baiwang）。
- hostnic 特例排除：QDC507 的 ECM 由模组 DHCP 供址，明确**不进**
  FM350/RW350 的静态地址路径（测试钉住）。

### 对照 OpenVohive 补 QMI 会话边界：IPv6 运行时设置回填 + TLV 编号裁决

来源 [6106757-lab/openvohive](https://github.com/6106757-lab/openvohive)
@ `6c1cc74`（PolyForm Noncommercial，只作协议行为基准），并用 libqmi
`qmi-service-wds.json` / `qmi-service-wda.json` 与厂商 C 拨号器
（quectel-CM、Fibocom-dial）仲裁编号分歧。逐条结论见
`docs/UPSTREAM_AUDIT.md` §8。

- **实改 1 处**：IPv6 呼叫成功后按同一 mask 向 `SVC_WDS_IPV6` 再取一次
  Get Runtime Settings，回填 `ipv6`/`ipv6_gateway`（原来只取 v4，状态页
  `modem_ipv6` 永远为空）。依据：quectel-CM `requestGetIPAddress()` 对
  两个 client 分别请求，且注释明示 "the following mask also applies to
  IPV6"；失败仅 debug 日志，不影响拨号结果。
- **TLV 编号裁决**：`wds_start` 的 `0x30`=Technology Preference（uint8，
  `[0x01]`=3GPP）、`0x31`=Profile Index 3GPP、`0x16`=Authentication
  Preference、`0x19`=IP Family 与 libqmi / quectel-CM / Fibocom dial
  三处一致；OpenVohive 把 Profile Index 写在 `0x30`、另发 `0x34` 扩展
  技术偏好，与三处冲突，**不采纳**（记录在案）。
- Get Runtime Settings mask `0x2310` 精确对应 quectel-CM
  `WdsGetRuntimeSettingReq()` 四位（PCSCF 两位有意不发）；WDA Set Data
  Format 与 Bind Mux Data Port 的 TLV 顺序/常量与 quectel-CM 逐条对上；
  聚合门限 `0x19`/`0x1B`/`0x1C` 仅在其 `QUECTEL_UL_DATA_AGG` 下出现，
  本项目同样不发（注释已记录依据）。
- `wds_runtime` 注释改为精确描述（不再笼统写"与 quectel-CM 相同"）。

### 对照 Siriling 多驱动补齐两处识别缺口（RMNET / GobiNet / MHI / PCIe）

来源 [Siriling/5G-Modem-Support](https://github.com/Siriling/5G-Modem-Support)
@ `b1d9a12`（GPL-3.0，只取事实依据）。逐条对照结论见
`docs/UPSTREAM_AUDIT.md` §9。

- **认 `fibo_mhi` 驱动**：`PCIE_MODEM_DRIVERS` / `pci_driver_is_mhi` 新增
  `fibo_mhi`（广和通厂外树 MHI，模块名既不 `mhi*` 也不是 `pcie_mhi`，
  原来会被当无线网卡漏掉）。
- **认 Fibocom MHI 数据网卡名**：`is_likely_mhi_face` 新增 `pcie_mhi*` /
  `pcie_swip*` 前缀（`mhi_netdev.c` 的 `interface_name`）。
- `qmap_size()` 注释校正：qmi_wwan_q / qmi_wwan_f 只暴露**每网卡**
  `qmap_size` 属性（USB ID 表取值，USB3 缺省 16 KiB），原注释所称的
  「模块参数」在两份源码里均不存在，该路径降级为防御性保留。
- siriling 自带的 fibocom-dial 与 §8 核对过的另一份拷贝 TLV 编号完全一致，
  QMI 编号结论再获独立佐证。
- SRPD / 展锐 PCIe（`sprd_pcie`、VID `0x16c3`、sipc 栈）无 QMI/MBIM 控制面，
  **不做**（记录在案）；GobiNet / qmi_wwan_q / qmi_wwan_f / `rmnet_mhi0`
  命名均为已覆盖项。

**质量**：`cargo fmt --check` 干净；`cargo clippy --all-targets --locked` 0 warning；
`cargo test --locked` **754 passed / 0 failed**（新增 1：Fibocom MHI 网卡名识别；
OpenVohive 批次的 v6 运行时回填并入既有端到端用例）；`cargo build --locked` 通过。
`docs/SUPPORT_MATRIX.md` 同步更新：92 型号 / 13 厂商、qdc507 入表、
新增「支持程度五级」（收录 / 识别 / 后端 / 自动化 / 实机；实机级 0）。

### 文档：首页 README 与各文档现状对齐

首页此前停留在「4 家厂商」与合并初期口径，与实际代码脱节。本次同步：

- 开头改为 **13 家厂商 / 92 型号**口径（数字由 `modem_catalog.json` 统计得出），
  并链接支持矩阵五级说明；
- 「特色亮点」补两条：统一拨号生命周期协调器（链 `DIAL_STATE_MACHINE.md`）、
  754 项自动化测试；
- 「文档」节补 `DIAL_STATE_MACHINE.md` 条目，`UPSTREAM_AUDIT.md` /
  `SUPPORT_MATRIX.md` 描述补 §8–§9 裁决与五级支持；
- 「致谢」表补齐 `docs/UPSTREAM_AUDIT.md` §10 的 4 个参考仓库
  （openvohive / luci-app-rg501q-eu / QDC507-Modem-Support / 5G-Modem-Support），
  各标注许可与「仅参考未拷贝」；许可证段落改为「许可见上表与 §10」，
  替换原来「上游仓库未声明开源许可证」的过期表述（QModem 为
  MPL-2.0 + 禁止商用附加条款，openvohive 为 PolyForm Noncommercial 1.0.0，
  另两个为 Apache-2.0 / GPL-3.0，均只作事实依据）。

## [1.0.3] - 2026-10-20

### 文档：补齐上游致谢（只列参考过的仓库）

README「致谢」段补入 [FUjr/QModem](https://github.com/FUjr/QModem)（模组知识底座：
型号清单、端口白名单、PCM 接口排除、厂商 AT 方言、拨号脚本、退避策略；MPL-2.0
附禁止商用附加条款），`docs/UPSTREAM_AUDIT.md` 新增 §7 上游致谢清单。致谢范围
按仓库计：ouyangzq/modem-pro、ouyangzq/luci-proto-modem-pro、FUjr/QModem、
LianXia233/luci-app-fm350；Rust crate 依赖与厂商拨号器 C 源码均不列入——内置
QMI/MBIM 拨号器是本仓库的独立 Rust 实现，未使用、未拷贝任何厂商 C 源码，
C 实现仅作为协议行为基准逐条对照（见 [1.0.3] 其余条目）。

### 修复：小窗页仍在调用已删除的 `/api/sms/auth`（接口名对照 modem-pro）

把本仓 `ROUTES`（99 个路径）与 modem-pro `module/kernel/api.go` 的
`RegisterRoutes`（92 个路径）逐条对照：**88 个共有接口名完全一致，
没有任何改名**；modem-pro 独有的 4 个（`/api/sms/auth`、`/api/sms/lock`、
`/api/sms/password`、`/api/sms/unlock`）是 2026-10-19 取消短信密码时
按设计移除的（见 `http/handlers/wechat.rs` 顶部说明）；本仓新增 11 个
（`/api/snapshot`、`/api/tasks` 提交/查询/取消、`/api/at/sched`、
`/api/dial/auto`、`/api/ntp` 及 `set`/`sync`、`/api/timezone`、`GET /api/traffic`），
均已在 `docs/API.md` 登记，命名沿用 modem-pro 既有约定。

但前端资产里有一处**残留调用**：`rust/web/mini.html`（5G 悬浮小窗内容页）
开机时仍 `fetch('/api/sms/auth')` 探「短信密码没启用就放行正文推送」——
该接口已随密码一起删除，请求稳定 404。后果：探不到 ⇒ `SMS_CONTENT`
永远停在 false ⇒ `+CMTI` 计数推送比正文推送先到 ⇒ **每条新短信弹两次卡**
（先弹“有 N 条未读、正文受保护”，再弹正文卡），且“受保护”文案在密码
取消后本身就是错的。

**修复**（`rust/web/mini.html`）：删掉该探测——短信密码取消后
`push_sms` 与角标一样走普通广播（见 `http/push.rs`），正文恒可拿。
顺带清掉同源死代码：WS 握手的 `sms_token=` 参数（后端 `ws.rs` 早已
不读它）、`localStorage['modemd.smsToken']` 的读取与 storage 监听，
以及只服务「未读变多」兜底的 `onSmsCount` / `SMS_UNREAD` /
`lastSmsPushAt` / `SMS_CONTENT` / `reconnectWS`。计数推送（`smscount`）
仍用于角标渲染，只是不再充当新短信信号——正文必然到达，两条都当新短信
正是当年要避免的双弹。

### 修复：内置拨号器在支持的模组上不自动启动

**现象**：插上 QMI/MBIM 模组，页面也识别出了类型，但拨号器始终不起，
`wwan0` / `wwan0_1` 的 `link_state` 一直是 0，手动点一次拨号才动。

**根因**：`DialerSupervisor::reconcile` 判断「这个模组要不要交给内置拨号器」
用的是 `Modem::cached_dial_kind()`，而它只返回 `dial_state.kind` —— 这个字段
**只在拨过号、或页面拉过 `/api/dial`（`dial_status` 第 ⑥ 步）之后才有值**。
全新模组插入后用户还没拨号、页面也没开，`dial_state.kind` 是空串，
`native_takes()` 把每个模组都判成「不归内置拨号器管」，拨号器永远不启动。
即：把拨号的**产物**当成了拨号的**前置条件**（鸡生蛋）。

**修复**：

- 新增 `Modem::ensure_dial_kind()`：`dial_state.kind` 没有时走
  `usb_type(false)`（自带 30 秒缓存，组网循环本来就会调它，命中缓存零 AT），
  按 `dial_status` ⑥ 的同一口径回写 `kind` / `type_name`，**不动 `state`**
  （没拨号就是没拨号，不能因为探明了链路类型就假装在拨）；探不到
  （unknown / 空）返回空串，调用方按「不接管」处理。`native_takes()` 与
  `reconcile_native_one()` 改用它。
- 顺带修掉「算不出计划」路径的退避失效：原实现在 `plan_native` 失败时先调
  `stop_native()`，把 `native_retry` / `native_fails` 一并清掉，指数退避
  （5/10/20/40/60s）永远停在第一档，持续故障（缺 APN / 控制口未出现 /
  网卡未被 netifd 拉起）会被 15 秒一轮地硬刷。现在退避状态跨轮保留，
  错误**变化**时才清零（情况有变，值得立刻再试）。
- 组网循环改为「先跑一轮再进 15 秒节拍」（quota 循环同一模式）：启动时
  立即建接口、立即让拨号器托管看到模组，插上模组后不必空等一个节拍。

### 对照厂商 C 拨号器补的两处缺口（modem-pro `files/third_party/`）

与 quectel-CM 1.6.8 / fibocom-dial / meig-cm / simcom-cm 源码逐条对照后：

- **`qmap_mode` 探测补齐两条兜底路径**：C 版 `ql_qmi_qmap_mode_detect()`
  （`qmap_bridge_mode.c:241-259`）读三条路径（网卡属性 → 模块参数 → 按设备
  定位的模块参数）；原实现只读第一条。OpenWrt 的 `qmi_wwan_q` / `qmi_wwan_f`
  只把参数挂在模块上，漏读即漏判 QMAP，进而不下发 `WDA Set Data Format` 与
  `Bind Mux`，驱动不 `add_mux`，`wwan0_1` 永远 NO-CARRIER。
- **控制口反查补齐 `usb` 子目录分支**：C 版 `detect_path_cdc_wdm_or_qcqmi()`
  （`device.c:198-252`，fibocom-dial 同构）在 `<接口>/{GobiQMI,usbmisc,usb}`
  三级里找控制口；原实现只认 `usbmisc`。新版 qmi_wwan 把节点挂到接口的
  `usb` 子目录下，只认 `usbmisc` 会找不到控制口，拨号器以 `NoWdm` 退避，
  现象与「模组不支持」无异。

**对照结论（一致，无需改动）**：QMI 流程（CTL Sync → Get Client ID
WDS / WDS_IPV6 / WDA → WDA Set Data Format + Bind Mux ×2 → Set Autoconnect(0)
→ WDS Start v4/v6 → Get Runtime Settings）、全部 TLV 类型与顺序
（WDA 0x10/0x11/0x12/0x13/0x15/0x16/0x17，Bind Mux 0x10/0x11/0x13，
Start 0x30/0x14/0x17/0x18/0x16/0x19/0x31）、QMAP mux 规则
（`qmap_mode==1` 给 `0x81`，`>1` 给 `PDN+0x80`）、MBIM Basic Connect
（OPEN → SUBSCRIBER_READY → CONNECT → IP_CONFIGURATION → CLOSE）均与
C 实现一致；`QUECTEL_UL_DATA_AGG` 在 modem-pro 的 Makefile 未定义，
聚合门限 TLV（0x19/0x1B/0x1C）同样不发。

**已知差异（有意保留，非缺陷）**：

- WDS Start 的 Profile Index：C 版默认下发 `0x31 = 1`（`CONFIG_DEFAULT_PDP`），
  本项目 `[net] host_dial_pdn` 默认 0 = 不指定，由固件选默认 profile
  （单 profile 模组等价；多 profile 需显式配置 `host_dial_pdn`）。
- MBIM Session ID：C 版默认 `profile->pdp = 1`，本项目厂商策略给 0
  （MBIM 默认会话；C 版实机 log `cdc_mbim.txt` 中 session 0 工作正常）。
- C 版在 `/dev` 下没有控制口节点时会按 uevent 的 MAJOR/MINOR `mknod` 兜底；
  OpenWrt 有 devtmpfs/udev 自动建节点，本项目不复制该行为（节点缺失时
  `open` 失败会进退避重试，日志可见）。

测试：`cargo test --locked` 692 项通过（新增 3 项：`ensure_dial_kind`
缓存优先 / 探不到为空、`qmap_mode` 对不存在网卡返回 0）。小窗页改动另经
`node --check` 校验内联脚本语法，并以脚本复核「前端引用的接口路径
⊆ `ROUTES`」成立（修复前差 1 个：`/api/sms/auth`）。

## [1.0.2] - 2026-10-20

### 上游融合（一）：端口规则进发现流程 + 厂商数据通道指令

**发现流程不再「见 tty 就探」**

- 新增 `discovery/plan.rs`：按 USB 设备分组（`1-1`），按 QModem 端口规则的
  `include` 接口号白名单筛选，剔除语音 PCM 口；同组只选一个命令口，其余留作回退。
  以前 RM520N-GL 这类 4 口模组会被登记成 2~3 台「模组」，抢同一块网卡。
- 新增 `AtPort::probe_identity()`：`AT` 回 `OK` 不再当作命令口，必须
  `AT+CGMM`/`AT+CGMI`/`ATI` 有一条内容行（对齐上游 Go 版
  `module/netdev/ports.go:165-252` 的真机结论）。
- `option_driver=1` 的设备在枚举串口**之前**补绑（`option1/new_id`），
  按 5 分钟冷却避免刷日志；受 `[serial] usb_autobind` 控制。
- PCIe/MHI 的 AT 口（`/dev/mhi_*DUN*`、`/dev/mhi_*AT*`、`/dev/wwan*at*`）并入候选；
  `AtPort` 增加裸字符设备后端（这些节点不是 tty，`serialport` 打不开）。

**ECM/RNDIS/NCM 的拨号改成按厂商发指令**

- 新增 `vendor/datacall.rs`：`AT+QNETDEVCTL`（移远，按平台三套参数）、
  `AT+GTRNDIS`（广和通高通/展锐）、`AT^NDISDUP`（华为/鼎桥）、`AT^SETAUTODIAL`
  （华为展锐自拨）、`AT$QCRMCALL`（美格）、`AT$MYUSBNETACT`（有方）、
  `AT#ICMAUTOCONN`（Telit）、`AT+ZECMCALL`（高新兴）、`AT+RNDISCALL`（合宙）。
  以前一律 `AT+CGACT`，移远/华为/美格这类机型 PDP 显示 active 但网卡没有 carrier。
- 挂断与激活**严格对称**：断开时发同族的 deactivate 指令，不再只停 PDP；
  合宙（模组自持 PDP）按上游口径只停主机接口，并在步骤里注明。
- FM350/RW350（广和通 MTK）与芯讯通保持 `AT+CGACT`，不发 `GTRNDIS`；
  私有指令被拒时回退 CGACT 并记录，不会把失败伪装成成功。
- 平台未识别（型号不在清单里）时只走 3GPP 通用路径，不发任何私有指令。

**文档**

- 按**当前代码**重写 `docs/INTEGRATION.md`（分层/数据流/扩展指南）、`docs/BUILD.md`
  （两条构建路径/CI/验证）、`docs/API.md`（由 `router.rs` 导出的 105 条接口全表）；
  新增 `docs/UPSTREAM_AUDIT.md`、`docs/SUPPORT_MATRIX.md` 与 `docs/README.md` 索引；
  README、RELEASE_NOTES 同步，`PORTING.md` 的两处代码引用改为 `API.md`。

- 非 AT 口否定缓存：15s→30s→60s→120s→240s→300s 退避，端口消失即清空。
  巡检 3 秒一轮，否则每个「不是模组」的 tty 每轮都要白等 4 条 AT 超时。

测试：`cargo test --locked` 722 项通过（新增 19 项，含厂商命令字面量、平台回退、
cid 替换、激活/挂断对称、端口白名单与物理分组、退避曲线）。真机验证待办见审计文档 §5。

### 上游融合（二）：多 tty 拓扑 / PCIe 补绑 / 救援 / 退避

- **多 tty 拓扑对上层可见**：`ModemView.tty_list` 以前恒为「当前口自己」，
  现在按发现分组写入「首选 + 回退」；手动切口后新口排到最前，其余保留。
- **同型号多台设备的命令口绑定**：新增 `vid:pid@1-1` 绑定键（`/api/atport/bind`
  增加可选 `bus_path`），两台 RM520N 不会再共用一条绑定；旧键继续有效。
  WebUI 的设备下拉按物理路径区分，已绑定行改口/解绑都带 `bus_path`。
- **PCIe 模组自动补驱动**：`[serial] pcie_autobind`（默认 true）在发现前对
  未绑定驱动的模组尝试 `mhi_pci_generic` / `mtk_t7xx`，5 分钟冷却。
- **卡死救援**：`[serial] auto_rescue`（默认 **false**）。一台模组都认不出持续
  150s 后，对无 tty 的 USB 设备写 `authorized 0→1`、对未绑定 PCIe 设备做
  remove+rescan；5 分钟冷却、每轮最多 3 次。默认关闭：会短暂断开数据面。
- **拨号失败退避**：原生 QMI/MBIM 建立失败从固定 5s 改为 5/10/20/40/60s
  指数退避，成功即清零（持续故障不再每 5 秒刷一次控制口）。
- **清单形态核对**：支持清单里该型号 USB/PCIe 都不含当前识别出的形态时，
  在拨号步骤里写一条提示（不拦截），避免「清单说 A、实际是 B」时无从排查。
- USB 设备清单（`/api/usb`）纳入端口规则登记过的设备，否则绑定页选不到它。
- 新增探活预热窗口：设备 sysfs mtime < 90s 时固定 15s 重探，不参与指数退避
  （5G 模组开机慢，退避到 5 分钟等于永远认不出）。
- **断开后清旧地址/旧路由**：`POST /api/dial/disconnect` 成功后，逐条删除经该模组
  网卡的默认路由并 `ip -4 addr flush`（只动这台模组的网卡）。以前只改状态，
  内核里的旧 `default` 还在，页面显示「已断开」但流量继续往黑洞网卡发。

测试：`cargo test --locked` 732 项通过（本阶段新增 10 项：救援判定、预热、
形态核对、物理路径绑定键、多 tty 排序、拨号退避曲线、断开路由清理）。

### 强制下发 DNS（`[net] wan_dns`，默认 223.5.5.5 119.29.29.29）

实机「获取 IP 异常 / APN 异常」的真凶是 `resolv.conf.auto` 为空 —— 蜂窝链路
（FM350 系列 RNDIS 尤其如此）拿不到 DNS，dnsmasq 对公网域名一律 REFUSED。
新增 `[net] wan_dns`（默认两个公共 DNS，`off`/`none` 关闭），组网时写进
`network.modem<id>.dns`，netifd 拉起接口即经 `interface_add_dns_server`
下发到 `resolv.conf.auto`。取值可配，空值回落默认。

### 连接时间跟随接口时间

拨号链路「在线时长」早先有两个来源、都与「接口时间」对不上：模组会话
`connected_at`（重拨/接口重建后对不上）与内存观测起点 `link_since`
（modemd 重启即归零）。改为取 `ifstatus modem<id>` 的 `uptime`——
netifd 接口在线时长才是权威值，且跨重启保持正确；前两者降为兜底。
前端三处标签统一为「接口在线时长」。

## [1.0.1] - 2026-10-20

### 1.0.0-14：实机四连修 —— 网卡归属 / 小区数量 / APN 溢出 / 全链路自动恢复

#### 🟢 主机网卡只认模组 USB 设备下的接口

实机翻车：RNDIS 数据口是 eth2（挂在模组 USB 接口上），而
`pick_dial_iface(猜名快照)` 把 **SoC 有线口 eth1**（主路由 LAN，
192.168.1.x）认了进去 —— 页面「主机网卡接口 / 已分配 IPv4」显示的
与模组毫无关系。三处猜名点全部改为 `usb_netifs_for_port()` 按 sysfs
归属枚举（优先 QMAP mux 口）：`dial_inner` ④、`dial_status` 的
dial_update 钩子（改为**归属校验**：记录的网卡不属于这个模组就换掉）。
不再识别成其他接口。

#### 🟢 概览「服务小区数量」永远 0（两层）

1. 前端读 `cells.serving_count`，后端 `/api/cells` 返回的是 **`serving` 数组**
   —— 字段名对不上；
2. 快照（snapshot.blocks）里**根本没有 cells 块**，概览 load 也没拉过 ——
   改为 load 时自拉 `/api/cells`，顺手加一行「当前服务小区」明细
   （B8 · PCI · EARFCN · RSRP）。实机：数量 1、邻区 4。

#### 🟢 APN 表溢出

6 列表格套 `.mp-table-scroll` 滚动容器（与 cells 页同款），列不被压扁、
超出横向滚；表头「默认」改「备注」（内容已改为 IMS 专用徽标）。

#### 🟢 「内置原生拨号后端 未运行」文案误导

RNDIS/ECM/NCM 的数据面是模组自带的 USB 网卡，主机侧**本来就不需要**
拨号器 —— 徽标改「无需主机侧拨号器」，说明按数据面分叉（AT 路径 vs
QMI/MBIM 托管）。原生后端只在 QMI/MBIM 模式下接管，无需用户选择。

#### 🟢 实机排查带出的三个真缺陷（全链路自动恢复的关键）

1. **后端缺省 modem_id 硬编码 0**：USB 重新枚举/模组重启后管理器重新编号
   （实测 0→1→0 漂移），硬编码让所有缺省请求 404、**前端整页崩**
   （`.format` 报错的真相）。改为回落到第一台实际存在的模组；
   前端 store.js 同步：保存的 id 失效就跟随快照/第一台，带 id 请求 404
   时清掉保存值立即重试。
2. **地址缓存跨会话污染**：`own_ipv4_cache` 在会话重建后仍持旧值，
   自动组网把**上一条会话的地址**写进 static（10.24.46.185 旧 →
   10.12.170.224 新）。拨号开始即作废缓存；`query_own_ipv4` 的 cid
   解析走优选上下文（模组重启后固件把 cid 1 分给了 SOS，数据会话在
   cid 3 —— 照默认 cid 问永远问不到 v4）；组网改为实时查询优先。
3. **350 系列怪癖转换漏了 modem_pro 模式**：modem_pro 的 v4 也是
   udhcpc（mp-client wrapper），而 FM350 模组内 DHCP 服务器实测长期
   无响应 —— modem_pro 模式在 350 系列上永远拿不到 v4。转换闸门扩展
   到 dhcp + modem_pro 两处（provisioner + plan_wan）。

#### 验证（全部实机）

- 模组整机重启后**全自动恢复**：自动拨号（pick cid 3/ctnet）→ 组网按
  新地址写 static（10.12.170.224）→ `phase=online` → ping 0% 丢包，
  全程无人工干预；
- Playwright：概览服务小区 1/邻区 4 + 当前服务小区明细、拨号摘要
  eth2/10.12.170.224、APN 表滚动容器 3 行、原生拨号卡新文案；
- `cargo test` 702 passed / 0 failed；门禁 283 文件 0 命中。

---

### 1.0.0-13：全模块拨号逻辑审查 —— APN 优选三方统一 + 幂等闸门

对三条拨号触发路径（手动 / 自动拨号守护 / 拨号器托管 reconcile）做交叉审查，
发现的核心问题：**APN 优选逻辑只活在一键拨号前端里**，另外两条路径拿不到
APN 时只会空手退避。

| 发现 | 影响 | 修复 |
| --- | --- | --- |
| 自动拨号对 QMI/MBIM 模组也发 `dial()` | 与托管 reconcile 抢 `dial_busy`、往 steps 塞无意义的「已交办」 | 自动拨号只驱动 AT 路径（ECM/NCM/RNDIS）；QMI/MBIM 归托管（15s 一轮，含掉线重拨） |
| 自动拨号/托管拿不到 APN | MBIM 模组（拨号器无 -s 直接退出）陷入「算不出计划 → 5s 退避 → 再来」死循环——明明模组里存着可用上下文 | 新增 `pick_dial_context()` 三方共用：已激活非 IMS 优先 → 任意非 IMS → 回落默认 |
| `set_apn` 同值重写 + 先断后连 | 「PDP 已激活、主机链路未就绪」窗口里每 30s 拆一次正常会话（自动拨号带 APN 走到这） | `set_apn` 加幂等闸门：该 cid 同 APN 同类型时跳过重写与重激活 |
| 托管 reconcile APN 只有两级回退 | 同上（MBIM 计划拒绝循环） | 加第三级：模组已配置上下文（走 15s 缓存） |

#### 验证（全部实机，1.0.1-4）

- 自动拨号接管：重启后 disconnected → `[auto-dial] modem #0 自动拨号成功` →
  `phase=online`；之后 90 秒观察零重复拨号；
- 幂等闸门：同值写两次 cid 3，第二次返回「配置未变化，跳过重写（幂等）」；
- `cargo test` 702 passed / 0 failed（新增 pick_dial_context 4 组用例）。

---

### 1.0.0-12：跳转按钮修复 + APN/PDP 管理放开 + 自动拨号（实机反馈）

#### 🟢 overview 三处跳转失效

「基站详情 →」「拨号设置 →」「查看全部记录」用的是 `#tab=xxx` 锚点 ——
那是 `main.js` SPA 的路由格式，而本应用每个页面是**独立 LuCI view**
（menu.d 注册 `admin/modemd/<id>`），SPA 路由没人消费，点击毫无反应。
改为与顶部 tab 栏同口径的 `L.url('admin/modemd/<id>')` 真实地址。

#### 🟢 APN 上下文与 PDP 管理

1. **配置了 APN 不显示（根因：字段名对不上）**：前端读 `params.apns`，
   后端返回的一直是 `params.contexts` —— 列表永远是空的
   「模组内没有已保存的 APN 上下文」。修正字段名。
2. **CID 不再限制 1~16、不再固定 1**：默认取「现有最大 CID + 1」
   （350 系列 IMS/sos 常驻 cid 1~2，默认塞 1 会撞）；上限因模组而异，
   前端不写死，输错由模组回 ERROR 如实提示。
3. **「默认」列改「IMS 专用」徽标**：后端没有 default_cid 概念，
   `internal`（IMS/sos）才需要标注，防止误删上网那条。
4. **一键拨号默认使用已配置的 APN**（fm350 同款体验）：优选「已激活且非
   IMS」的上下文，其次任意已定义的非 IMS 上下文，带上 cid/apn/pdp_type；
   都没有才回落「沿用模组配置 + 默认 cid」。早先什么也不传，用户配好的
   APN 永远用不上。

#### 🟢 自动拨号（`[net] auto_dial`，fm350 同款）

配置字段早已存在（从 fm350 迁移、默认 true）却**从未被消费** —— 开机后
PDP 掉了只能手动点拨号。本轮接上：

- 新增 `spawn_auto_dial` 守护循环（30 秒）：总开关/自动拨号关着、已连接、
  正在拨、用户手动断开过（manual_stop）都跳过；否则走与手动完全相同的
  `Modem::dial`（apn 留空 = 沿用模组已配置的上下文）；
- 失败退避 5 分钟（无卡/无服务的机器不该被 30 秒一轮硬砸）；
- `POST /api/dial/auto {on}` 开关 + `GET /api/dial` 返回 `auto_dial`，
  拨号页策略区加第四格开关卡。

#### 验证（全部实机）

- 重启后 disconnected → 45 秒内日志 `[auto-dial] modem #0 自动拨号成功`、
  `state=connected phase=online`；
- 写 APN（cid 3）→ `contexts` 列表显示 → 删除回读确认；
- Playwright 实测：三按钮真实跳转（tools 页标题「调试工具」）、
  APN 表渲染 contexts、自动拨号卡四格齐。

---

### 1.0.0-11：MT5700 系列专项（鼎桥方言全量接入）

以官方手册《MT5700M-CN 5G 系列模组 AT 命令手册》V200R001C20 + 同作者
[luci-app-mt5700](https://github.com/LianXia233/luci-app-mt5700) 的**实机验证解析**
为依据，把 MT5700/MT5710 的支持从「只有识别」补到全功能：

- **小区/信号解析（此前完全缺失）**：`^MONSC`（服务小区，NR/LTE 两种布局 +
  旧编码格式 B + PCI 十六进制带越界回退）、`^MONNC`（邻区，多行）——
  此前 MT5700 的小区页/信号数据完全解析不出来；
- **主动扫频**：`AT^CELLSCAN`（手册 §5.35，支持无卡扫描）接入 `/api/netscan`，
  15 字段行解析（band 十六进制、99=无效、LTE/NR 的 SINR 列不同）；
- **锁网方言**：`Dialect::Tdtech`（`AT^SYSCFGEX?` 探测）——锁频段
  （`^LTEFREQLOCK=3,0,<num>,<band>…` / `AT^NRFREQLOCK=…`，写后自动切飞行模式，
  手册明文语义）、锁状态读、解锁；锁小区因需 `<band>` 参数（UI 只有 earfcn/pci）
  按「未验证拒绝盲写」口径明确拒绝；
- **温度**：`^CHIPTEMP?` 12 路传感器（0.1 ℃，未上报的 0 跳过）接入 `/api/modem/status`；
- **AT 指令面**：`^VERSION` / `^MONSSC` / `^C5GOPTION?` / `^LENDC` / `^CASCELLINFO?` /
  `^TXPOWER?` / `^NTXPOWER?` / `^MCS?` / `^RRCSTAT` / `^DHCP` / `^DCONNSTAT?` 等 11 条
  手册指令进预设（全部标注手册章节号）；
- 识别关键字补 `MT5710`（同族）；
- 新增 11 个单测（MONSC 双布局 + 格式 B + PCI 回退、CELLSCAN 15 字段、
  FREQLOCK 读解析、CHIPTEMP）。

### 1.0.0-10：待办清零 —— CCLK 时区按 3GPP 修正 + 路由判定按地址族

上一轮（1.0.0-9）留下的三件待办，本轮全部用实机做完。

#### 🟢 ① `ignore_cclk_tz` 实机核对完成并接入

FM350 实测：`AT+CCLK?` 回 `26/10/19,23:02:41+00`，与主机 UTC 同刻 ——
值是 UTC、**后缀本身正确**。据此确定接入方式：新增 `parse_cclk_tz()`
**默认按 3GPP TS 27.007 减 `±zz`**（zz 是 15 分钟单位），怪癖表标记的
违规固件（值是 UTC、后缀却报 +32 的 RM520N/EM05/MT5700/FM650 那批）
才跳过。本机 FM350 两种路径结果一致（+00 减 0），**行为零变化**；
变化只在 FM160 这类「报本地时间且后缀正确」的型号上——修正后 clock_skew
不再显示 +8h。7 个单测覆盖 +00/±32/负偏移/无后缀/垃圾后缀。

#### 🟢 ② `route_pending` 实机复现 + 顺带修掉 v4/v6 混判

删掉 eth2 的 v4 默认路由（frp 走 eth1，不断连）：

```
T+0 路由数 0 → phase=route_pending（网卡有地址，但默认路由不经它）
恢复后 phase=online，ping 丢包 0%
```

第一次复现时 phase 却仍是 `online` —— 因为 eth2 的 **IPv6 默认路由
不受 v4 删除影响**，而 `has_default_via` 把两张路由表混着判。已改为
**按地址族分别判**（`has_default_via_family`）：有 v4 地址就要求 v4 默认
路由、有 v6 地址就要求 v6 默认路由。这正是「双栈配置不完整」的分层诊断。

另实测：netifd 对被外部 `ip route del` 的 static 路由会在 **~3 秒内重装**。

#### 🟢 ③ DNS 盘点（结论：无缺陷）

双 WAN 实机：`modem0`（static）未配 `dns`/`peerdns`，DNS 全走 eth1 上游；
modem 口不提供 DNS 是 netifd 对 static 协议的正常行为，纯模组上网机器由
peerdns 或 static `dns` 提供。

#### 验证

- `cargo test`：**689 passed / 0 failed**（新增 9）；
- 交叉编译 aarch64-musl 通过并已部署；实机恢复原状（多余的路由也清了）；
- 测试样例与文档中的实机地址一律换文档段（2001:db8 / 192.0.2.0/24 / 10.42.0.x）。

---

### 1.0.0-9：拨号链路阶段（phase）——「已连接」不再是一句空话

对齐上游拨号实现后确认：方式判定、按类型分派、网口关联、幂等组网都已与
Go 上游一致；真正缺的是**连接状态分层**。审计明细见
`docs/DIAL_AUDIT.md`（已删除，内容并入 `docs/UPSTREAM_AUDIT.md`）。

#### 🟢 问题

AT 拨号（ECM/NCM/RNDIS）收尾只凭 `AT+CGACT=1` 回 OK + `AT+CGPADDR` 有地址
就置 `state=connected`。但「主机真的能出去」还差两层：

3. 主机网卡存在且拿到地址（FM350 实测：模组侧全好，eth2 挂 169.254）；
4. **默认路由经这块网卡**（有地址但路由被别的口抢走，照样打不开网页）。

页面显示「已连接」，用户以为网络坏了 —— 排障方向完全错误。

#### 🟢 修复：`phase` 阶段层（`state` 契约不变）

| phase | 含义 | 判据 |
| --- | --- | --- |
| `iface_pending` | 会话已建立，主机网卡未就绪 | 网卡缺失或无地址 |
| `route_pending` | 网卡有地址但默认路由不经它 | `has_default_via()==false` |
| `online` | 地址 + 默认路由都有 | 三层全过 |

- 路由事实读 `/proc/net/route` + `/proc/net/ipv6_route`（**不 fork**，
  `netprov/route.rs`，小端网关还原 / 默认路由识别 / 子网排除 6 个单测）；
- `dial_status()` 是 HTTP/WS/snapshot 唯一出口，phase 统一在此计算；
- 拨号收尾把「主机网卡就绪」「默认路由经该网口」写进 `steps`，
  失败那条直接给排障方向；
- LuCI 拨号页加「链路阶段」行、WebUI 拨号卡加阶段徽标（同一份 JSON）。

#### 验证

- `cargo test`：**687 passed / 0 failed**（新增 8 个）；
- 实机（netifd 拉起 eth2）：`phase=online`，LuCI 显示
  「链路阶段 网络就绪（地址 + 默认路由）」；
- `route_pending` / `iface_pending` 两条路径由单测覆盖（构造三布尔全组合）。

---

### 1.0.0-8：多维度模组识别 + USB 端口规则 + 逐型号怪癖

对齐两个上游（QModem `2ef9227`、Go 版 `397e014`）的模组知识，并把「认模组」
从单线索改成多维度取证。审计明细见 [`docs/UPSTREAM_AUDIT.md`](docs/UPSTREAM_AUDIT.md)。

#### 🟢 识别体系（`modem/detect.rs`，新增 481 行）

原来只有一条线索：`ATI` → 厂商关键字 → 短名。三个后果：ATI 回 ERROR 的固件
（部分展锐）识别落空；型号撞车无法校验；认错了页面上看不出来。

现在按**证据可信度**取结论，每条证据都可追溯：

| 依据 | 置信度 | 条件 |
| --- | --- | --- |
| `port_rule`（`VID:PID` 精确登记 + AT 型号一致） | 90 | 最强 |
| `catalog_model`（AT 型号命中清单） | 85 | 设备自报 |
| `port_rule`（AT 回 ERROR 时） | 80 | 仍能定型号 |
| `vendor_keyword`（只认到厂商） | 55 | — |
| `usb_vid`（清单里该 VID 唯一厂商） | 45 | 仅独占 VID |
| `no_match` | 0 | 未知设备不猜 |

- USB 身份从 sysfs 取（`usbdev::identity_for_tty`：VID/PID + 接口号 + 拓扑）；
- AT 只发**只读**指令：`ATI`，失败退回 `AT+CGMM` / `AT+CGMR`；
- 冲突不猜：设备自报优先，厂商按清单归属纠正，置信度 −15，冲突原文进 `conflicts[]`；
  `huawei` / `tdtech` 视为同族（`MT5700M` 共线）不报冲突；
- 型号匹配补「主干」退路：`ATI` 只报 `MT5700M` 时也能命中清单里的 `mt5700m-cn`
  （原来整名包含失败，显示未识别）。

#### 🟢 USB 端口规则（`modem/ports.rs` + `modem_ports.json`）

迁入 QModem `modem_port_rule.json` 的 18 条 `VID:PID` 规则：AT 接口白名单
（`1.2`/`1.3`/`1.4`…）、语音 PCM 口、是否需要绑 option 驱动。
作用是「别把 diag 口当 AT 口」——选错口的典型症状是能打开、发 AT 全超时。

#### 🟢 逐型号怪癖（`modem/quirk.rs` + `modem_quirks.json`）

迁入 Go 上游 `module/modelquirk/table.go` 的 19 条实测特例，每条保留原有
证据（固件版本 + 原始应答）。**只接入能验证的三个字段**：

| 字段 | 效果 |
| --- | --- |
| `voltage_cmd` | Sierra/富士康走 `AT!PCVOLT?`，`AT+CBC` 兜底 |
| `iccid_cmd` | 合宙/Telit 走 `AT+ICCID`，有方 N510M 走 `AT$MYCCID` |
| `temp_cmd` | FM160 的 `AT+GTLADC` 插到厂商候选最前 |

`ignore_cclk_tz` **未接入**：上游语义是「后缀不可信、不要换算」，而本仓库的
`parse_cclk` 从一开始就忽略后缀（等于对所有型号都 ignore）。直接接会让未登记
型号的时钟偏差从 0 变成 −8h，需实机核对后再动。

#### 🟢 前端（LuCI + WebUI 同源）

`ModemView` 新增 `identify` 字段（证据 / 候选 / 冲突 / USB 身份 / 置信度）。
LuCI 概览页「模组硬件」与 WebUI「身份」卡片各加一行**识别依据**，
有冲突时再多一行**识别冲突** —— 两端读同一份 JSON 的同名字段。

#### 验证

- `cargo test`：**678 passed / 0 failed**（新增 31 个）；
- 交叉编译 aarch64-musl 通过；
- 实机 FM350-GL：`matched_by=catalog_model`、`confidence=85`、
  `usb=0e8d:7127`、`conflicts=[]`，与 `ATI` 原文一致。

#### 仍未验证

端口规则命中、冲突降级两条路径只有单测（本机 PID 不在 18 条规则里）；
华为/SIMCom/高新兴 的温度电压指令无设备可验。

---

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

#### 🔴 实机暴露：下拉框永远选中「最后一项」（全局缺陷）

用户报「时区选择功能不可用」。真机排查发现两层问题：

1. **下拉根本没进 DOM**。`service.js` 里创建了 `tzSel` 和「应用时区」按钮，
   但只把按钮 append 进卡片头，**下拉忘了插**。页面上只剩一个按钮，
   点下去 `tzSel.value` 恒为空、函数第一行就 `return` —— 表现为「点了没反应」。
2. **更普遍的一层**：LuCI 的 `E()` 会把 `selected: false` 照样序列化成
   `selected="false"`，而 HTML 布尔属性**只看存在性**。于是同一组里所有选项
   都被判为「选中」，最后一个赢。实测这台机器的时区下拉永远停在预置列表
   最后一项，而系统时区其实是另一个值。

第 2 条影响的不止时区，全项目 **16 处**下拉都有：

| 文件 | 下拉 |
| --- | --- |
| `service.js` | 系统时区（12 项） |
| `dial.js` | PDP 类型（3 项）、WAN 模式 |
| `lock.js` | 锁网模式（6 项） |
| `sms.js` | 短信存储区 |
| `tools.js` | 配额动作等 5 处 |

统一改成 `'selected': (cond) ? 'selected' : null`（`null` 会被 `E()` 跳过，
实测渲染后该 option 上没有任何 `selected` 属性），时区下拉再补一道
`tzSel.value = tzCur.zonename` 兜底，不依赖属性是否正确。

顺带给时区下拉加了「未读到」的兜底选项，并把卡片底部说明改清楚：
**右上角的「系统时区」**改的是路由器本身（`/etc/config/system` + `/etc/TZ`），
**下方「当前时区」**是 modemd 自己的 `[ntp] timezone`（决定日志时间戳），
两个不是一回事。

#### 🟢 页面布局与渲染修复（实机三处）

1. **实时速率卡片：合并成一张，且上下行各自一条曲线**
   ```
   [上行速率] [   上行曲线   ]
   [下行速率] [   下行曲线   ]
   [本次累计上传] [ 本次累计下载 ]
   ```
   早先是「四格指标卡 + 独立曲线卡」两张卡，读的是同一个 `/api/traffic`，
   拆开摆着反而像两份数据，还白占一屏。合并后按行对齐：每行是
   「此刻多快 + 这一分钟怎么走」，眼睛不用上下扫。

   曲线**按行拆开、各用各的纵轴**（`spark.render(..., { only: 'up' | 'down' })`）。
   叠在一张图共用一根轴时，上行 20 kbps、下行 100 Mbps 会把上行压成
   贴底的一条直线，等于没信息。

   尺寸也修了：`viewBox` 直接取容器实测像素（`box.clientWidth/clientHeight`），
   与显示尺寸 1:1。早先固定 `viewBox 0 0 1000 h` 再配 `preserveAspectRatio="none"`，
   卡片里实际只有 190~890px —— 手机上横向缩放 0.34，刻度数字被压成一条竖线。

   压缩与适配（**布局在任何宽度下都不变，不塌成单列**）：

   | 视口 | 卡片高 | 行高 | 曲线 | 数值/字号 |
   | --- | --- | --- | --- | --- |
   | 1440 | 362px | 76px | 890×76 | 18px |
   | 390 | 339px | 58px | 190×58 | 15px |
   | 320 | 349px | 58px | 148×58 | 13px |

   窄屏只缩字号、间距、曲线高度（720 / 360 两档断点）。为吞下这些高度，
   卡片内隐藏了副标题（「实时发送速率」「已发数据量」——标题已说明），
   每格从 85px 降到 67px。

   顺带修掉两处整页横向溢出（只在 ≤360px 出现）：
   - `.mp-tab-bar-wrap` 是 `width:100%` 加左右 6px 内边距，默认 `content-box`，
     整页比视口宽 12px → 补 `box-sizing: border-box`；
   - 卡片网格写死 `minmax(320px, 1fr)`，容器只有 286px 时撑破容器
     → 全项目 10 处统一改成 `minmax(min(320px, 100%), 1fr)`。
     实测 320px：`scrollWidth 310 = clientWidth 310`，不再能横向拖动。

2. **锁频段子页一片空白（`lock.js`）** —— 两层：
   - `drawBands` 在 `render` 内用 `document.getElementById` 找容器，
     而 render 返回的节点**此刻还没挂到 document 上**，全局查找必然拿到 null，
     函数静默 return —— 连「未获取到频段分组信息」的提示都不出现
     （那条提示也在同一个函数里）。改成传 `root` 用 `root.querySelector`。
   - 子页签写成 `E('button', { 'click': function … })`，而 LuCI 的 `E()`
     不认裸函数（要 `ui.createHandlerFn`），四个标签点了全无反应。
     改成 render 末尾显式 `addEventListener`；顺带修掉 `e.target` 老问题
     （点到的是内部 `<span>` 图标，`classList.add('active')` 加在 span 上无效）。
   - 实机结果：LTE 31 个、NR 19 个 chip 全部渲染，支持全选/清空/下发。

3. **cells 页 8 列表格溢出屏幕** —— 套一层滚动容器：
   - 横向：表格 `min-width:760px`，列不被压扁，超出在容器内横向滚；
   - 纵向：`max-height:560px` + 表头 `position:sticky`，几十个邻区也不撑长页面。
   - 实测（注入 40 行）：1440px 文档宽 1430 无溢出；390px 文档宽 380 无溢出，
     表格在 322px 容器内横向滚动、纵向 560px 限高生效。

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
