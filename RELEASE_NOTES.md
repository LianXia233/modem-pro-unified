# modem-pro v1.0.7 发版说明

发布日期：2026-10-24 ｜ 标签 `v1.0.7` ｜ 版本号 `1.0.7-1`（`VERSION`）

本版两条主线：**350 系列「模组有 V6、接口没 V6」的三层修复**（实机 FM350-GL /
中国移动），以及 **内核 6.x 下一台模组被登记成多台的 discovery 修复**（实机
Hiveton H5000M / ImmortalWrt SNAPSHOT / kernel 6.18.55 / Quectel RG520F-EB）。
逐项细节见 [CHANGELOG.md](CHANGELOG.md) 的 [1.0.7] 条目。

## 这版做了什么

### 1. 350 系列「模组有 V6、接口没 V6」（实机 FM350-GL，RNDIS）

三个叠在一起的问题，本版逐层修齐：

| # | 问题 | 修法 |
| --- | --- | --- |
| ① 根因 | `AT+CGCONTRDP` 解析从第 3 位起「扫第一个像地址的字段」，而 FM350-GL 的 `<local_addr>` / `<gw_addr>` 列常年空串 —— 把运营商的 IPv6 DNS 显示成「模组本机 IPv6」；若用户把 v6 模式改 `static`，还会被写成 `/128` 并配出一条永远不通的默认路由 | 只读 `f[3]`（`f[4]` 仅在「另一个地址家族」时收，否则那是网关）；新增 `split_local_addr()` 处理 `地址+掩码` 连写（IPv4 8 段 / IPv6 32 段），DNS 列一律不参与 |
| ② | 350 系列地址源可能回落到 CGCONTRDP 残留值 | 对齐参考实现 `luci-app-fm350`：地址只在 `AT+CGPADDR` 里，`::1` 占位即「无 IPv6」；CGCONTRDP 只作兜底。仅对 `is_fibocom_350()` 生效，其它厂商路径不变 |
| ③ | v6 兄弟接口每 15 秒被拆一次（`should_kick` 见「没 up 就重拉」，而 odhcp6c 的 soltimeout 是 120 秒），客户端永远跑不完一次交涉 | 接口没 up 也走退避，只有第一次立刻拉起；节奏 **15 秒 ×10 → 61 秒 ×10 → 180 秒（无限兜底）**，纯 IPv4 环境也保持巡检以便运营商补开 v6 时自愈 |

实机验证（FM350-GL，cmiot5g）：网络卡片不再出现运营商 DNS 假地址；v6 重拉节奏
15s ×10 → 61s ×10 → 180s；IPv4 仍为 static，上网正常。该卡实测没有 IPv6
（`AT+CGPADDR` v6 位恒为 `::1`），此前「模组有 V6」的显示本身就是 ① 的误报。

### 2. 内核 6.x：一台模组被登记成多台（discovery）

`discovery::ports::describe()` 原本假设 `/sys/class/tty/<tty>/device` 直接指向 USB
接口目录（`2-1:1.2`），但内核 6.x 的 usb-serial 指向接口下的**端口目录**
（`2-1:1.2/ttyUSB2`），名字里没有冒号 —— 解析失败，`usb_dev` / `iface` / `vid` /
`pid` 全为空。连锁后果：

- `plan()` 退化成「每个串口一台设备」：一台两个 AT 口的模组被登记成两台，
  自动组网给同一张网卡建出重复接口；
- `vid/pid` 为空 → `modem_ports.json` 里既有的 `2c7c:0800` 规则永远匹配不上；
- 两个拨号器抢同一个 `/dev/cdc-wdm0`，QMI CTL Sync 超时（service 0x00
  message 0x0027），始终拨不上号。

修法：新增 `split_usb_interface()`，从 canonical 路径逐级上溯找第一个
`<bus>:<配置>.<接口>` 形状的目录；两种 sysfs 布局（接口目录 / 端口目录）都兼容，
非 USB 串口仍正确拒绝。修复后按真实路径推演：`1.0/1.2/1.3` → 同组 `usb:2-1` →
规则命中 → `primary=1.2`、`fallback=1.3` → 只登记 1 台。

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

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.7-1`）。
- **内核 6.x + usb-serial 模组（如 H5000M / RG520F-EB）**：升级后同一模组只登记
  1 台、重复接口不再创建。若升级前已被登记成多台，建议重启插件（或整机）让发现
  周期按正确分组重建接口。
- **350 系列**：页面不再显示运营商 DNS 假地址；`modem<id>_6` 重拉节奏见上表 ③。
- 无 API 路径与字段变更，无新增必填配置；`/etc/modemd/config.toml` 是 conffiles，
  升级不覆盖你的取值。
- 排障入口：discovery 看 `logread -e modem-pro` 里「发现模组 … 规则=」行
  （修复后 `group=` 应为 `usb:<bus>` 而非 `port:/dev/ttyUSBx`）；350 的 IPv6 看
  「v6 接口 modem<id>_6 已运行 Ns 仍无全局 IPv6（第 N 次重拉）」。

## 已知边界（如实说明）

- **discovery 修复未经完整拨号回归**：真机上只确认了 sysfs 路径形态、归组与规则
  匹配逻辑（等价推演 + 单测），QMI 拨号全流程未在修复后重跑；350 系列修复有实机验证。
- 质量：`cargo test --locked` **801 项通过**（相对 1.0.6 的 794 项新增 7 项）。
- `cargo fmt --check`：本版触及文件已对齐，但**存量漂移仍在**
  （`sms.rs` / `netprov` 等历史文件，非本版改动，未越界重排）。
- `cargo clippy` 报 **1 处存量 error**：`sms.rs:1476` 的 `cur >= 255` 对该类型
  恒等价于 `cur == 255`（min/max comparison lint），行为正确但建议下版顺手改为
  `==`；该文件本版未动，未越界修改。
- 发布说明基于本地 x86_64 验证；musl 交叉编译与产物由 CI（OpenWrt SDK）完成，
  以 Actions 运行结果为准。

## 文档

`CHANGELOG.md` 的 [1.0.7] 条目（本版全部细节）、`docs/INTEGRATION.md`
（架构/数据流）、`docs/BUILD.md`（构建/CI/验证）、`docs/API.md`
（`/api/*` + WebSocket 全表）、`docs/UPSTREAM_AUDIT.md`（审计与上游致谢）、
`docs/SUPPORT_MATRIX.md`；历史文档的处置见 `docs/README.md`。
