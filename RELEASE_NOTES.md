# modem-pro v1.2.0 发版说明

发布日期：2026-11-17 ｜ 标签 `v1.2.0` ｜ 版本号 `1.2.0-1`（`VERSION`）

本版是 **v1.1.0 之后的全量审查修复版**：按「模块识别 → 模块拨号 → 接口拉起 → IPv4/IPv6
获取与分配 → APN 自动拨号」逐链路审查，对照 QModem、ouyangzq/modem-pro、
luci-proto-modem-pro、luci-app-fm350、luci-app-mt5700、openvohive 六个上游，修复 91 条审计
问题（2 P0 / 26 P1 / 63 P2），再补一轮并发/进程健壮性与 PR #3 的两条 Codex 审查发现。
**前端契约零改动**，**未做 Tokio 迁移**（架构改动留待单独 PR）。逐项细节见
[CHANGELOG.md](CHANGELOG.md) 的 [1.2.0] 条目。

## 这版做了什么

### 1. 模块识别（28 条修复）

- **同路径热插拔**按 USB 身份指纹（VID:PID@端口）判定更换并重建，旧身份/方言不再永久残留；
  负探测缓存记录指纹，换上的新模组立即失效退避，不再等最长 300 秒。
- **无编号 MHI 通道**（`mhi_DUN`/`mhi_AT`）归入固定组，同一 PCIe 模组不再被注册成两台；
  PCIe 口按 BDF 分配 `wwan*` 名，补 `/sys/class/mhi_uds` 解析与大小写不敏感匹配。
- **厂商与 VID/PID 家族修正**：`mt57*`/目录明确条目才改判 TDTech、`mh5000-*` 按平台归属；
  SIMCom `1e0e` 走 VID 别名；补齐 Foxconn/Thales/Telit/Gosuncn/Neoway/Huawei 等 USB 家族与
  `MODEM_VIDS`/`DATA_DRIVERS`（含 `mhi_netdev`/`t7xx`/`pcie_mhi`）。
- **`option new_id` 只写 `option*` 驱动**，找不到时先 modprobe 重试，绝不写进 ch341/cp210x
  等无关驱动。
- 端口选择加入 `bInterfaceProtocol` 评分（0x12=PCUI/AT 优先，0x13/0x14 排除）与接口名整词
  评分；`/dev` 扫描补 `ttyAP*` 等 serialport 漏枚举的口。

### 2. 拨号 / 自愈（17 条修复）

- **探活移出全局会话锁**（摘出 → 锁外 `alive()` → 放回），一个模组控制口卡死不再阻塞其它
  模组；`reconcile` 加 single-flight，退出期在途会话不会写回会话表。
- 拨号首段状态写入加**代次守护**（`usb_type()` 耗时期间用户点断开不再被 `manual_stop=false`
  顶掉）；`dial_busy` 改 RAII，panic 不再永久卡住后续拨号。
- MBIM 去激活校验激活态并重试、start 失败补 deactivate；ACTIVATING 轮询到 ACTIVATED 才算
  成功；QMI 结果 TLV 0x02 缺失判失败；MBIM 重同步改逐字节滑动。
- **APN 四级回退修实**：过滤 IMS/sos 内部 APN，拨号成功单独记 `last_dial_apn`；运营商识别
  改 **ICCID（SIM 归属）优先**，漫游时不再把中国预制 APN 写给外国 SIM。
- 原生拨号区分「手动」与「自动」：`auto_dial=off` 只关自动重拨，页面点「拨号」仍生效；
  `manual_stop` 按 USB 身份落盘并在重启首轮恢复。
- 无 SIM / 未注册 / 无 APN 纳入**轻量门禁**（只读既有网络缓存，不新增 AT），不再周期性空拨；
  MBIM 缺 APN 给出显式 `no_apn` 停止原因。PR 审查后进一步收紧：注册查询失败（ERROR/超时）
  留下的占位 `stat=0` 不再被当作「明确未注册」误拦拨号。

### 3. 接口拉起 / 取址 / 路由（30 条修复）

- **主线 `qmi_wwan` 的 raw_ip 取址修复（P0）**：raw_ip 网卡不收 DHCP，改用会话上报地址走
  static（含会话真实子网掩码），MTU/网关/DNS 一并落 UCI，不再「已连接但永远没有 IPv4」。
- **强制 DNS 改 UCI list**（`delete + add_list`）——netifd 的 `dns` 是 list 型，空格分隔串会被
  整串丢弃；`parse_existing` 与 cleanup 的 list 聚合口径统一。
- 主机地址采集改 `ip -o addr` **双栈**并过滤 `fe80::/10`：v6-only 链路不再判「断开」，页面 v6
  地址与逐族路由检查恢复；`native_link_ok` 接受 v4 或 v6 任一有效地址。
- 断开链路补 `ifdown` 主/兄弟接口，`flush_stale` 增补 IPv6 全局地址与默认路由清理，「已断开」
  后 v6 仍可上网的问题消除。
- v6 落地动作以「真的建了兄弟接口」为前提；RS 节流改**单调时钟**，NTP 校时跳变不再让 RS
  停发数小时；`-P0` 仅在用户未显式给 `-P<n>` 时追加，不覆盖 `reqprefix`。
- `set_apn` 的 `pdp_type` 走白名单校验、回读比较忽略大小写；`is_usable_global_v6` 收紧到
  2000::/3；`cleanup` 缺席轮数按模组去重；归档片段转义单引号。
