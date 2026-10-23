# modem-pro v1.0.6 发版说明

发布日期：2026-10-23 ｜ 标签 `v1.0.6` ｜ 版本号 `1.0.6-1`（`VERSION`）

本版两条主线：**350 系列（FM350 / RW350）的 IPv6 打通到「建 / 认 / 重拉」全链路**，
以及 **MT5700 / FM350 短信深度适配**。逐项依据见
[`docs/DIAL_STATE_MACHINE.md`](docs/DIAL_STATE_MACHINE.md) §12 与
[CHANGELOG.md](CHANGELOG.md) 的 [1.0.6] 条目。

## 这版做了什么

### 1. 350 系列的 IPv6 不再「判一次就定型」

350 系列的 USB 网卡不响应模组内 DHCP，IPv4 会被转成 `proto=static`；而
`modem_pro` 协议只在 `proto=modem_pro` 时才跑 `mp-client.sh`（odhcp6c 在里面）——
转 static 之后主接口既没 odhcp6c、也没有 v6 兄弟接口，表现就是「模组明明有 V6，
插件却认为没有」。原逻辑另有两处时序缺陷，本版逐层补齐：

| 阶段 | 原来 | 现在 |
| --- | --- | --- |
| 建接口 | 取决于 `wan_mode` 开关或主接口是否已转 static（= 已拿到 v4） | 350 系列只要 `wan_ipv6 != off` 就**无条件**建 `modem<id>_6`，不等 IPv4 |
| 认地址 | `AT+CGPADDR` 残值（只有 v4 时回一个无前缀 `::xxxx`）非空即信，此后不再重查 | 判据改为「是不是可用全局地址」，不是就重查；查询结果同步回写拨号状态 |
| 重拉 | 接口建起来后没人再碰 | 每轮读一次 `ifstatus`，up 满 20s 仍无全局地址/委派前缀 ⇒ `ifdown`+`ifup`，退避 30/60/120/240/300s，拿到地址立即清零 |

重拉只在 `dhcpv6` / `ra` 两种模式下动作，只动 `modem<id>_6`，且仅对 350 系列生效。

### 2. 新配置项 `mp_no_v6`（自动写、自动清）

同一张网卡上只能有一个 DHCPv6 客户端。350 的主接口仍是 `modem_pro` 时，插件写
`mp_no_v6=1`，协议脚本据此导出 `MP_NO_V6`，`mp-client.sh` 跳过 v6 上报脚本生成 /
odhcp6c 启动与巡检，`accept_ra` 也不再被翻起（否则内核在主网卡上自行 SLAAC，
兄弟接口永远看不到地址）。两条护栏：

- **确有接手方才让位**：`wan_ipv6=off` 时既不建兄弟接口，也不写这个键；
- **不留残留**：计划退回普通形态（换非 350 模组 / 主接口转 static / 关掉 IPv6）时
  自动 `uci delete network.modem<id>.mp_no_v6`，不会留下永久少跑 odhcp6c 的接口。

这个键由插件自己维护，**不需要手工配置**。

### 3. 短信（MT5700 / FM350 深度适配 + 四项补强）

- **发送按方言分路**：MT5700 / MT5710 走 `AT+CMGS=<TPDU 长度>` + 字面 `\r` + PDU + CR
  的一次性形态（该固件不实现 `>` 提示符两段式），仅在**本地** `ERROR` / `+CME ERROR`
  时退回标准两段式重试一次；`+CMS ERROR`（网络已拒）与超时（结果不明）一律不重试，
  避免重复投递。FM350 走标准两段式 + Ctrl-Z。
- **长短信按 UDH 拼接**：统一 8 位参考号 + 连接头，GSM7 每段 153 septet、
  UCS2 每段 67 个 UTF-16 单元，单段 TP-UD 上限 140 字节；不再拆成互不相关的多条。
- **收件号码归一化**：剥空格与 `-()`、认手写的 `00` 国际接入码、不带 `+` 的
  `8613800138000` 按国际格式、裸写 11 位手机号自动补 `86`、TOA 取 `0x91`/`0x81`、
  长度限 2~20 位，且幂等。
