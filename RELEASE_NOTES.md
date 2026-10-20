# modem-pro v1.0.1 发版说明

发布日期：2026-10-20 ｜ 对应仓库 `LianXia233/modem-pro-unified` 标签 `v1.0.1`

## 这版做了什么（相对 v1.0.0 / 1.0.0-7）

### 0. MT5700 系列专项（1.0.0-11）

以官方手册 V200R001C20 + luci-app-mt5700 实机验证解析为依据，MT5700/MT5710
从「只有识别」补到全功能：`^MONSC`/`^MONNC` 小区与信号解析（NR/LTE 双布局 +
旧编码格式 + PCI 十六进制回退）、`AT^CELLSCAN` 主动扫频（支持无卡扫描）、
锁网方言 `^LTEFREQLOCK`/`^NRFREQLOCK`（写后自动切飞行模式）、`^CHIPTEMP`
12 路温度、11 条手册指令进预设（详见 CHANGELOG 1.0.0-11）。

### 1. 多维度模组识别（可解释、可校验、不猜）

- 识别不再是「ATI 关键字 → 短名」一条线索，而是**多维取证按可信度取结论**：
  USB 端口规则（90）＞ AT 型号命中清单（85）＞ 仅端口规则（80）＞ 厂商关键字（55）＞ 独占 VID（45）；
- 结论带**证据、候选、冲突、置信度**，LuCI 与 WebUI 都能显示「为什么认成这个型号」；
- 冲突不猜：设备自报优先、厂商按清单归属纠正，冲突原文直接上页面；
- 型号匹配支持主干（`MT5700M` 也能命中清单里的 `mt5700m-cn`）；
- 新增数据资产：QModem USB 端口规则 18 条、VID 映射 86 条、逐型号怪癖 19 条
  （电压/ICCID/温度的厂商指令差异，每条保留上游实测依据）。

### 2. 拨号链路阶段（phase）：「已连接」不再是一句空话

- `state` 之外新增 `phase`：`iface_pending`（主机网卡未就绪）/ `route_pending`
  （有地址但默认路由不经它）/ `online`（地址 + 默认路由三层全过）；
- 路由事实读 `/proc/net/{route,ipv6_route}`，**不 fork**；
- v4 与 v6 路由**按地址族分别判**（双栈下 v4 出不去不再被 v6 路由掩盖）；
- 拨号步骤里直接给排障方向（route_pending → 查 netifd/metric；iface_pending → 查 USB/驱动/hostnic）；
- `AT+CBC` 两字段固件（FM350/FM160）供电电压解析修复（此前恒显示 `-`）；
- `AT+CCLK?` 时区按 3GPP 修正（默认减 `±zz`），违规固件（值是 UTC、后缀报 +32）
  由逐型号怪癖表跳过。

### 3. 实测修掉的其它问题

- 时区下拉永远选中最后一项（`selected:false` 被序列化成真）+ 时区下拉未进 DOM（16 处下拉同类）；
- 锁频段子页空白（DOM 未挂载时全局查找 + `E()` 不认裸函数）；
- cells 页 8 列表格溢出（滚动容器 + 表头 sticky）；
- 速率卡片「左表盘 + 右曲线」布局、上下行各自一条曲线、窄屏不换布局；
- netifd 拉起链路的「在线时长」显示（观测值）与「已连接」判定（按实际地址而非 operstate）。

## 产物清单

| 文件 | 适用 |
| --- | --- |
| `modem-pro-*.apk`（`apk-mainline`） | OpenWrt 主线 / snapshots，包架构 **aarch64_generic**（A53/A55/A72 等通用） |
| `modem-pro-*.apk`（`apk-filogic`） | OpenWrt 主线 / snapshots，包架构 **aarch64_cortex-a53**（MT7981/7986 等 filogic 设备；`apk --print-arch` 为 `aarch64_cortex-a53` 时选它） |
| `modem-pro_*.ipk` | OpenWrt 24.10.x（opkg 包管理器） |

> 两个 APK 内容相同（同一份静态 musl 二进制、无内核模块依赖），只是包架构标签不同；
> apk 会拒绝架构不匹配的包，按设备 `apk --print-arch` 的输出选对应文件即可。
> 内含 modemd 守护进程、四家厂商拨号器、netifd 协议脚本与 LuCI 前端（菜单已汉化）。

## 安装

```sh
# OpenWrt 25.x（apk）
apk add --allow-untrusted /tmp/modem-pro-*.apk

# OpenWrt 24.10（opkg）
opkg install /tmp/modem-pro_*.ipk
```

完整安装验证清单见仓库 [docs/BUILD.md](../../blob/main/docs/BUILD.md)。

## 升级注意

- 直接 `opkg/apk` 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.1-1`）；
- 配置兼容：UCI 配置项、`/api/*` 路由与 JSON 键均向后兼容，`dial.state` 枚举不变，
  新增字段（`phase`/`phase_text`/`identify`）为可选展示；
- 升级后模组会重新识别一次（只读 `ATI`/`CGMM`/`CGMR`，不会打断已有链路）；
- 若概览页「供电电压」从 `-` 变成有值，是 1.0.1 修复了两字段 `AT+CBC` 固件的解析。

## 已知边界（如实验证）

- `route_pending`/`iface_pending` 两条路径由单测覆盖 + 实机复现过 `route_pending`；
  `iface_pending` 仅单测（需拔网卡场景）；
- 华为/SIMCom/高新兴 的温度电压私有指令：无设备，未实机验证；
- FM160 的 CCLK 时区修正依赖上游实测证据，本机（FM350）行为零变化；
- 自动组网为 15 秒轮询（幂等、零写入），事件驱动为后续优化项。
