# modem-pro v1.0.4 发版说明

发布日期：2026-10-21 ｜ 标签 `v1.0.4` ｜ 版本号 `1.0.4-1`（`VERSION`）

本版做两件事：**①** 从头把「模组插入 → 自动拨号 → 接口拉起 → 地址/DNS 落地」这条链再走一遍，
修掉 5 个会让「识别到支持的模块却拨不上 / 接口配歪」的缺陷；**②** 把组网强制下发的 DNS
默认值换成「IPv4 首选+备选 / IPv6 首选+备选」四条。逐项依据见
[`docs/DIAL_STATE_MACHINE.md`](docs/DIAL_STATE_MACHINE.md) §10 与 [CHANGELOG.md](CHANGELOG.md) 的 [1.0.4] 条目。

## 这版做了什么

### 1. 拨号状态机与接口拉起：第二轮自检（5 项）

上一版统一的拨号协调器本身没有再发现设计缺陷，问题都在它的**接入点**和**接口侧**：

| # | 现象 | 修复 |
| --- | --- | --- |
| 1 | QMI/MBIM 在页面上点「拨号」要干等 ≤15 秒才变 `connected`（只有异步任务调了 `kick()`） | 协调器新增 `arm(Arc<AppState>)`，`kick()` 不再需入参，同步接口 `/api/dial/connect` 成功后立即踢一轮 |
| 2 | 接口 `device` 漂到 `eth2` 或**另一台模组的** `wwan1` ⇒ 断网 | 排除名单不再把本模组自己的 `modem<id>` 算「被占用」（新增 `occupied_by_others`） |
| 3 | 网卡还没枚举出来就建了没有 `device` 的接口（netifd 判 `NO_DEVICE` 并 `proto_block_restart`） | 新增 `skip_without_device`：无 device 且接口不存在 ⇒ 本轮不动 UCI |
| 4 | 从未拨号的 QMI 模组点「断开」失败，过会儿自己又连上 | `hangup` 改用 `ensure_dial_kind()`（原来用缓存 kind，空串时会误发 `AT+CGACT=0`） |
| 5 | 断开后页面短暂 `idle`、下一拍又跳回「已连接」 | `dial_status` 的改判与补地址都加 `!manual_stop` 门禁 |

顺带把「拨号器与组网必须拨/绑同一张网卡」从注释落成机制（组网 `remember_device`、
拨号器 `device_for`、回收 `forget_device`）：两边入参不同，QMAP 下算错就是父口永远
NO-CARRIER、地址出不来。

### 2. 接口默认下发 DNS 改四条

| 族 | 首选 | 备选 |
| --- | --- | --- |
| IPv4 | `119.29.29.29`（DNSPod） | `223.5.5.5`（阿里） |
| IPv6 | `2402:4e00::`（DNSPod） | `2400:3200::1`（阿里） |

- 蜂窝链路（尤其 FM350 系列 RNDIS）的 DHCP 常年不给 DNS，不强制下发就是
  `resolv.conf.auto` 为空、域名解析全 REFUSED；
- `wan_ipv6 = false` 时**自动不下发** v6 那两条（新增 `netprov::effective_wan_dns`）；
- 想用运营商 / DHCP 给的 DNS：把 `[net] wan_dns` 写成 `off`（`none` / `carrier` 同义）；
- `GET /api/dial` 新增 `wan_dns` 字段，页面「自动组网报告」卡片加一行显示。

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

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.4-1`）。
- **`[net] wan_dns` 默认值变了**：`223.5.5.5 119.29.29.29` → `119.29.29.29 223.5.5.5
  2402:4e00:: 2400:3200::1`。`/etc/modemd/config.toml` 是 conffiles（升级不覆盖），
  所以**配置文件里没写这一行的机器也会按新默认值下发**（默认值在代码里）；
  自己写过这一行的沿用你的取值，想要新默认就删掉该行或改成 `off` 之外的空值。
- **新增 API 字段**：`GET /api/dial` 多一个 `wan_dns` 字符串（空串 = 不强制下发）。
  其余接口路径、字段名、`code` 语义与 1.0.3 完全一致。
- 升级后组网循环（≤15 秒一轮）会把新 DNS 写进 `network.modem<id>.dns` 并 reload 一次；
  已建接口不会重建，只是多/改一个 `dns` 项。
- 关掉 IPv6 的机器不会拿到 v6 那两条（避免写进 `resolv.conf.auto` 后永不回应）。

## 已知边界（如实说明）

- **本版 5 项修复均未经过真机验证**：无 5G 模组、SIM 与目标机。逻辑与回归由单测保证，
  待实机确认的是：多网卡机器上接口 `device` 的稳定性、点「拨号」≤3 秒转 `connected`、
  从未拨号的 QMI 模组的断开行为；
- AT 路径仍不做主动「掉线探活」（依赖模组会话 + netifd），已覆盖显式断链
  （重启 / 飞行模式 / 配额）；
- 自动化：`cargo test --locked` **764 项通过**（本版新增 5 项）、`cargo fmt --check` 干净、
  clippy 0 warning、release 构建通过、`scripts/check-desensitize.sh` exit 0。

## 文档

`docs/DIAL_STATE_MACHINE.md`（拨号生命周期，本版新增 §10 第二轮自检）、
`docs/INTEGRATION.md`（架构/数据流）、`docs/BUILD.md`（构建/CI/验证）、
`docs/API.md`（`/api/*` + WebSocket 全表）、`docs/UPSTREAM_AUDIT.md`（审计与上游致谢）、
`docs/SUPPORT_MATRIX.md`；历史文档的处置见 `docs/README.md`。