- **幂等补 ifup / 补 reload / diff apply 全部尊重 `manual_stop`**：用户点「断开」后，会话地址
  取不到导致 `plan_wan` 翻回 dhcp 的那套 UCI 改动不再下发，接口不会被重新拉起（PR #3 审查
  发现并修复）。
- `interface_down` 改解析 `ifstatus` 顶层 JSON `up`，嵌套字段不再掩盖真实 down。

### 4. 并发 / 进程健壮性（16 条修复）

- **WebSocket**：写超时 + 写失败立即摘除；pong 走统一写锁不与广播交错；服务端 ping + 读超时
  回收半开连接；屏端占串口指令按 `(模组,动作)` 去重并限制总并发 8。
- **网关**：并发连接上限 96（超限 503）、请求头不完整直接 400 关连接（截断头不再可能绕过
  `X-Real-IP` 覆盖），头后转发加双向空闲超时。
- 后台循环（urc/netprov/push/traffic/quota/change/hotplug/sim/logs + 巡检/定时/ifstat/ntp）
  统一用 `catch_unwind` 监护，panic 记日志并退避重启；线程数按实际接线统计。
- 任务注册去重与插入合到同一把锁（TOCTOU 消除）；`progress()` 不再把已取消任务写回 Running。
- 退出钩子先删 PID 再限时 5 秒清理，procd 重启不再被卡住的清理挡住；台账落盘改原子写
  （QuotaStore）/ 锁内快照锁外写（SchedStore）。

### 5. 前端与流水线

- LuCI 调试工具「预设 AT 命令」下拉为空修复：`presets.map(...)` 嵌套数组展平成单层，预设
  选项正常渲染（仅前端渲染，后端 `/api/at/presets` 未改动）。
- GitHub Actions 统一升级至 Node 24 运行时；全仓复扫 `node20`/`node16`/`node12` 引用残留为 0。

## 产物清单

| 文件 | 适用 |
| --- | --- |
| `modem-pro-1.2.0-r1_apk-mainline.apk` | OpenWrt 主线 / snapshots，包架构 **aarch64_generic** |
| `modem-pro-1.2.0-r1_apk-filogic.apk` | OpenWrt 主线 / snapshots，包架构 **aarch64_cortex-a53**（MT7981/7986 等） |
| `modem-pro_1.2.0-r1_aarch64_generic.ipk` | OpenWrt 24.10.x（opkg） |
| `SHA256SUMS.txt` | 上述产物的校验和（按合并后的实际文件重算） |

两个 APK 内容相同（同一份静态 musl 二进制、无内核模块依赖），只是包架构标签不同；
按设备 `apk --print-arch` 的输出选对应文件。包内含 `modemd`、netifd 协议脚本与 LuCI 前端。

## 安装

```sh
# OpenWrt 25.x（apk）
apk add --allow-untrusted /tmp/modem-pro-1.2.0-r1_apk-mainline.apk

# OpenWrt 24.10（opkg）
opkg install /tmp/modem-pro_1.2.0-r1_aarch64_generic.ipk
```

## 升级注意

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.2.0-1`）：
  `modemd` 的版本、包版本与页面显示的版本同源。
- **默认行为不变**：`dial_gate` 默认 `off`、`reset_before_dial` 默认关、壁纸随机切换默认关。
  本版凡新增的门禁/判定均以「不误拦」为原则：缓存缺失、查询失败、正在搜索一律放行。
- **用户断开语义加强**：`manual_stop` 现在落盘并跨重启恢复，断开后自动组网不再下发 UCI
  改动、不补 reload / ifup；页面点「拨号」即清除该状态并恢复自动组网。
- 无 API 路径与字段变更（`RegState.known` 为新增可选字段，原 `stat`/`text` 原样保留），
  无新增必填配置；`/etc/modemd/config.toml` 是 conffiles，升级不覆盖你的取值。
- **前端契约零改动**：ROUTES、`{code,msg,data}` 信封、WS 帧格式与 `X-Real-IP` 鉴权模型不变。

## 已知边界（如实说明）

- **多数修复经单测覆盖，但未经真机回归**：`cargo test --locked` **893 项通过**（0 失败），
  但 QMI/MBIM 实拨、多模组并发、v6-only 链路、热插拔等需真机验证的场景未上设备跑过。
- **需真机核对后改的项**（审计建议保留）：`qmap_size` sysfs 路径、QMI 4096 读缓冲、
  `write_frame` 部分写（B-P2-13）；350 系列 static 仍按 /24 + 派生网关（C-11/C-20，
  /32 + onlink 需实机验证）；hostnic 一键方案与 netprov 点对点 static 是两套语义（C-21）。
- **有意保留不改**：static-v6 默认路由仍由本程序装、RA 默认路由不受 `wan_metric` 控制
  （多 WAN 策略路由是路线图项）、`uci::apply` 失败即停不回滚、`force_link` 只写不清。
- **未做 Tokio 迁移**：AT 串口、QMI/MBIM 同步 IO、tiny_http handler 仍同步；计时编排层与
  WS 推送层的异步化为后续架构工作，本次只在同步模型内修完并发缺陷。
- `cargo fmt --check` 与 `cargo clippy` 仍有存量漂移/告警（历史文件，非本版改动，未越界重排）。
- 发布说明基于本地 x86_64 验证；musl 交叉编译与产物由 CI（OpenWrt SDK）完成，
  以 Actions 运行结果为准。

## 文档

`CHANGELOG.md` 的 [1.2.0] 条目（本版全部细节，含各条的保留项与未验证说明）。
