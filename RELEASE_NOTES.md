# modem-pro v1.0.3 发版说明

发布日期：2026-10-20 ｜ 标签 `v1.0.3` ｜ 版本号 `1.0.3-3`（`VERSION`）

本版修复**内置拨号器在支持的 QMI/MBIM 模组上不自动启动**的问题（插上模组、页面识别出类型，
但拨号器始终不起、`wwan0`/`wwan0_1` 无 carrier，手动点拨号才动），并对照四家厂商 C 拨号器源码
补齐两处协议缺口；同时清理小窗页对已删除接口的残留调用。逐项依据见
[`docs/UPSTREAM_AUDIT.md`](docs/UPSTREAM_AUDIT.md) 与 [CHANGELOG.md](CHANGELOG.md) 的 [1.0.3] 条目。

## 这版做了什么

### 1. 内置拨号器自动启动（本版核心）

**根因**：拨号器托管判定读 `dial_state.kind`，而该字段只在拨过号、或页面拉过 `/api/dial`
之后才有值——把拨号的**产物**当成了拨号的**前置条件**。全新模组插入后永远判「不接管」，
拨号器不启动，`link_state` 保持 0。

**修复**：

- 新增 `Modem::ensure_dial_kind()`：`kind` 为空时走 `usb_type`（自带 30 秒缓存，命中零 AT），
  按 `dial_status` 同口径回写 `kind`/`type_name`，**不伪造拨号状态**；探不到按「不接管」处理；
- 修掉「算不出计划」路径的退避失效：原实现先清退避计数再重试，指数退避（5→60s）永远停在
  第一档；现在跨轮保留、错误变化才清零；
- 组网循环改为「先跑一轮再进 15 秒节拍」：启动即建接口、拨号器托管即见模组。

### 2. 对照厂商 C 拨号器补齐的两处缺口

与 quectel-CM 1.6.8 / fibocom-dial / meig-cm / simcom-cm 源码逐条对照后修复：

- **QMAP 模式漏判**：`qmap_mode` 探测补齐模块参数两条兜底路径。OpenWrt 的
  `qmi_wwan_q`/`qmi_wwan_f` 只把参数挂模块上，漏读即漏判 QMAP → 不下发
  `WDA Set Data Format` + `Bind Mux` → 驱动不 `add_mux` → `wwan0_1` 永远 NO-CARRIER；
- **控制口反查缺 `usb` 子目录**：新版 qmi_wwan 把 `cdc-wdm` 节点挂到接口的 `usb` 子目录，
  只认 `usbmisc` 会找不到控制口，拨号器以 `NoWdm` 退避（现象与「模组不支持」无异）。

协议层（QMI 全流程、TLV 顺序与常量、QMAP mux 规则、MBIM Basic Connect）经逐条核对与 C 实现一致；
有意保留的差异（Profile Index、MBIM Session ID、`mknod` 兜底）已在 CHANGELOG 说明。

### 3. 小窗页残留调用清理

- `mini.html` 开机仍在 `fetch('/api/sms/auth')`（接口已随短信密码取消而删除，稳定 404），
  导致每条新短信弹两次卡（先「正文受保护」再正文卡）。已删除探测并清理同源死代码；
- 顺带完成与 modem-pro 的**接口名全量对照**：88 个共有接口名完全一致、无任何改名；
  本版新增的 11 个接口均已在 `docs/API.md` 登记。

### 4. 文档：补齐上游致谢

README 与 `docs/UPSTREAM_AUDIT.md` §7 列出全部参考过的仓库（ouyangzq/modem-pro、
ouyangzq/luci-proto-modem-pro、FUjr/QModem、LianXia233/luci-app-fm350）。
内置 QMI/MBIM 拨号器是**独立 Rust 实现**，未使用、未拷贝任何厂商 C 源码。

## 产物清单

| 文件 | 适用 |
| --- | --- |
| `modem-pro-*.apk`（`apk-mainline`） | OpenWrt 主线 / snapshots，包架构 **aarch64_generic** |
| `modem-pro-*.apk`（`apk-filogic`） | OpenWrt 主线 / snapshots，包架构 **aarch64_cortex-a53**（MT7981/7986 等） |
| `modem-pro_*.ipk` | OpenWrt 24.10.x（opkg） |

两个 APK 内容相同（同一份静态 musl 二进制、无内核模块依赖），只是包架构标签不同；
按设备 `apk --print-arch` 的输出选对应文件。包内含 `modemd`、netifd 协议脚本与 LuCI 前端。

## 安装

```sh
# OpenWrt 25.x（apk）
apk add --allow-untrusted /tmp/modem-pro-*.apk

# OpenWrt 24.10（opkg）
opkg install /tmp/modem-pro_*.ipk
```

构建与安装后验证见 [`docs/BUILD.md`](docs/BUILD.md)。

## 升级注意

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.3-3`）。
- **无新增配置键、无 API 变更**：接口路径、字段、`code` 语义与 1.0.2 完全一致；
  `/etc/modemd/config.toml` 是 conffiles，升级不覆盖你的取值。
- 本版修复只在「插上模组后不发任何拨号指令」的路径上生效，已有链路不受影响；
  升级后模组重新识别一次（只读 `ATI`/`CGMM`/`CGMR`）。
- QMI/MBIM 模组升级后建议观察日志：应出现
  「原生拨号：模组 N 拉起 native <厂商> qmi [cdc-wdm /dev/cdc-wdmX ...]」，
  且 `wwan0_1` 出现 carrier。

## 已知边界（如实说明）

- **拨号器自动启动未经真机验证**：无 5G 模组、SIM 与目标机；`ensure_dial_kind` 探测、
  退避行为、QMAP mux 与控制口反查均按源码与既有测试保证，实机效果待确认；
- `WDS Start` 的 Profile Index 默认 0（不指定，由固件选默认 profile），多 profile 模组
  需显式配置 `[net] host_dial_pdn`；
- 自动化：`cargo test --locked` 692 项通过（新增 3 项），`cargo fmt --check` 干净，
  clippy 无新增警告；小窗页内联脚本经 `node --check` 校验。

## 文档

`docs/INTEGRATION.md`（架构/数据流）、`docs/BUILD.md`（构建/CI/验证）、
`docs/API.md`（`/api/*` + WebSocket 全表）、`docs/UPSTREAM_AUDIT.md`（审计与上游致谢）、
`docs/SUPPORT_MATRIX.md`；历史文档的处置见 `docs/README.md`。
