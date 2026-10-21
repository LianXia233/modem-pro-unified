# modem-pro v1.0.5 发版说明

发布日期：2026-10-21 ｜ 标签 `v1.0.5` ｜ 版本号 `1.0.5-1`（`VERSION`）

本版新增**运营商感知的自动拨号**：拨号前自动读取模组返回的运营商信息，识别
中国移动 / 中国联通 / 中国电信 / 中国广电后自动匹配并下发预制 APN，**回读确认
生效后才拨号**——专治「部分模组必须显式下发 APN 才能拨号，而页面上明明显示着
运营商甚至在用 APN」的场景。逐项依据见
[`docs/DIAL_STATE_MACHINE.md`](docs/DIAL_STATE_MACHINE.md) §11 与
[CHANGELOG.md](CHANGELOG.md) 的 [1.0.5] 条目。

## 这版做了什么

### 1. 运营商识别（四级证据，纯表驱动）

| 证据级 | 来源 | 说明 |
| --- | --- | --- |
| 名称 | `AT+COPS?` 显示名 | 中英文变体：CMCC / China Mobile / 中国移动 / CHN-CT / CHN-UNICOM / 中国广电 … |
| PLMN | `AT+QNWINFO` / COPS `format=2` | MCC 460 + 在网 MNC：移动 00/02/04/07/08、联通 01/06/09、电信 03/05/11、广电 15 |
| ICCID | SIM 身份缓存 | 发卡行 IIN 前 6 位，只作兜底（物联网卡常由代理发卡，中等置信） |

名称与 PLMN 冲突时以 PLMN 裁决；四级全失败 ⇒ 保留现状，**绝不**动现有 APN；
`COPS mode=2/3`（注销 / 注册被拒）直接判识别失败。

### 2. APN 决策（用户配置永远最高）

| 场景 | 动作 |
| --- | --- |
| 用户/表单显式指定的 APN | 沿用，不识别不覆盖 |
| 模组上下文 = 同家预制 APN | 沿用（跳过写入，不重复下 AT） |
| 模组上下文 = 别家预制 APN（插过别家卡 / 运营商已切换） | 校正为本家（需名称/PLMN 级证据） |
| **自定义 APN**（企业卡 / 专用套餐） | 沿用，**绝不覆盖** |
| 模组无上下文但 `CGCONTRDP` 报了在用 APN（页面显示的那个） | 直接用它拨号 |
| 模组没有可用 APN + 识别成功 | **下发预制 APN**：移动 `cmnet` / 联通 `3gnet` / 电信 `ctnet` / 广电 `cbnet` |
| 识别失败 | 保持旧行为（沿用模组配置） |

### 3. 按后端分流落地（不假设所有模组用同一种 APN 设置方法）

- **AT 路径**（ECM/RNDIS/NCM）：拨号线程内发 3 条识别指令（`COPS` / `QNWINFO` /
  `CGCONTRDP`）→ 按需 `AT+CGDCONT`（幂等闸门 + **回读确认**，回读成功才记
  「已生效」）→ **之后**才拨号；识别 / 写入任何一步失败都不阻断拨号，回落旧行为。
- **QMI/MBIM**：APN 作为拨号器参数直接进 `WDS Start` / MBIM `CONNECT`，
  会话建立即生效；协调器线程**不发 AT**，只用采样循环顺手缓存的运营商证据
  （10 分钟有效）。
- **换卡 / 切换**：SIM 更换作废「自动下发」记录与证据缓存，下一次自动拨号按新卡
  重新识别校验；失败重拨走统一退避表（AT 30→300s、原生 5→60s），不无限循环、
  不重复写、不碰用户配置；全程记进页面拨号步骤（「运营商识别」「APN 自动配置」）。
- 手动拨号表单 `cid/apn/pdp_type` 全空时同样走这条链（任一项显式填写则完全不介入）。

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

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.5-1`）。
- **无新增配置键、无 API 字段变更**：接口路径、字段名、`code` 语义与 1.0.4 完全一致；
  `/etc/modemd/config.toml` 是 conffiles，升级不覆盖你的取值。
- 自动拨号行为变化：模组里**没有**可用 APN 上下文时，现在会按识别到的运营商
  下发预制 APN（旧版只会「沿用模组配置」然后失败退避）；模组里**已有** APN 的
  完全不受影响，用户手动配置的 APN 永远优先。
- 物联网卡 / 企业卡 / 专用套餐若靠自定义 APN 上网：只要它存在模组上下文里就不会
  被覆盖；若模组里没有、靠本版自动下发默认 APN，请改在页面上显式填一次
  （显式配置永远优先）。
- 拨号页面「流程步骤记录」会新增「运营商识别」「APN 自动配置」两步，排障先看这里。

## 已知边界（如实说明）

- **本版功能未经真机验证**：无 5G 模组、SIM 与目标机。逻辑与回归由单测保证
  （`cargo test --locked` **777 项通过**，本版新增 13 项）；
- 未实机验证：各固件 `COPS` / `QNWINFO` 应答变体的识别表现、`AT+CGDCONT` 写入后
  部分固件的生效延迟、广电实卡（`46015` / `898615` 表项按公开资料整理，未实卡验证）；
- 广电 IIN 公开资料少，宁可漏认不可错认：识别不出就保留现状，不会写错 APN；
- `cargo fmt --check` 干净、clippy 0 warning、release 构建通过、
  `scripts/check-desensitize.sh` exit 0。

## 文档

`docs/DIAL_STATE_MACHINE.md`（拨号生命周期，本版新增 §11 运营商感知 APN）、
`docs/INTEGRATION.md`（架构/数据流）、`docs/BUILD.md`（构建/CI/验证）、
`docs/API.md`（`/api/*` + WebSocket 全表）、`docs/UPSTREAM_AUDIT.md`（审计与上游致谢）、
`docs/SUPPORT_MATRIX.md`；历史文档的处置见 `docs/README.md`。