- **`AT+CSMP` 复位**：文本模式发英文前补 `AT+CSMP=17,167,0,0`。TP-DCS 是模组全局
  状态，上一条中文把它设成 UCS2 后不复位，这条英文会按 UCS2 发出去、对方收到乱码。
- **主动 arm `+CMTI`**：URC 常驻循环每轮补一次 `AT+CNMI=2,1,0,2,0`（确认成功才置位，
  未确认按 30s 限速重试），打破「没 arm ⇒ 没 `+CMTI` ⇒ 没人读短信 ⇒ 永远不 arm」的
  死锁。MT5700 掉电不保存 CNMI，此前重启后新短信既不进页面也不推微信，直到手动点开
  短信页。顺带 `AT+CMEE=2`，失败原因是可读的 `+CME ERROR: …` 而不是裸 `ERROR`。
- **状态报告不再产生鬼条目**：已发送区里的 SMS-STATUS-REPORT 不再留下一条没号码
  没正文的空短信，也不会被按 DELIVER 布局误读出假的「第 1/2 段」。
- **`AT+CPMS?` 六字段形态**与 LuCI 显示如实化：只回六个数字（无区名）时徽章显示
  「未报区名」而不是硬猜 `SM`，下拉框加禁用占位并挡住空值提交；发送框分段预估改为
  「首条 160/70，长短信每段 153/67」。

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

- 覆盖安装即可；版本号单一来源 `VERSION`（本版 `1.0.6-1`）。
- **350 系列会多出一个 `modem<id>_6` 接口**（`wan_ipv6=off` 时不建）。IPv4 与 IPv6
  从此分属两个接口：metric、防火墙 zone、`wan_dns` 下发口径与原来一致。
- 主接口在 350 上不再跑 odhcp6c，`logread -e modem-pro` 会有一条
  「接口 modem0 只跑 IPv4（mp_no_v6=1，IPv6 走独立 v6 接口）」，属预期。
- 若你**手工**建过同名 `modem<id>_6` 接口，插件的计划会覆盖它的 proto/device 等
  字段，建议改名或交给插件管理。
- `mp_no_v6` 与 `modem<id>_6` 全由插件维护，无需也不建议手改；`/etc/modemd/config.toml`
  是 conffiles，升级不覆盖你的取值。
- 无 API 路径与字段变更，无新增必填配置；IPv6 判定不再吃缓存，只在「缓存值不是可用
  全局地址」时多问一次 `AT+CGPADDR`。
- 排障入口：IPv6 看「v6 接口 modem<id>_6 已运行 Ns 仍无全局 IPv6（第 N 次重拉）」，
  短信失败现在回 `+CME ERROR: …` 原文。

## 已知边界（如实说明）

- **本版功能未经实机验证**（发版环境无模组、无 netifd）：350 上 `mp_no_v6` 后
  主接口 IPv4 与兄弟接口 IPv6 并行、`maybe_rekick` 在真实 RA 延迟下的重拉次数与
  退避表现、MT5700 掉电重启后 `+CMTI` 自动补 arm、MT5700 one-shot `CMGS` 与
  FM350 两段式的本地 `ERROR` 回退路径。
- 质量：`cargo test --locked` **794 项通过**（相对 1.0.5 的 777 项新增 17 项）；
  `cargo build --release` **0 warning**；`sh -n` 与 `node --check` 通过；
  `scripts/check-desensitize.sh` exit 0（286 文件）。
- **`cargo fmt --check` 与 `cargo clippy` 本次未执行**：发版工具链缺 `cargo-fmt` /
  `cargo-clippy` 组件，CI 也不跑这两项。所有改动按 ≤100 列与既有风格手工对齐，
  但仍需在有组件的环境里补跑一次再确认。

## 文档

`docs/DIAL_STATE_MACHINE.md`（拨号生命周期，本版新增 §12「350 系列的 IPv6 接口生命
周期」与 §12.1 短信 arm 同构问题）、`docs/INTEGRATION.md`（架构/数据流）、
`docs/BUILD.md`（构建/CI/验证）、`docs/API.md`（`/api/*` + WebSocket 全表）、
`docs/UPSTREAM_AUDIT.md`（审计与上游致谢）、`docs/SUPPORT_MATRIX.md`；
历史文档的处置见 `docs/README.md`。
