# modem-pro v1.0.2 发版说明

发布日期：2026-10-20 ｜ 标签 `v1.0.2` ｜ 版本号 `1.0.2-1`（`VERSION`）

本版把 QModem 与 Go 版 modem-pro 的**设备发现、端口选择、厂商数据通道**能力并入现有 Rust 架构，
并补上多模组拓扑、同型号区分、失败退避与断开清理。逐项依据与真机待办见
[`docs/UPSTREAM_AUDIT.md`](docs/UPSTREAM_AUDIT.md)，逐型号状态见
[`docs/SUPPORT_MATRIX.md`](docs/SUPPORT_MATRIX.md)。

## 这版做了什么

### 1. 设备发现按「物理设备 + 端口规则」选口

- 按 USB 设备分组：一台模组只登记**一个**命令口（RM520N 这类 4 口设备不再变成 2~3 台模组）；
- 端口规则的 `include` 白名单生效，语音 PCM 口剔除；白名单外宁可等 tty 出现，也不拿 diag 口凑数；
- 身份级探活：`AT` 回 `OK` 不算命令口，必须 `AT+CGMM`/`AT+CGMI`/`ATI` 有一条内容行；
- `option_driver=1` 的设备在枚举串口**之前**补绑 `option1/new_id`（5 分钟冷却）；
- PCIe/MHI 的 AT 口（`mhi_*DUN*`、`mhi_*AT*`、`wwan*at*`）纳入发现；`AtPort` 增加裸字符设备后端；
- 非 AT 口否定缓存 15s→300s，设备刚插入（<90s）固定 15s 重探。

### 2. ECM / RNDIS / NCM 按厂商下发数据通道指令

以前一律 `AT+CGACT`，PDP 显示 active 而网卡没有 carrier。现在按「厂商 × 平台」发：

| 厂商 | 激活 |
| --- | --- |
| 移远 | `AT+QNETDEVCTL`（海思 / 展锐 / 高通三套参数） |
| 广和通 | `AT+GTRNDIS`（MTK 平台仍 `AT+CGACT`，FM350 不走 GTRNDIS） |
| 华为 / 鼎桥 | `AT^NDISDUP`（华为展锐为 `AT^SETAUTODIAL`） |
| 美格 / 有方 / Telit / 高新兴 / 合宙 | `AT$QCRMCALL` / `AT$MYUSBNETACT` / `AT#ICMAUTOCONN` / `AT+ZECMCALL` / `AT+RNDISCALL` |

挂断与激活严格对称；私有指令被拒时回退 `AT+CGACT` 并记录；平台未识别只走 3GPP。

### 3. 拓扑、恢复与清理

- `tty_list` 显示同一设备的全部 AT 候选口，手动切口后新口排最前；
- 命令口绑定新增 `vid:pid@1-1`（API `bus_path`），同型号多台不再共用一条绑定，旧键兼容；
- PCIe 模组自动补绑 `mhi_pci_generic` / `mtk_t7xx`（`[serial] pcie_autobind`，默认开）；
- 卡死救援：150s 无模组 → USB `authorized 0→1` / PCIe remove+rescan（`[serial] auto_rescue`，**默认关**）；
- 原生 QMI/MBIM 失败退避 5→60s（成功清零）；
- 断开后清该网卡旧默认路由与地址，避免「已断开但流量继续进黑洞」；
- 拨号步骤新增「形态与支持清单核对」（清单形态不符时提示，不拦截）。

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

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.2-1`）。
- 配置**只新增**两个键，缺省即旧行为：`[serial] pcie_autobind = true`、`[serial] auto_rescue = false`；
  `/etc/modemd/config.toml` 是 conffiles，升级不会覆盖你的取值。
- API 只新增可选参数 `bus_path`（`/api/atport/bind`）与若干步骤文案；路径、字段、`code` 语义不变。
- 升级后模组会重新识别一次（只读 `ATI`/`CGMM`/`CGMR`，不打断已有链路）。
- 若之前手工绑过 `vid:pid`，继续有效；同型号多台建议改绑 `vid:pid@总线路径`。

## 已知边界（如实说明）

- **本版没有做真机验证**：无 5G 模组、SIM 与目标机，厂商数据通道、MHI 裸口、断开清理
  在 busybox 上的行为、`auto_rescue` 与 `pcie_autobind` 的实际效果均待实机确认；
- `auto_rescue` 默认关闭（会短暂断开数据面），需要时再打开；
- 仓库基线仍有 7 条 clippy warning，均在未改动模块，未新增 `#[allow]`；
- 自动化：`cargo test --locked` 732 passed，`cargo fmt --check` 通过，release 构建通过。

## 文档

`docs/INTEGRATION.md`（架构/数据流）、`docs/BUILD.md`（构建/CI/验证）、
`docs/API.md`（103 条 `/api/*` + 2 个 WebSocket 全表）、`docs/UPSTREAM_AUDIT.md`、
`docs/SUPPORT_MATRIX.md`；历史文档的处置见 `docs/README.md`。
