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
| 测试 | `tests/*.sh`、C 单测 | 91 个 `_test.go` / 1084 个 `Test*` | 721 个 Rust 单测（本次前 703） |

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
| C1 | README 链接的 `docs/INTEGRATION.md`、`docs/API.md` 等 6 个文件不存在 | `git log` 显示 `66853b8 Delete docs directory` 一次性删掉了 10 个文档（5777 行），README 未同步 | 新增 `docs/README.md` 说明现状；新增本文件与 `docs/SUPPORT_MATRIX.md`；不改写历史文档（内容已过时，恢复会误导） |
| C2 | 代码注释多处引用 `docs/INTEGRATION.md` | 同上 | 保留注释（指向设计意图），在 `docs/README.md` 里说明该文件已删除 |

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
| `cargo test --locked` | **732 passed / 0 failed**（基线 703，新增 29） |
| `cargo build --release --locked` | 通过（见构建报告） |
| `cargo clippy --all-targets --locked` | 0 error；7 条 warning **全部在未改动文件**（`cells.rs` 死代码/文档格式、`params.rs` 无用赋值、`route.rs` 比较写法、`siminfo.rs` map 恒等、`dial.rs:1962` contains） |
| `cargo fmt --check` | 仓库基线本就有 14 个文件 / 48 处不合规；本次改动文件已 `rustfmt` 过，另有一个独立的 `style: cargo fmt` 提交把全仓格式统一 |
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
