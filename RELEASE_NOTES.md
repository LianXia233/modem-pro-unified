# modem-pro v1.1.0 发版说明

发布日期：2026-11-08 ｜ 标签 `v1.1.0` ｜ 版本号 `1.1.0-1`（`VERSION`）

本版三条主线：**netifd 接口拉起链路迁入 Rust/Tokio**（`mp-client.sh` 从 182 行缩到 25 行，
新增 `modemd wan-client` 子命令）；**上游对照的九处健壮性修复**（模组识别 / 拨号 / 接口拉起，
逐文件比对四条上游后按真实实现补齐）；**三项原「未采纳」落地**（拔卡后清理残留接口、
`[net] dial_gate` 拨号门禁、识别不到型号时清空型号）。逐项细节见
[CHANGELOG.md](CHANGELOG.md) 的 [1.1.0] 条目。

## 这版做了什么

### 1. 接口拉起迁入 Rust/Tokio（`modemd wan-client`）

接口拉起一直是纯 shell：`mp-client.sh` 用 sed 生成上报脚本、拉起 udhcpc/odhcp6c、
每 3 秒 `kill -0` 巡检、连续失败 5 次才整体退出。状态机写在 shell 里，边界条件只能靠真机踩
（2026-10-13 的孤儿雪崩就是），也无法单测。现在业务逻辑全部落进 `rust/src/wanclient/`
（Tokio `current_thread` 运行时），shell 只保留 netifd 契约必需的两层：

- `files/lib/netifd/proto/modem_pro.sh`：保留 `proto_config_add_*`、`MP_UOPTS`/`MP_OOPTS` 拼装、
  `proto_export`、`proto_run_command`、`teardown`、MTU；`accept_ra` 的写 sysctl 移到 Rust，
  shim 只把「是否要设」的判定结果经 `MP_FIX_ACCEPT_RA` 传给子进程。
- `files/usr/lib/modem-pro/mp-client.sh`：缩到 25 行，只做「校验 `/usr/bin/modemd` →
  `exec /usr/bin/modemd wan-client "$@"`」。

**为什么用 CLI 子命令而不是 unix socket**：`proto_run_command` 跟踪的是它 exec 出来的 PID，
协议 down 时 netifd 直接对该 PID 发 TERM、超时 SIGKILL。若让常驻 daemon 托管真正的客户端，
shim 被杀后 daemon 仍持有 udhcpc/odhcp6c，netifd 既看不到也收不掉 —— 正是要避免的孤儿场景。
子命令模式下进程树与旧 shell 完全一致，信号直达、退出码直传，且无新增 socket 与权限面。

**兼容性**：脚本路径、`add_protocol modem_pro`、`proto_export` 变量名与语义、`mp-client.sh`
路径与启动方式、uci 选项、接口命名全部未变，老配置无需迁移。唯一新增的是内部变量
`MP_FIX_ACCEPT_RA`（用户不可见）；日志 tag 与文案仍为 `modem-pro: mp-client: …`。

### 2. 上游对照：九处健壮性修复

对照基线：`ouyangzq/modem-pro`（Go）、`ouyangzq/luci-proto-modem-pro`、
`FUjr/QModem`、`6106757-lab/openvohive`，逐文件逐逻辑比对。

| # | 落点 | 修了什么 |
| --- | --- | --- |
| ① | 拨号 | MBIM 激活态常量取错（`2` 是 activating、`activated` 是 `1`），导致探活恒为 false、会话反复重拨；现 `CONNECT` 接受两者、探活把两者都算活着 |
| ② | 拨号 | 同参数重复点「拨号」后状态永久停在「拨号中」；协调器「计划未变且会话仍活」分支现按当前代次补写 `connected` |
| ③ | 拨号 | 断开命令失败时不再清理残留地址/路由（半拆后流量打到没有承载的网卡）；清理提到 `match` 之前，成功失败都清 |
| ④ | 接口拉起 | 宿主拨号接口未下发 `force_link=1`，netifd 见「link is down」会连网卡一起拆接口，之后永远等不到载波；现在 QMI/MBIM 且 `[net] host_dial` 开时对主接口与 v6 兄弟接口都写 |
| ⑤ | 接口拉起 | 非 350 的 `wan_mode=both` 会建 v6 兄弟接口却仍让主接口跑 `odhcp6c`（同链路两份 DHCPv6 抢同一份 IA_NA）；现在「是否让出 odhcp6c」与「是否建兄弟接口」共用同一判据 |
| ⑥ | 接口拉起 | 接口在 netifd 里存在但 DOWN 时不再被拉起（开机模组未就绪 / 首次 up 失败 / 半途 reload / 手动 ifdown）；每轮对 `ifstatus` 里 `"up": false` 的接口补一次幂等 `ifup`（20 秒超时、同接口 45 秒限速） |
| ⑦ | 模组识别 | 换厂商后仍残留按旧方言解析的 `params`/`usbnet`/`mode` 缓存；换方言即作废 |
| ⑧ | 模组识别 | `AT+QCFG="usbnet"` 的写与回读分属两张调度票，回读可能读到被切断的应答或撞上第三个写者；现票内连发，回读走同一套解析并同步缓存 |
| ⑨ | 模组识别 | 回读不符时缓存未更新，`GET /api/usbnet` 会报出模组已不再使用的档位；缓存紧跟回读解析、早于校验 |

