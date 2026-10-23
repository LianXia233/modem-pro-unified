# modem-pro

OpenWrt 上统一管理 **5G/LTE 模组**的单一软件包：覆盖 **13 家厂商共 92 个型号**
（移远 Quectel、广和通 Fibocom、美格 Meig、芯讯通 SIMCom、华为 Huawei、百旺 Baiwang、
合宙 OpenLuat、富士康 Foxconn、高新兴 Gosuncn、有方 Neoway、Sierra、Telit、Thales），
一个 Rust 守护进程 `modemd`（**内置 QMI / MBIM 拨号**，含 QMAP 绑 mux）
+ netifd 协议 + LuCI 前端。逐型号支持程度见
[docs/SUPPORT_MATRIX.md](docs/SUPPORT_MATRIX.md)（收录 / 识别 / 后端 / 自动化 / 实机五级）。

> 本仓库由原先分离的两个工程合并而成：LuCI 前端插件与模组后端。现在是一个仓库、一个包、一份版本号。

## 组成

| 目录 | 内容 |
| --- | --- |
| `rust/` | `modemd` 守护进程（Rust，按功能域细分模块）+ 内置调试台页面；`runtime/dialer/` 是 QMI/MBIM 拨号实现 |
| `files/` | netifd 协议脚本、默认 `config.toml`、mp-client wrapper |
| `luci/` | LuCI 前端：协议 JS、控制器、i18n、视图、菜单、ACL |
| `packaging/` | init 脚本与安装/卸载脚本 |
| `docs/` | 架构说明、构建说明、接口契约、上游审计与支持矩阵 |

## 快速开始（设备端）

```sh
# 安装后
/etc/init.d/modemd status
/etc/init.d/modemd restart
# 原生独立 WebUI 调试台： http://<路由器地址>:9527/
# LuCI 菜单： Mobile Network（移动网络） -> 5G 终端（二级单入口，支持页内横向左右滑动 TAB 栏）
```

配置在 `/etc/modemd/config.toml`（conffile，升级不覆盖）；改完 `restart` 生效。

### 特色亮点

- **统一精致的白色毛玻璃视觉规范（White Glassmorphism）**：适度透明模糊卡片、高光轻薄边框、低饱和状态强调色与规整弹性自适应栅格。
- **二级单入口 + 页内横向滑动 TAB**：收敛侧栏菜单树，杜绝下拉式菜单冗余；页内 TAB 栏支持滚轮水平滚动、左右微调与触控自适应。
- **原生动态 SVG 信号系统**：5 档蜂窝天线信号计真实联动 3GPP 评级，RSRP/SINR/RSRQ/RSSI 4 宫格物理量程 SVG 进度刻度槽。
- **与 WebUI 100% 完整对齐**：8 大核心业务页面与 10 项调试工具箱全功能覆盖，数据源、参数交互与刷新状态无缝同步。
- **原生 Rust 守护进程**：内置 QMI / MBIM 拨号协议托管，无需外部厂商 C 二进制。
- **统一拨号生命周期**：AT 与 QMI/MBIM 共用意图、状态、退避与连接代次的单一协调器，
  插拔 / 重启 / 飞行模式 / 配额断网均可正确回收与重拨（设计见 [docs/DIAL_STATE_MACHINE.md](docs/DIAL_STATE_MACHINE.md)）。
- **794 项自动化测试守护**（`cargo test --locked`）：配方、解析、状态机、端口规则逐条钉住。

## 从源码构建

见 [docs/BUILD.md](docs/BUILD.md)。核心：把本仓库当作 OpenWrt 的 package feed。
Rust 后端有两条构建路径：默认走 `feeds/packages`（提供 `lang/rust`，需启用）；
或用宿主 `rustup` 工具链直编（`MODEM_PRO_HOST_RUST=1`，跳过 rust 源码编译）。

```sh
echo "src-link modem-pro /path/to/modem-pro-unified" >> feeds.conf.default
./scripts/feeds update -a && ./scripts/feeds install -a
make menuconfig   # Network -> Modem Manager -> modem-pro
make package/modem-pro/compile V=s                    # 默认：feeds 的 lang/rust
make package/modem-pro/compile V=s MODEM_PRO_HOST_RUST=1   # 宿主 rustup 直编
# 也可在 menuconfig 勾选「用宿主 rustup 工具链直编 modemd」（即 CONFIG_MODEM_PRO_HOST_RUST=y，等价）
```

## 文档

- [docs/README.md](docs/README.md) — 文档索引。
- [docs/INTEGRATION.md](docs/INTEGRATION.md) — 架构分层、数据流、数据表与扩展指南。
- [docs/BUILD.md](docs/BUILD.md) — 本机开发、SDK 构建、CI 与安装后验证。
- [docs/API.md](docs/API.md) — HTTP / WebSocket 接口契约（由路由表导出）。
- [docs/UPSTREAM_AUDIT.md](docs/UPSTREAM_AUDIT.md) — 上游源码审计、迁移矩阵、验证结果、真机待办与上游致谢清单（§8 OpenVohive QMI 会话边界与 TLV 编号裁决、§9 Siriling 多驱动审计、§11 luci-app-mt5700 与厂商 AT 手册整合）。
- [docs/SUPPORT_MATRIX.md](docs/SUPPORT_MATRIX.md) — 92 型号逐项支持矩阵（五级支持程度）与厂商数据通道指令。
- [docs/DIAL_STATE_MACHINE.md](docs/DIAL_STATE_MACHINE.md) — 拨号生命周期审计与统一状态机设计。
- [CHANGELOG.md](CHANGELOG.md) — 变更记录；[RELEASE_NOTES.md](RELEASE_NOTES.md) — 发版说明。

