# modem-pro v1.0.8 发版说明

发布日期：2026-10-24 ｜ 标签 `v1.0.8` ｜ 版本号 `1.0.8-1`（`VERSION`）

本版主线：**QMI/MBIM 模式下内置原生拨号后端显示「未运行」的修复**——对照上游
[LianXia233/modem-pro](https://github.com/LianXia233/modem-pro) 的主机侧拨号链路
（`hostdial*.go`）逐项核对，定位到四处移植缺口并全部按上游真实实现补齐，
另修一处随释放步骤暴露的断开竞态。逐项细节见 [CHANGELOG.md](CHANGELOG.md) 的
[1.0.8] 条目。

## 这版做了什么

「拨号上网」页的「内置原生拨号后端」卡片按 `/api/dial` 的 `host_dialer` 是否有
会话显示徽标；此前 QMI/MBIM 模组上它长期显示「未运行」——不是 UI 判断错，而是
会话真的建立不起来。四处根因与修法：

| # | 根因 | 修法（上游依据） |
| --- | --- | --- |
| ① QMI 主根因 | `raw_ip` 在运行中的网卡上写不进：主线内核 qmi_wwan 的 `raw_ip_store()` 对 `netif_running()` 的设备直接回 `-EBUSY`（"Cannot change a running device"），而 netifd 的 `modem_pro` 接口把数据网卡常年拉在 up——原实现直接写 sysfs，每一次都失败，会话起不来、5→60s 退避循环 | 移植上游 `prepareQMIWwanRawIP`：**down → 写 → up**，5 次 × 800ms 重试（覆盖模组刚重新枚举后的短暂 EBUSY 窗口）；已是 Y 不动，无该属性（GobiNet / MHI）视为不需要 |
| ② QMI/MBIM 共同根因 | 缺「拉起前释放模组侧数据上下文」：模组只有一个数据承载，固件自拨 / 换卡复位后的残留会话 / 用户激活过的上下文占住承载时，WDS Start 被拒（QMUXError 0xe）、MBIM CONNECT Status=2 FAILURE，会话永远建立不起来 | 移植上游 `releaseHostDialDataContext`：每次拉起会话前对**非 IMS** 的激活上下文发 `AT+CGACT=0`（每个 cid 重试 3 次、间隔 1.5s，与上游 hostDialReleaseAttempts/Gap 一致）；手动拨号的交办路径同样先释放并记一步「释放模组侧数据承载」；释放失败不拦拉起（上游同口径，由统一退避兜底） |
| ③ 放大器 | APN / PDP 管理缺「承载独占」闸门：QMI/MBIM 模组上 `set_apn` 会「先断后连」重新激活（`AT+CGACT=1`）并激活 IMS——用户改一次 APN 就把正在跑的原生会话顶掉；又因没有 ② 的释放步骤，之后再也恢复不了 | 移植上游 `modemUsesHostDialer` 口径：`set_apn` 对 QMI/MBIM **只写配置、不激活**（连 IMS 也不发），返回说明写明原因（上游 apiSetAPN 同款 autoNote）；`set_pdp` 对 QMI/MBIM **只允许停用**（上游 apiSetPDPActive 同款）。ECM / RNDIS / NCM 的行为不变 |
| ④ 恢复缺口 | 缺「会话在跑但一直没有地址」的掉线自愈：模组重新枚举后 raw_ip 被复位这类「协议层已连接、承载早已废了」的情况无法自愈 | 移植上游 `hostDialRestartDue` 口径：会话探活之外每轮看链路事实（数据网卡有 IPv4 地址 + 载波），跑满 **60s** 没地址就重开会话（间隔 **120s**、最多 **10 次**、拿到地址即清零；只停不原地起，下一轮拉起） |

另修一处竞态：释放承载要占串口几秒，期间用户点「断开」的话（hangup 置
`manual_stop` 并 bump 代次），取代次发生在 bump 之后、代次比对挡不住——
现在拉起前复查一次 `manual_stop`，不让会话把用户的断开意图顶掉。

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

## 升级注意

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.8-1`）。
- **QMI / MBIM 模组**：升级后插上模组 ≤15 秒内「内置原生拨号后端」应变为
  `native-qmi` / `native-mbim`。若此前因改过 APN 卡住，现在会自动释放模组侧
  承载并重拨，不需要重启模组。
- **在 APN 管理里改 APN / 在 PDP 管理里点激活**：QMI/MBIM 模组上现在只写配置、
  不激活（说明文案里会写原因）；PDP 卡片的「激活 / 重连」对这类模组会明确拒绝，
  「停用」照常可用。要建立数据呼叫请用「一键拨号」或等自动重拨——这是有意的
  承载独占保护，不是功能退化。
- 无 API 路径与字段变更，无新增必填配置；`/etc/modemd/config.toml` 是 conffiles，
  升级不覆盖你的取值。
- 排障入口：`logread -e modem-pro` 里看「原生拨号」行——拉起前会记
  「拉起前先让模组侧让出承载：…」；raw_ip 切换记「把 wwan0 的 qmi/raw_ip 置为 Y
  （第 N 次尝试）」；自愈记「掉线自愈 —— 会话在跑但数据网卡一直没有地址（第 N 次重开）」。

## 已知边界（如实说明）

- **原生拨号链路未经实机验证**：本版根因全部有上游真机记录与内核源码依据
  （EBUSY 行为核对 torvalds/linux 当前 `qmi_wwan.c`），代码路径有单测覆盖，
  但 QMI/MBIM 会话建立、释放承载、raw_ip 切换、掉线自愈均未在真机上跑过。
  升级后请按上面「升级注意」的第一条确认。
- 质量：`cargo test --locked` **805 项通过**（相对 1.0.7 的 801 项新增 6 项；
  另有 2 个与本版无关的存量 `http::ws` 环境用例在本机沙箱失败，git stash 基线
  相同、单跑即过）。
- `cargo fmt --check`：本版触及文件已对齐，但**存量漂移仍在**
  （`sms.rs` / `netprov` 等历史文件，非本版改动，未越界重排）。
- `cargo clippy` 仍报 **1 处存量 error**：`sms.rs:1476` 的 `cur >= 255`
  （行为正确，本版未触碰该文件）；改动文件 0 新告警。
- 发布说明基于本地 x86_64 / WSL 验证；musl 交叉编译与产物由 CI（OpenWrt SDK）
  完成，以 Actions 运行结果为准。

## 文档

`CHANGELOG.md` 的 [1.0.8] 条目（本版全部细节）。