第 ⑨ 条来自 PR #1 的 Codex 行内评审意见（`discussion_r4504716083`），核实成立后修复。

### 3. 三项原「未采纳」落地

1. **拔卡后清理残留接口**（`modem<id>` / `modem<id>_6`，`rust/src/netprov/cleanup.rs`）。
   保留上游「模组不在 + 设备不在 ⇒ 删接口」的两道保险，再加三道闸：
   **生成记录比对**（`netprov_ifaces.json` 只记本程序写过的 `MANAGED_KEYS`，UCI 现状多键 /
   改值 / 删键即判「用户动过」并保留接口；没有生成记录的老接口一律保留）；
   **删除前留档**（完整选项追加 `netprov_cleanup.json`（保留最近 20 条），同时把可直接粘贴
   执行的 `uci set` 片段打到 warn 日志）；
   **抖动稳定判据**（连续 ≥4 轮且距第一次确认 ≥90 秒才删；USB 复位 / 驱动重载 / 拨号器自愈
   都会让网卡短暂消失）。只对 `auto_wan` 打开且 `uci` 可用的环境生效。
2. **`[net] dial_gate` 拨号门禁（默认 `off`）**（`rust/src/runtime/dialer/gate.rs`）。
   有序判据对齐上游 `hostDialStopReason`，结果写成稳定代码进 `DialState.stop_reason`
   （`host_dial_off` / `dial_disabled` / `roaming` / `airplane` / `manual_stop` / `quota`），
   前端与 ubus 按代码判断、不再匹配中文短句。三档 `off` / `roam` / `all`，
   **默认 `off` 与旧版行为一字不差**；配置写错或空值一律归 `off`（写错不能变成更严的门禁）；
   事实查询带 60 秒缓存，用户动开关显式清缓存；查询失败按「不拦」处理。
   AT 路径已连接的会话不在门禁里拆（拆会置 `manual_stop`，等于替用户做断开决定）。
3. **识别不到型号时清空型号**。旧实现让型号只升不降：换了另一台不在清单里的模组、或命令口
   被 `repoint` 指到别的设备后，页面一直显示上一台的名字、方言也按旧型号解释。现在识别不出
   即写 `None` 并记一条 warn，同时作废 `params`/`usbnet`/`mode` 缓存；界面按
   `m.model || '-'` / `|| 'MODEM'` 显示「未识别」占位（已核对为空值安全）。

### 4. 其余随版变化

- **generic MHI（主线 PCIe 驱动）数据面**：此前控制口发现只认 `cdc-wdm` / `GobiQMI` /
  `mhi_QMI`；现在支持 `mhi_pci_generic` / `mhi_net` / `mhi_wwan_*`（控制口在 `/dev/wwan*`），
  并新增 QMI 裸 IP（无聚合）数据格式设置与校验、`BindMhi`（省略 mux id TLV）。
  候选控制口数 ≠ 1 直接放弃（对齐上游 fail-closed 口径）。
- **`[net] reset_before_dial` 自愈复位（默认关）**：原生路径同一模组连续失败 ≥5 次
  且超过冷却期（`reset_before_dial_cooldown`，默认 300s、钳 60~3600）时发 `AT+CFUN=1,1`
  并等 AT 口恢复（≤150s）后立即重拨；只作用于 QMI/MBIM 原生路径。
- **内置 WebUI 退役**：`modemd` 只提供 `/api/*` 与 WebSocket，不再托管静态页面；
  非 `/api/` 路径返回 404。`/api/*` 契约、`/ws`、`/display/ws`、企业微信转发、鉴权、
  定时任务全部保留；历史配置里的 `[ui]`、`[server]` 键完全兼容。
- **CI 每次编译都发 Release**：手动触发也会发布 `build-<run_number>`，产物不必等 14 天内的
  artifact；自动清理旧 artifact 与旧 `build-*` Release（保留最近 3 个），`v*` 版本不动。
