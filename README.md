# modem-pro

OpenWrt 上统一管理 **5G/LTE 模组**（移远 Quectel、广和通 Fibocom、美格 Meig、芯讯通 SIMCom）
的单一软件包：一个 Rust 守护进程 `modemd`（**内置 QMI / MBIM 拨号**，含 QMAP 绑 mux）
+ netifd 协议 + LuCI 前端。

> 本仓库由原先分离的两个工程合并而成：LuCI 前端插件与模组后端。现在是一个仓库、一个包、一份版本号。

## 组成

| 目录 | 内容 |
| --- | --- |
| `rust/` | `modemd` 守护进程（Rust，按功能域细分模块）+ 内置调试台页面；`runtime/dialer/` 是 QMI/MBIM 拨号实现 |
| `files/` | netifd 协议脚本、默认 `config.toml`、mp-client wrapper |
| `luci/` | LuCI 前端：协议 JS、控制器、i18n、视图、菜单、ACL |
| `packaging/` | init 脚本与安装/卸载脚本 |
| `docs/` | 集成说明、构建说明、接口契约 |

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

- [docs/INTEGRATION.md](docs/INTEGRATION.md) — 整合方案：目录结构、依赖关系、命名与版本、向后兼容。
- [docs/BUILD.md](docs/BUILD.md) — SDK/feeds 编译与安装验证。
- [docs/API.md](docs/API.md) — `modemd` HTTP 接口契约。
- [docs/PORTING.md](docs/PORTING.md) — 接口级迁移清单（逐条路由的迁移状态）。
- [docs/PROGRESS.md](docs/PROGRESS.md) — 完整进度文档与后续提示词要求。
- [docs/HANDOFF.md](docs/HANDOFF.md) — 阶段交接表与硬性要求。
- [CHANGELOG.md](CHANGELOG.md) — 变更记录。

## 致谢

本项目的方向、接口契约与真机调参经验来自以下上游工程，特此致谢：

| 上游仓库 | 内容 |
| --- | --- |
| [ouyangzq/modem-pro](https://github.com/ouyangzq/modem-pro) | 模组侧守护进程 `modemd`：多模组管理、厂商指令方言、锁网/小区解析、短信与拨号托管等一整套真机打磨过的实现。本仓库的 HTTP/WS 接口、JSON 字段、AT 解析口径与配置项都以其为对齐基准。 |
| [ouyangzq/luci-proto-modem-pro](https://github.com/ouyangzq/luci-proto-modem-pro) | `modem_pro` netifd 协议：**一张接口同时承载 IPv4 + IPv6** 的 `mp-client.sh` wrapper 与 LuCI 协议表单，并给出 `accept_ra=2`、QMI 数据面、前缀漂移等实机注意事项。 |
| [LianXia233/luci-app-fm350](https://github.com/LianXia233/luci-app-fm350) | **350 系列模组**（广和通 FM350 / RW350）的真机参考：模组识别与 AT 方言、锁频/锁小区写法、IMEI 的 MFG 块备份与恢复等口径参考了该工程。 |

本仓库是在这些工程的基础上做「整合为单仓库 + 后端 Rust 重写」的工作，目标是与上述
接口契约逐字兼容。上游仓库未声明开源许可证，本仓库的 [MIT 许可证](LICENSE) 仅覆盖本仓库自身的代码。

## 许可证

MIT，见 [LICENSE](LICENSE)。
