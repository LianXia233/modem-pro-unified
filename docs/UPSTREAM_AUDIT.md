# 上游审计与本次迁移对照（2026-10-20）

> 审计对象是三个仓库的**真实源码**，不是 README 或历史总结。
>
> | 仓库 | HEAD | 角色 |
> | --- | --- | --- |
> | [FUjr/QModem](https://github.com/FUjr/QModem) | `e00a306`（2026-10-20） | 模组知识底座：清单 / 端口规则 / 厂商 AT 脚本 / 拨号脚本 |
> | [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro) | `397e014`（2026-10-17） | Go 原版：发现/分组/串口策略/状态机 |
> | 本仓库 `modem-pro-unified` | `5a0145a`（2026-10-20 起点） | Rust 单体后端 + LuCI + WebUI |
>
> 本文只写**能落到文件行号**的结论。没核到的写「未实现」或「未验证」，不写「已支持」。

---

## 1. 审计范围

| 区域 | QModem | modem-pro（Go） | 本仓库（Rust） |
| --- | --- | --- | --- |
| 设备发现 | `application/modem_scan/src/modem_scand.c`（1654 行） | `module/kernel/modem_discover.go` + `module/netdev/ports_unix.go` | `rust/src/modem/discovery/`（原 113 行）+ `manager.rs` |
| 端口规则 | `modem_port_rule.json`（18 条） | `PickATPort` 四级优先级 | `modem_ports.json` + `ports.rs` |
| 型号清单 | `modem_support.json`（90 usb + 41 pcie + 9 device） | `module/catalog/modem_catalog.json`（91） | `modem_catalog.json`（91） |
| 厂商方言 | `cmds/*.sh`（12 家）+ `vendor/*.sh`（12 家） | 14 个厂商包 + `modelquirk` | 14 个 `vendor/*.rs` |
| 拨号 | `modem_dial.sh`（62 KB，`at_dial` / `ecm_hang` / `qmi_dial`） | `dial_engine.go` + `hostdial*.go` | `model/dial.rs` + `runtime/dialer/` |
| 组网 | `modem_dial.sh:757-1020`、`qmodem_runtime.sh` | `netprov_*.go` | `netprov/` + `files/lib/netifd/proto/modem_pro.sh` |
| 测试 | `tests/*.sh`、C 单测 | 91 个 `_test.go` / 1084 个 `Test*` | 740 个 Rust 单测（迁移前 703） |

---

## 2. 功能迁移矩阵

状态口径：**已实现**＝代码合入且有自动化测试；**已有**＝本次审计前就正确实现；
**未实现**＝本次没做，附原因/后续入口；**不适用**＝上游实现与本项目架构冲突且无收益。

### 2.1 设备发现与端口选择

| # | 能力 | 上游依据 | 本仓库改动前 | 本次处置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| D1 | USB **接口号白名单**（`include`，完整串 `1.2` 精确匹配） | QModem `modem_scand.c:621,563` | `ports.rs` 只在**识别**时查规则，发现流程从不看它 —— diag/NMEA 口照样被探 | 新增 `discovery/plan.rs::plan()`：白名单外直接跳过并记录原因 | 已实现 |
| D2 | **语音 PCM 接口**排除 | QModem `modem_scand.c:629` | 未处理（`1.1` 会被当 AT 口） | `plan()` 剔除 `voice_pcm_interface` | 已实现 |
| D3 | **物理设备分组**（一台模组多个 tty 只登记一次） | Go `ports_unix.go:112-148` `GroupByUSBDevice` | 每个 tty 单独探活、单独成模组（RM520N 会出 2~3 个「模组」） | `plan()` 按 `usb_dev` 分组；`scanner.rs` 每组只返回一个口 | 已实现 |
| D4 | `option_driver` **枚举前补绑** | QModem `modem_scand.c:570` | 只有 `/api/usb/bind` 手动触发 | `usbdev::option_bind_candidates()` + `manager::auto_bind_option()`（5 分钟冷却、受 `usb_autobind` 控制） | 已实现 |
| D5 | **身份级探活**（`AT` 回 OK ≠ 命令口） | Go `ports.go:165-252` | `AtPort::probe()` 只看 `contains("OK")` | 新增 `AtPort::probe_identity()`：CGMM→CGMI→ATI 至少一条内容行 | 已实现 |
| D6 | **PCIe/MHI AT 口**枚举 | Go `ports_unix.go:38-51`、QModem `:695-761` | `is_at_candidate()` 只认 ttyUSB/ttyACM，MHI 口永远发现不了 | 并入 `pcidev::pcie_at_port_candidates()`；`is_at_candidate` 支持 `mhi_*DUN/AT/uci_q`、`wwan*at*` | 已实现 |
| D7 | **裸字符设备**读写（MHI 口不是 tty） | Go `atdev.go:90-143` | `serialport` 打开即 `ENOTTY`，PCIe 模组一个都认不出 | `AtPort` 后端抽象成 `AtIo`，Linux 上退到非阻塞裸读写 | 已实现 |
| D8 | 非 AT 口**否定缓存 + 指数退避** | Go `modem_serial.go:307-343` | 无：巡检 3s 一轮，每轮对「不是模组」的 tty 重发 4 条 AT | `scanner.rs` 进程级否定缓存：15→30→60→120→240→300s，端口消失即清（重插立即重探） | 已实现（**无 90s 预热窗口**：上游按 sysfs mtime 判断「新设备」固定 15s 重探，本实现未取 mtime） |
| D9 | 卡死救援（150s 无 AT → USB `authorized 0→1` / PCIe 重扫） | Go `modem_serial.go:352-428` | 无 | `manager.rs`：`[serial] auto_rescue`（**默认关**）＋150s 触发、5 分钟冷却、每轮最多 3 次；判定为纯函数并有测试 | 已实现（默认关闭） |
| D11 | PCIe 模组**补绑驱动**（MHI / T7xx） | Go `pcie_unix.go:86-104` | 只有 `/api/pcie/bind` 手动 | `manager::auto_bind_pcie()`：`[serial] pcie_autobind`（默认开）＋5 分钟冷却 | 已实现 |
| D12 | **多 tty 拓扑**对上层可见 | Go `Modem.Ports` | `tty_list` 恒为「当前口自己」 | `Modem::set_ttys()`；发现流程写入「首选＋回退」，切口后重排 | 已实现 |
| D13 | 同型号多台设备的**物理路径绑定** | QModem 按 UCI slot；Go 仅 `vid:pid` | 只按 `vid:pid`：两台同型号会共用一条绑定 | 新增 `vid:pid@1-1` 键（`at_port_bind_key_scoped`），旧键兼容；API 增 `bus_path`，WebUI 同步 | 已实现 |
| D10 | 热插拔事件（udev/hotplug） | QModem `etc/hotplug.d/usb/20-modem-usb` | 周期发现 + `reap_gone()` | 保持现状：OpenWrt 上周期发现已覆盖；事件通道是后续项 | 不适用（本期） |

### 2.2 拨号与数据通道

| # | 能力 | 上游依据 | 改动前 | 本次处置 | 状态 |
| --- | --- | --- | --- | --- | --- |
| P1 | **ECM/RNDIS/NCM 厂商激活指令** | QModem `modem_dial.sh:1286-1482` | 一律 `AT+CGACT=1,<cid>`（移远/华为/美格那几家网卡没 carrier） | 新增 `vendor/datacall.rs` + 9 家厂商实现（QNETDEVCTL / GTRNDIS / NDISDUP / SETAUTODIAL / QCRMCALL / MYUSBNETACT / ICMAUTOCONN / ZECMCALL / RNDISCALL） | 已实现 |
| P2 | **挂断与激活对称** | QModem `ecm_hang()` `:1122-1165` | 只发 `AT+CGACT=0`，数据通道不断开 | `hangup()` 按同一配方发 deactivate；无配方时才用 CGACT | 已实现 |
| P3 | QMI/MBIM **不在模组侧激活 PDP** | Go `hostdial_sync.go:44-125` | 已正确（`wan_host_dial_by_daemon` 提前返回） | 保持不变，补注释 | 已有 |
| P4 | FM350 系列**静态地址**（模组不发 DHCP） | QModem `modem_dial.sh:1723-1750`、Go `netprov_hoststatic.go` | 已实现（`netprov/provision.rs:151-176` + `hostnic::quirk`） | 保持不变 | 已有 |
| P5 | APN / cid / PDP 类型校验 | Go `dial_engine.go:270-306` | 已实现（`valid_cid` / `valid_apn` / `valid_pdp_type`） | 保持不变 | 已有 |
| P6 | 拨号前置体检（SIM/注册/网卡） | QModem `modem_dial.sh:570-575` | 已实现（`dial_readiness()`，失败时写进错误） | 保持不变 | 已有 |
| P7 | QMI/MBIM 会话建立/释放 | Go `hostdial*.go` | 已内化（`runtime/dialer/native/`） | 保持不变 | 已有 |
| P8 | 连接阶段分层（session / iface / route / online） | 本项目自研（历史修复） | 已实现（`DialState::phase`） | 保持不变 | 已有 |
| P9 | PPP 数据面 | QModem 无（`ppp` 只是认证命令） | 无 | 不实现，并在支持矩阵里明确标注 | 不适用 |
| P10 | 拨号**失败退避** | Go `hostdial_run.go:39-45`（10s 起、翻倍、封顶 60s） | 固定 5 秒 | `retry_delay()`：5/10/20/40/60s，成功即清零；纯函数 + 测试 | 已实现 |
| P11 | 清单形态核对（协议是否被型号支持） | Go `catalog.go:555-585` | 无 | `shape_note()`：清单里 USB/PCIe 形态都不含当前 kind 时写一步提示（不拦截），有测试 | 已实现 |
| P12 | 断开后**清理旧地址/旧路由** | Go `hostdial_posix.go:108-139` | 只改状态：内核里的旧 `default` 与地址还在（黑洞） | `netprov::route::flush_stale()`：逐条删经该网卡的默认路由 + `ip -4 addr flush`；只动本模组网卡；`dial_disconnect` 调用并写步骤 | 已实现 |

### 2.3 文档与契约

| # | 问题 | 事实 | 处置 |
| --- | --- | --- | --- |
| C1 | README 链接的 `docs/INTEGRATION.md`、`docs/API.md` 等 6 个文件不存在 | `git log` 显示 `66853b8 Delete docs directory` 一次性删掉 10 个文档（5777 行），README 未同步 | 按**当前代码**重写 `INTEGRATION.md` / `BUILD.md` / `API.md`，新增本文件与 `SUPPORT_MATRIX.md`，README 与文档索引同步；旧文不恢复（内容过时） |
| C2 | 代码注释多处引用 `docs/INTEGRATION.md` / `docs/API.md` | 同上 | 三份文档已重写，引用重新有效；`PORTING.md` 的两处引用改为 `API.md` |

---

## 3. 代码变更清单

| 文件 | 变更 | 说明 |
| --- | --- | --- |
| `rust/src/modem/vendor/datacall.rs` | 新增（≈330 行） | 数据通道配方类型 + 查表 + 9 项测试 |
| `rust/src/modem/vendor/mod.rs` | +18 行 | `VendorSpec::data_channel()` 默认实现（默认 None） |
| `rust/src/modem/vendor/{quectel,fibocom,huawei,tdtech,meig,neoway,telit,gosuncn,simcom,openluat}.rs` | 各 +30~50 行 | 各家按平台给激活/挂断指令 |
| `rust/src/modem/model/dial.rs` | 改 `dial_inner` ②/②b、`hangup`、+helper | 接入配方；保留 FM350 cid 消失补写 APN 的历史修复；失败回退 CGACT |
| `rust/src/modem/discovery/plan.rs` | 新增（≈330 行） | 纯函数：分组 / 白名单 / 排序 / 跳过原因 + 7 项测试 |
| `rust/src/modem/discovery/ports.rs` | 重写（84→200 行） | sysfs 快照（usb_dev/iface/driver/vid:pid）、PCIe AT 口、伪分组 |
| `rust/src/modem/discovery/scanner.rs` | 重写 | 按计划探活，一台设备一个口，返回证据/跳过原因 |
| `rust/src/modem/discovery/mod.rs` | 导出调整 | |
| `rust/src/modem/manager.rs` | 发现流程 + 补绑 + 冷却 | 日志带 group/规则/证据 |
| `rust/src/modem/usbdev.rs` | +`option_bind_candidates()` | 按端口规则找「该补 option 但没 tty」的设备 |
| `rust/src/modem/at/port.rs` | +`probe_identity()`、`AtIo` 后端、裸字符设备 | MHI/wwan AT 口可打开 |
| `rust/src/modem/at/command.rs` | 去掉不再使用的 `Read/Write` 导入 | 后端抽象后由 `AtIo` 负责 |
| `docs/UPSTREAM_AUDIT.md` / `docs/SUPPORT_MATRIX.md` / `docs/README.md` | 新增 | 审计与支持矩阵 |
| `rust/src/modem/manager.rs` | 救援 / PCIe 补绑 / 形态核对接线 | `auto_bind_pcie`、`maybe_rescue`、`set_ttys` |
| `rust/src/modem/model/modem.rs` | 多 tty 拓扑 | `ttys` 字段、`set_ttys`、`view().tty_list` |
| `rust/src/runtime/store.rs` | `vid:pid@bus` 绑定键 | `at_port_bind_key_scoped` |
| `rust/src/netprov/route.rs` | 断开清理 | `default_route_specs` / `flush_stale` + 2 项测试 |
| `rust/src/http/handlers/dial_disconnect.rs` | 断开后清地址/路由 | 写一步「清理主机网卡旧地址/路由」 |
| `rust/src/runtime/dialer/mod.rs` | 指数退避 | `retry_delay` + 失败计数 |
| `rust/src/http/handlers/atport_bind.rs` | `bus_path` 参数 | 兼容旧键 |
| `rust/web/index.html` | 绑定 UI 按设备物理路径 | 下拉值带 `bus_path` |
| `files/etc/modemd/config.toml` | 新增 `pcie_autobind` / `auto_rescue` | 默认 true / false |
| `CHANGELOG.md` | 新增条目 | 见 `[未发布]` |

没有改动：HTTP API、WebSocket 字段、配置文件、LuCI 协议、netifd 脚本、打包脚本。

---

## 4. 测试与验证

| 命令 | 结果 |
| --- | --- |
| `cargo test --locked` | **740 passed / 0 failed**（703 → 732 → 738 → 740；本任务新增 7 个测试、删除 2 个重复/空壳用例） |
| `cargo build --release --locked` | 通过（见构建报告） |
| `cargo clippy --all-targets --locked` | **0 warning**（原迁移时存量 7 条，本轮全部清零：`cells.rs` 注释结构、`route.rs` 空串比较、`siminfo.rs` map 恒等、`dial.rs` manual_contains） |
| `cargo fmt --check` | 全仓干净（原迁移后曾用独立提交统一过格式；本轮新增/改动的 15+ 个文件含脚本注入代码块，已重新统一并复验） |
| `make package/modem-pro/compile` | **未执行**：本沙箱没有 OpenWrt SDK/feeds 与交叉工具链（`STAGING_DIR`、`TOOLCHAIN_DIR` 均不存在）。属环境限制，不是跳过 |

新增测试的覆盖点：

- `datacall::tests::vendor_table_matches_qmodem_dial_script`：10 组厂商/平台的**命令行字面量**与 QModem 脚本逐字对齐；
- `datacall::tests::activate_and_deactivate_are_symmetric`：所有厂商 × 6 个平台，有激活必须有挂断（合宙例外必须在 note 里声明）；
- `datacall::tests::unknown_platform_never_guesses` / `qmi_and_mbim_have_no_at_plan` / `cid_is_substituted_everywhere`；
- `discovery::plan::tests::*`：RM520N 白名单 + PCM 剔除、同型号双设备分组、未知设备仍可选口、自然序、白名单缺失时**宁可空等**、MHI 独立分组；
- `discovery::scanner::tests::backoff_grows_then_caps` / `warmup_keeps_probe_interval_short`：15→300s 退避与 90s 预热窗口；
- `manager::tests::rescue_*`：150s 触发、开关、5 分钟冷却、3 次上限；
- `model::dial::tests::shape_note_only_warns_on_real_mismatch`：清单形态核对；
- `store::tests::scoped_bind_key_separates_identical_models`：`vid:pid@bus` 键与路径穿越拒绝；
- `model::modem::tests::tty_list_follows_current_port`：多 tty 拓扑与切口排序；
- `dialer::retry_tests::retry_delay_backs_off_then_caps`：5→60s 指数退避；
- `discovery::ports::tests::describe_missing_port_is_empty_but_safe`：无 sysfs 时不 panic。

---

## 5. 真机待办（按优先级）

本会话没有 5G 模组、运营商 SIM 与 OpenWrt 目标机，以下项目**未做**，不能声称通过：

1. **P1/P2 实机验证**（最关键）：
   - 移远 RNDIS（如 RM520N-GL 切 `AT+QCFG="usbnet",3`）→ 期望 `AT+QNETDEVCTL=3,<cid>,1` 后 `wwan0`/`usb0` 出 carrier，`AT+CGPADDR=<cid>` 有地址；
   - 广和通 FM350-GL（MTK）→ 确认仍走 `AT+CGACT`，静态地址由 `netprov` 写入；
   - 华为 E8372 / ME906（巴龙）→ `AT^NDISDUP=1,<cid>`。
2. **D4**：找一台 `05c2:...`/`1782:...` 需要 option 补绑的设备，确认发现前补绑后 tty 出现且能识别。
3. **D6/D7**：PCIe 模组（FM150 PCIe / RM500Q PCIe）确认 `/dev/mhi_*DUN*` 能被 `probe_identity` 打开并识别。
4. **D3**：RM520N 上确认 `/api/modems` 只出现一台，而不是 2~3 台。
5. **D8/D9**：如现场确实出现「巡检把 5G 慢启动模组判死」或「USB 假死」，再按 Go 版参数（15s 预热 / 150s 救援）补上，并带上开关。

---

## 6. 与上游冲突时的取舍

| 冲突点 | 选择 | 理由 |
| --- | --- | --- |
| QMI/MBIM 数据面归属 | 保持本仓库的**内置 Rust 拨号器**，不用厂商 CM | 已有 `native/qmi_link.rs`、`mbim_link.rs` 且通过了既有测试；引入外部二进制会与现有 netifd 协议重复控制同一会话（Go 版自己也记录了 `QMUXError 0xe`） |
| 非 QMI 拨号 | 采用 **QModem 的厂商指令表**，不以 Go 版为准 | Go 版这条路径只有 `AT+CGACT`，无法解释「PDP active 但网卡无 carrier」；QModem 的指令有 14 家实测脚本可对照 |
| 平台不明时的行为 | 走 3GPP 通用路径，不发私有指令 | 与项目既有红线「未取证就不发」一致；失败也不会伪装成功（仍要过 CGPADDR + 网卡就绪校验） |
| 端口白名单严格度 | 白名单外**不选**，宁可这一轮不登记 | 选到 diag 口的代价是「能打开、全超时」，比晚一轮发现更糟 |

---

## 7. 追加审计（2026-10-21）：上游对齐实改、冗余收敛与引用卫生

在 §1–§6 的基础上继续逐条核对，本轮落地的结论如下。**原则不变**：
能落到行号的才写；单一来源不可证的写成「有意差异」或「未验证」，不改代码。

### 7.1 上游对齐实改（3 处，均有测试）

| # | 改动 | 上游依据 | 性质 |
| --- | --- | --- | --- |
| U1 | **QNW 5G 锁小区参数错位修复**：4G/5G 两套语法混用导致 NR 锁小区每个参数错一位。`parse_qnwlock` 增 NR 分支（PCI 在最前）、`set_cell_lock` 增 `band` 参数、NR 只接受 `lock_type=1`、band 缺省用缓存驻网频段兜底（不发 AT）；API/LuCI/文档/校验层全链路同步 | MikroTik ROS 文档实测回读 `+QNWLOCK: "common/5g",138,628032,30,78` | **重大 bug 修复** |
| U2 | **华为展锐挂断前先探自拨状态**：`AutodialProbe { query, fallback }` 挂到 `DataChannel`，unisoc 填 `AT^SETAUTODIAL?` / `AT^NDISDUP=0,0`；未开自拨时不碰射频 | QModem `modem_dial.sh:1546-1568` `auto_dial_hang_huawei_unisoc` | 行为对齐 |
| U3 | **发现退避年轻期清零**：年轻期失败不计入 `tries`，出预热期从基值重数 | Go `modem_serial.go:307-343` `nonATNextProbe` | 参数对齐 |

QModem 按平台分的 QNWLOCK 变体（unisoc 的 `common/lte` scope、`lte` 平台
lock_type 1/2 反向语义）**单一来源不可证**，按「未验证不盲写」保留现状，
已在 `lock.rs` 注释里声明。

### 7.2 冗余收敛（9 处重复实现合一，均不改变对外契约）

sysfs 小工具 → `modem::sysfs`；JSON 台账读写 → `store::{load_json,
write_json_atomic}`；`fmt_bps` → 只留 `model::fmt_bps`（主机侧/模组侧
文案必须一致）；锁频段入参 → `handlers::band_lock_params`（页面与定时
任务共用，杜绝「一半入口生效」）；IMEI 收尾 → `handlers::imei_done`；
串口写字节 → `AtPort::write_bytes`；`DialRequest` 构造 →
`dial_request_from_params`；会话校验 → `auth::session_valid`（ubus 不可用
时统一放行 + warn，防认证半边缺口）；删除死代码 3 处
（`ca.rs::empty_note`、`mbim.rs` `v6_gw_off`、`params.rs` `was_active`）。

### 7.3 引用卫生与有意差异文档化

- 修正 1 处**错误文件前缀**：代码注释原引用 `cmds/modem_dial.sh:454-481`，
  实际 `cmds/` 下只有 14 行 helper 存根，主脚本在
  `usr/share/qmodem/modem_dial.sh`（1844 行）——引用已改，并抽查
  `cell.go:36` / `net.go:667-678` / `net.go:157` 确认其余引用准确；
- 逐文件补上游行号依据（Go `sched_view.go` / `cell.go` / `net.go` /
  `data_validate.go` / `netprov_proto.go`、QModem `modem_dial.sh`、
  MBIM 1.0 §10.5.29），并把 5 处**有意差异**写进注释：CESQ 换算差 1 dB
  （取下界）、QENG 信号固定列不改量纲定位、`wan_mode` 无 `modemProtoOK`
  开关、拨号不发 `AT+COPS=0,0` / `AT+QCFG="nat"`、体检只读
  `AT+CFUN?` 不补 `=1`。

### 7.4 剩余可调研项（已记录为有意差异，未实改）

| 项 | 现状 | 阻塞原因 |
| --- | --- | --- |
| QModem 按平台分的 QNWLOCK 变体 | 文档化保留 | 单一来源不可证 |
| 移远 `AT+MGAUTH` / 华为 `AT^AUTHDATA` 的 PAP/CHAP | 不做 | `DialRequest` 无 auth 字段，属新增契约，需先定 API |
| neoway 挂断后 `sleep 3` 的节拍 | 不做 | 节拍类差异无功能收益，且会拖慢所有挂断路径 |

---

## 8. 追加审计（2026-10-21 续）：OpenVohive QMI 会话边界核对

参考仓库 [6106757-lab/openvohive](https://github.com/6106757-lab/openvohive)
HEAD `6c1cc74`（PolyForm Noncommercial，**只作协议行为基准，未拷贝代码**）。
本轮把其 `pkg/quectel-qmi/qmi/wds.go` 的 Start Network / Get Runtime
Settings / WDA / Bind Mux 编解码与本仓库 `rust/src/runtime/dialer/qmi.rs`
逐条对照，并用两个独立来源仲裁编号分歧：libqmi
`data/qmi-service-wds.json` / `qmi-service-wda.json`（freedesktop 官方
镜像，LGPL-2.1+，仅取协议编号）与厂商拨号器 C 源码（quectel-CM
`QMIThread.c` / `MPQMUX.h`；Fibocom `Fibocom-dial_Linux_Tool/QMIThread.c`
——与 FM350 同厂，参考价值最高）。

### 8.1 TLV 编号裁决：`wds_start` 本仓库编号正确，OpenVohive 标签有误

| TLV | 本仓库 | libqmi 定义 | quectel-CM / Fibocom dial | OpenVohive | 结论 |
| --- | --- | --- | --- | --- | --- |
| `0x30` | Technology Preference，uint8 `[0x01]`（3GPP） | Technology Preference（guint8，`ALLOW_3GPP=1`） | `0x30`，len 1，`0x01`/`0x02` | **Profile Index** | 本仓库正确；OpenVohive 把 profile index 写进了技术偏好的位置 |
| `0x31` | Profile Index（`pdn > 0` 时发） | Profile Index 3GPP（guint8） | — | — | 正确 |
| `0x34` | 未使用 | Extended Technology Preference（guint16 位图） | — | "Technology Preference" uint16 | OpenVohive 用的是扩展字段，与 `0x30` 不是同一个字段；不采纳 |
| `0x14` | APN | APN（string） | `0x14` | `0x14` | 一致 |
| `0x17` / `0x18` | Username / Password | 同名 string | `0x17` / `0x18` | 同名 | 一致 |
| `0x16` | Authentication Preference，uint8 | Authentication Preference（guint8） | `0x16`，len 1 | `0x16` | 一致（某处 PDF 流传的 `0x15` 说法不成立：libqmi 明确 `0x15` = IPv4 Address Preference） |
| `0x19` | IP Family，uint8（4/6） | IP Family Preference（guint8） | 同 | `0x19` | 一致 |
| 应答 `0x01` | Packet Data Handle | Packet Data Handle（guint32） | 同 | `0x01` | 一致 |

OpenVohive 不发 `0x30`、改发 `0x34` 扩展技术偏好，模组不支持扩展字段时只能
吃默认值；本仓库显式发 `0x30=[3GPP]`，与 quectel-CM / Fibocom dial /
libqmi 主流一致，保留。

### 8.2 Get Runtime Settings：mask 与应答 TLV 全部有据

- 请求 mask `0x2310` = DNS bit4 / IPv4 地址 bit8 / 网关 bit9 / MTU bit13，
  即 quectel-CM `WdsGetRuntimeSettingReq()` 里用得上的四位；PCSCF 两位
  （bit11/bit14）本项目不消费，**有意不发**（`qmi.rs` 注释已改为精确
  描述，不再笼统写"与 quectel-CM 相同"）。
- 应答 TLV：`0x15`/`0x16` DNS、`0x1E` IPv4、`0x20` 网关、`0x21` 掩码、
  `0x29` MTU 与 quectel-CM `MPQMUX.h` 的 `QMIWDS_GET_RUNTIME_SETTINGS_TLV_TYPE_*`
  逐值相同；v6 的 `0x25`/`0x26` 同样有据。

### 8.3 WDA Set Data Format / Bind Mux Data Port：逐条对上

WDA `Set Data Format`（`0x0020`）TLV 顺序与 libqmi + quectel-CM
`WdaSetDataFormat()` 一致：`0x10` QoS=0（no-QOS）、`0x11` 链路层协议=2（IP）、
`0x12`/`0x13` 上下行聚合协议=QMAP 版本、`0x15` 下行最大报文数=
`rx_urb_size/512`、`0x16` 下行最大聚合尺寸=`rx_urb_size`、`0x17` 端点
（ep_type + iface_id）。聚合门限 `0x19`/`0x1B`/`0x1C` 只在 quectel-CM 定义
`QUECTEL_UL_DATA_AGG` 时下发，本项目同样不发（`qmi.rs` 注释已记录依据）。
Bind Mux Data Port（`0x00A2`）：`0x10` 端点（8 字节）、`0x11` mux id
（1 字节）、`0x13` client type=1（TETHERED），与 quectel-CM
`WdsSetQMUXBindMuxDataPort()` 逐字节同构。

### 8.4 本轮实改（1 处，有测试）

| # | 改动 | 上游依据 |
| --- | --- | --- |
| Q1 | **IPv6 client 上也取 Get Runtime Settings**：原来只向 v4 client 取运行时设置，状态页 `modem_ipv6` 永远为空。现在 v6 呼叫成功后按同一 mask 向 `SVC_WDS_IPV6` 再取一次，回填 `ipv6`/`ipv6_gateway`（不覆盖 v4 的 MTU/地址；v6 失败仅 debug 日志，不影响拨号结果） | quectel-CM `requestGetIPAddress()` 对 v4/v6 两个 client 分别请求，且注释明示 "the following mask also applies to IPV6"；测试 `qmap_session_sequence_matches_quectel` 假设备新增 v6 应答并断言回填 |

### 8.5 有意不采纳 / 待评估

| 项 | 结论 |
| --- | --- |
| OpenVohive `0x30=Profile Index` | 不采纳：与 libqmi / quectel-CM / Fibocom dial 三处冲突，见 §8.1 |
| OpenVohive `StopAnyNetworkInterface`（残留会话 escape hatch） | 待评估：本仓库 `stop()` 已覆盖正常释放路径；需实机残留会话场景才能判断是否补 |
| OpenVohive call-end reason 诊断（应答 `0x10`/`0x11`） | 待评估：诊断价值明确，但属新增对外字段，需先定 API 契约再动 |

---

## 9. 追加审计（2026-10-21 续二）：Siriling 多驱动（RMNET / GobiNet / MHI / PCIe）

参考仓库 [Siriling/5G-Modem-Support](https://github.com/Siriling/5G-Modem-Support)
HEAD `b1d9a12`（各子包 GPL-3.0，**只取事实依据、未拷贝代码**）。它是一个
OpenWrt 包集合：6 个内核驱动包（`gobinet` / `pcie_mhi` / `pcie_mhi_fb` /
`qmi_wwan_q` / `qmi_wwan_f` / `sprd_pcie`）、2 个拨号工具
（`quectel_cm_5G` / `fibocom-dial`）、若干 LuCI 应用。本轮把它与
`rust/src/modem/pcidev.rs`、`rust/src/runtime/dialer/native/mod.rs`
的驱动识别 / 网卡命名 / QMAP 窗口口径逐条对照。

### 9.1 已被本仓库覆盖（核对通过，无需改动）

| Siriling 侧事实 | 来源 | 本仓库对应 |
| --- | --- | --- |
| 移远厂外树 MHI 数据网卡叫 `rmnet_mhi`，`luci-app-pcimodem` init 脚本写死 `-i rmnet_mhi0` | `quectel_MHI/src/devices/mhi_netdev.c:615`；`luci-app-pcimodem/root/etc/init.d/pcimodem` | `is_likely_mhi_face` 的 `rmnet` 前缀 |
| MHI 控制口节点名 `mhi_<通道名>`（`mhi_QMI0` 等） | 两个 MHI 驱动的 `device_create(..., "mhi_%s...")` | `pcie_qmi_control_candidates` 的 `mhi_*QMI*` 等 glob |
| GobiNet 走自己的数据格式、拨号器显式跳过 WDA | `gobinet` 包（GobiUSBNet.c） | `ControlKind::GobiNet` + `find_gobinet` + `qmap_settings` 的 `handles_data_format` 门 |
| `qmi_wwan_q` / `qmi_wwan_f` 的 QMAP 版本 5、每网卡 `qmap_size` 属性、USB3 缺省 16 KiB | `qmi_wwan_q.c` / `qmi_wwan_f.c` 的 `DEVICE_ATTR(qmap_size)`、`qmap_version = 5` | `QMAP_VERSION=0x05`；`qmap_size()` 先读每网卡属性；缺省 16 KiB |
| fibocom-dial 的 WDS TLV 编号（`0x30` 技术偏好 / `0x14` APN / `0x17` `0x18` / `0x16` 认证 / `0x19` IP 族）、GetRuntimeSettings mask 四位、BindMux `0x10/0x11/0x13` | siriling 自带 `fibocom-dial/src/QMIThread.c` 与 `MPQMUX.h` | 与 §8.1–§8.3 结论**再次一致**（第二份独立拷贝），我方编号无需改动 |

### 9.2 本轮实改（2 处识别缺口 + 1 处注释依据，均有测试）

| # | 改动 | 上游依据 |
| --- | --- | --- |
| S1 | **认 `fibo_mhi` 驱动**：`PCIE_MODEM_DRIVERS` 与 `pci_driver_is_mhi` 新增 `fibo_mhi`——广和通厂外树 MHI 既不以 `mhi` 开头也不是 `pcie_mhi`，原来会被当成无线网卡漏掉 | `fibocom_MHI/src/Makefile` `obj-m := fibo_mhi.o`（PKG_NAME `pcie_mhi_fb`） |
| S2 | **认 Fibocom MHI 数据网卡名**：`is_likely_mhi_face` 新增 `pcie_mhi*` / `pcie_swip*` 两个前缀 | `fibocom_MHI/src/devices/mhi_netdev.c:1249-1253` 的 `interface_name` |
| S3 | **`qmap_size()` 注释校正**：原注释称 qmi_wwan_q / qmi_wwan_f 把窗口挂成**模块参数**，两处源码证明它们只暴露**每网卡**属性（取值来自 USB ID 表，如 16/31 KiB）；模块参数路径降级为「防御性保留」并注明最终值会被 WDA 应答回读覆盖 | `qmi_wwan_q.c:448` / `qmi_wwan_f.c:372` 的 `DEVICE_ATTR(qmap_size)`；`qmi_wwan_q.c:2141` 的 ID 表取值 |

新增测试 `fibocom_mhi_faces_are_recognised`（网卡名）并扩充
`pci_id_and_driver`（驱动名）。

### 9.3 有意不做（范围外，记录在案）

| 项 | 结论 |
| --- | --- |
| SRPD / 展锐 PCIe（`sprd_pcie` / `mcd`，VID `0x16c3`，sipc/sblock 通道栈） | **不做**：没有 QMI/MBIM 控制面，本仓库的原生拨号器（QMI/MBIM）与 AT 配方都不适用；强认只会让设备「可见但不可用」。真要做是独立后端，超出本次整合范围 |
| `quectel_cm_5G` | 与已审计的 quectel-CM 同源，无新口径 |
| 各 LuCI 应用（hypermodem / usbmodem / pcimodem / gobinetmodem / spdmodem） | 它们是「整套 UI」，本仓库是 modemd + LuCI 前端架构，无可借鉴契约；`pcimodem` 仅用于确认 `rmnet_mhi0` 命名（见 §9.1） |

---

## 10. 上游致谢

本项目在开发过程中参考了以下仓库（按对齐基准排序）：

| 仓库 | 参考内容 | 许可 |
| --- | --- | --- |
| [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro) | Go 原版 `modemd`：HTTP/WS 接口契约、JSON 字段、AT 解析口径、配置项、状态机、组网流程。本仓库以上游为逐字兼容基准（接口名对照结论见 CHANGELOG [1.0.3]） | 未声明 |
| [ouyangzq/luci-proto-modem-pro](https://github.com/ouyangzq/luci-proto-modem-pro) | `modem_pro` netifd 协议：IPv4 + IPv6 单接口承载、`accept_ra=2`、QMI 数据面、前缀漂移等实机注意事项 | 未声明 |
| [FUjr/QModem](https://github.com/FUjr/QModem) | 模组知识底座：型号清单、USB 接口号白名单与端口规则、语音 PCM 接口排除、厂商 AT 方言与拨号脚本、发现退避策略 | MPL-2.0（附禁止商用附加条款） |
| [LianXia233/luci-app-fm350](https://github.com/LianXia233/luci-app-fm350) | FM350 / RW350 系列模组的识别与 AT 方言、锁频/锁小区、IMEI MFG 块备份恢复口径 | 未声明 |
| [6106757-lab/openvohive](https://github.com/6106757-lab/openvohive) | QMI/MBIM 会话边界对照基准：WDS Start/Stop/Runtime、WDA Set Data Format、Bind Mux 的 TLV 编号与流程；据此修正 IPv6 运行时设置缺口（§8.4） | PolyForm Noncommercial 1.0.0（仅参考，未拷贝） |
| [hchyhchyxh/luci-app-rg501q-eu](https://github.com/hchyhchyxh/luci-app-rg501q-eu) | RG501Q-EU（`2c7c:0800`）的 USB 接口号口径（AT 口 1.2/1.3、QMI 1.4）、不绑 option 驱动的端口规则；据此新增 `modem_ports.json` 条目 | Apache-2.0（仅事实依据，未拷贝） |
| [ainuoyan/QDC507-Modem-Support](https://github.com/ainuoyan/QDC507-Modem-Support) | 百旺 QDC507（`2c7c:0125`）识别口径与移远方言 AT、`AT+QCFG="nwscanmodeex"` 位标志、通用 `CGACT` 拨号配方；据此新增 baiwang 厂商模块 | GPL-3.0（仅事实依据，未拷贝） |
| [Siriling/5G-Modem-Support](https://github.com/Siriling/5G-Modem-Support) | 多驱动（RMNET / GobiNet / MHI / PCIe）拨号工具与 LuCI 应用的差异清单（审计中） | GPL-3.0（仅参考，未拷贝） |

**厂商拨号器 C 源码（quectel-CM / fibocom-dial / meig-cm / simcom-cm 等）不在致谢范围**：
内置 QMI / MBIM 拨号器是本仓库的独立 Rust 实现，未使用、未拷贝任何厂商 C 源码；
C 实现仅在开发中作为协议行为基准逐条对照（TLV 顺序、常量、流程），
据此修复的缺口与有意保留的差异记录见 [CHANGELOG.md](../CHANGELOG.md) 的 [1.0.3] 条目。