- **修复 FM350 NR 驻网时 RSRP/RSRQ 系统性偏低**：`parse_cesq` 改为 NR SSB 扩展格优先，
  制式判定从「猜」改为「看扩展格在不在」（实机抓包交叉验证）。
- **`docs/` 文档集移除**：仓库内不再维护文档，事实来源为源码、`config.toml` 逐项注释与
  `CHANGELOG.md`。历史条目里出现的 `docs/*.md` 路径只是历史记录。

## 产物清单

| 文件 | 适用 |
| --- | --- |
| `modem-pro-1.1.0-r1_apk-mainline.apk` | OpenWrt 主线 / snapshots，包架构 **aarch64_generic** |
| `modem-pro-1.1.0-r1_apk-filogic.apk` | OpenWrt 主线 / snapshots，包架构 **aarch64_cortex-a53**（MT7981/7986 等） |
| `modem-pro_1.1.0-r1_aarch64_generic.ipk` | OpenWrt 24.10.x（opkg） |
| `SHA256SUMS.txt` | 上述产物的校验和（按合并后的实际文件重算） |

两个 APK 内容相同（同一份静态 musl 二进制、无内核模块依赖），只是包架构标签不同；
按设备 `apk --print-arch` 的输出选对应文件。包内含 `modemd`、netifd 协议脚本与 LuCI 前端。

## 安装

```sh
# OpenWrt 25.x（apk）
apk add --allow-untrusted /tmp/modem-pro-1.1.0-r1_apk-mainline.apk

# OpenWrt 24.10（opkg）
opkg install /tmp/modem-pro_1.1.0-r1_aarch64_generic.ipk
```

## 升级注意

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.1.0-1`）：
  `modemd` 的版本、包版本与页面显示的版本同源。
- **默认行为不变**：`dial_gate` 默认 `off`、`reset_before_dial` 默认关、拔卡清理只在
  `auto_wan` 打开时生效。要用门禁请在 `/etc/modemd/config.toml` 的 `[net]` 下显式设
  `dial_gate = "roam"` 或 `"all"`。
- **接口拉起链路已换实现**：排障仍看 `logread -e modem-pro`，`mp-client:` 前缀照旧。
  连续失败 5 次后 modemd 退出、由 netifd 按协议策略重启，与旧行为一致。
- **内置 WebUI 不再提供**：原先收藏的 `http://<设备>:<port>/` 静态页现在 404，
  请直接用 LuCI（`admin/modem/modemd`）。API 端口本身保留，LuCI 仍按它代理。
- **未识别型号**现在会显示「未识别」占位而不是沿用上一台的型号；插上受支持的型号后
  ≤15 秒内自动回填。
- 无 API 路径与字段变更，无新增必填配置；`/etc/modemd/config.toml` 是 conffiles，
  升级不覆盖你的取值。

## 已知边界（如实说明）

- **接口拉起迁移未经真机验证**：Rust 侧有单测覆盖参数切分、sed 等价的行插入、失败计数
  1→5 与清零、udhcpc/odhcp6c 参数顺序；但 `.apk` 交叉编译与实拨（udhcpc/odhcp6c 实际起停、
  连续失败 5 次后 netifd 是否按预期重启协议）、`/dev/log` 在目标设备上的 syslog 落盘行为、
  tokio 引入后的二进制体积增量均未实测。
- **上游对照的九处修复未经真机回归**：只做了 `cargo test --lib` 与编译检查，未做镜像构建、
  未上真机跑 MBIM / QMI / 350 系列实拨；`force_link=1` 与补 `ifup` 在 netifd 上的实际效果、
  MBIM 激活态常量改动后的真实会话行为都未回归。
- **拔卡清理只在单测里验证**了判定 / 留档 / 抖动窗口，**没有在真机上跑过
  `uci delete` + `reload_network` 全流程**（本项误删代价高，建议先观察 warn 日志与
  `netprov_cleanup.json` 再放宽）。
- **`dial_gate`** 的 `roam` / `all` 两档未在真机验证（国际漫游、飞行模式、配额三条判据
  的数据来源依赖固件应答）；默认 `off` 不触发这些查询。
- 质量：`cargo test --lib` **808 项通过**（0 失败）。
- `cargo fmt --check` 与 `cargo clippy` 仍有**存量漂移/告警**（`sms.rs` 等历史文件，
  非本版改动，未越界重排）。
- 发布说明基于本地 x86_64 验证；musl 交叉编译与产物由 CI（OpenWrt SDK）完成，
  以 Actions 运行结果为准。

## 文档

`CHANGELOG.md` 的 [1.1.0] 条目（本版全部细节，含各条的未验证说明）。