> 2026-10-20 已按**当前代码**重写 `INTEGRATION.md` / `BUILD.md` / `API.md`（旧版在
> 提交 `66853b8` 被删除，内容已过时，未恢复）。`PORTING.md` / `PROGRESS.md` /
> `HANDOFF.md` / `PRIVACY.md` / `DIAL_AUDIT.md` 不再维护，见 `docs/README.md` 的说明。

## 致谢

本项目的方向、接口契约与真机调参经验来自以下上游工程，特此致谢：

| 上游仓库 | 内容 |
| --- | --- |
| [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro) | 模组侧守护进程 `modemd`：多模组管理、厂商指令方言、锁网/小区解析、短信与拨号托管等一整套真机打磨过的实现。本仓库的 HTTP/WS 接口、JSON 字段、AT 解析口径与配置项都以其为对齐基准。 |
| [ouyangzq/luci-proto-modem-pro](https://github.com/ouyangzq/luci-proto-modem-pro) | `modem_pro` netifd 协议：**一张接口同时承载 IPv4 + IPv6** 的 `mp-client.sh` wrapper 与 LuCI 协议表单，并给出 `accept_ra=2`、QMI 数据面、前缀漂移等实机注意事项。 |
| [LianXia233/luci-app-fm350](https://github.com/LianXia233/luci-app-fm350) | **350 系列模组**（广和通 FM350 / RW350）的真机参考：模组识别与 AT 方言、锁频/锁小区写法、IMEI 的 MFG 块备份与恢复等口径参考了该工程。 |
| [LianXia233/luci-app-mt5700](https://github.com/LianXia233/luci-app-mt5700) | **MT5700M-CN**（鼎桥 TD Tech）私有 AT 命令族：`AT^MONSC` / `AT^HCSQ` / `AT^DSFLOWQRY` / `^RSSI` URC 等口径据此补齐鼎桥信号与流量链路（裁决记录见 `docs/UPSTREAM_AUDIT.md` §11）。GPL-3.0，**仅参考未拷贝**。 |
| [FUjr/QModem](https://github.com/FUjr/QModem) | 模组知识底座：`modem_support.json` 型号清单、USB 接口号白名单与端口规则、语音 PCM 接口排除、12 家厂商 AT 方言与拨号脚本。本仓库的发现白名单 / 分组策略、厂商数据通道指令表、发现退避参数均以其源码为基准（逐条核对记录见 `docs/UPSTREAM_AUDIT.md`）。许可为 MPL-2.0 并附禁止商用附加条款。 |
| [6106757-lab/openvohive](https://github.com/6106757-lab/openvohive) | QMI/MBIM 会话边界对照基准：WDS Start/Stop/Runtime、WDA Set Data Format、Bind Mux 的 TLV 编号与流程；据此修正 IPv6 运行时设置缺口（裁决记录见 `docs/UPSTREAM_AUDIT.md` §8）。许可为 PolyForm Noncommercial 1.0.0，**仅参考未拷贝**。 |
| [hchyhchyxh/luci-app-rg501q-eu](https://github.com/hchyhchyxh/luci-app-rg501q-eu) | RG501Q-EU（`2c7c:0800`）的 USB 接口号口径（AT 口 1.2/1.3、QMI 1.4）与不绑 option 驱动的端口规则。Apache-2.0，**仅事实依据未拷贝**。 |
| [ainuoyan/QDC507-Modem-Support](https://github.com/ainuoyan/QDC507-Modem-Support) | 百旺 QDC507（`2c7c:0125`）识别口径、移远方言 AT 与通用 `CGACT` 拨号配方。GPL-3.0，**仅事实依据未拷贝**。 |
| [Siriling/5G-Modem-Support](https://github.com/Siriling/5G-Modem-Support) | RMNET / GobiNet / MHI / PCIe 多驱动拨号工具与 LuCI 应用的差异清单（据此补齐 `fibo_mhi` 驱动与 Fibocom MHI 数据网卡名识别）。GPL-3.0，**仅参考未拷贝**。 |

本仓库是在这些工程的基础上做「整合为单仓库 + 后端 Rust 重写」的工作，目标是与上述
接口契约逐字兼容。各上游仓库许可见上表与 `docs/UPSTREAM_AUDIT.md` §10；
本仓库的 [MIT 许可证](LICENSE) 仅覆盖本仓库自身的代码，整合过程中
**未拷贝任何上游代码**（GPL / PolyForm 组件仅作事实依据与设计参考）。

### 关于厂商拨号器 C 源码

内置 QMI / MBIM 拨号器是**本仓库的独立 Rust 实现**（`rust/src/runtime/dialer/`），
**不包含、未拷贝任何厂商 C 源码**（quectel-CM / fibocom-dial / meig-cm / simcom-cm 等均未采用），
因此不列入致谢。开发过程中仅将这些 C 实现作为协议行为的事实基准逐条对照
（TLV 顺序、常量、流程），差异与取舍记录见 [docs/UPSTREAM_AUDIT.md](docs/UPSTREAM_AUDIT.md) §7–§8
与 [CHANGELOG.md](CHANGELOG.md) 的 [1.0.3] 条目。

## 许可证

MIT，见 [LICENSE](LICENSE)。
