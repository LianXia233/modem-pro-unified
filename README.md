<div align="center">

# 🛰️ Modem Pro · 统一模组管理系统

**适用于 OpenWrt 的新一代 5G/LTE 蜂窝模组一体化管控中心 · 原生 Rust 守护进程 · 覆盖 13 家厂商 92 款型号**

[![Version](https://img.shields.io/badge/Version-v1.0.8--1-blue?style=flat-square&logo=git&logoColor=white)](VERSION)
[![Daemon](https://img.shields.io/badge/Daemon-Rust%20modemd-DEA584?style=flat-square&logo=rust&logoColor=white)](rust/)
[![Dialer](https://img.shields.io/badge/Dialer-Built--in%20QMI%20%7C%20MBIM-orange?style=flat-square)](#-自研原生-qmimbim-拨号器)
[![Vendors](https://img.shields.io/badge/Vendors-13%20Brands%20%7C%2092%20Models-success?style=flat-square)](#-支持厂商与型号生态)
[![UI Style](https://img.shields.io/badge/UI-White%20Glassmorphism-0ea5e9?style=flat-square)](#-特色视觉与交互亮点)
[![License](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)

<p align="center">
  <b>单包全栈交付</b>：内置自研纯 Rust 原生 QMI / MBIM 拨号引擎 · netifd 双栈协议承载 · 白色毛玻璃现代 LuCI 界面<br>
  涵盖全生命周期自动重拨 · 动态 SVG 射频仪表 · 8 大核心业务页面 · 10 大专业调试工具 · 企业微信状态告警
</p>

</div>

---

> [!NOTE]
> **📦 架构整合演进**
>
> 本仓库由原先独立的两个工程深度整合重构而来：**LuCI 现代前端插件** 与 **Rust 模组通信后端**。现已合流为**单代码仓库、单安装软件包、统一版本号发布**，彻底告别旧版跨项目依赖不一致与接口脱节的痛点。

---

## 🌟 核心特性与架构优势

| 维度 | 架构实现与核心优势 |
|:---|:---|
| 🦀 **原生 Rust 守护进程 (`modemd`)** | 基于纯 Rust 开发的独立常驻守护进程，按业务领域高内聚解耦；常驻内存微量，无外部动态库依赖，具备极高的稳定性与并发调度能力。 |
| ⚡ **自研原生 QMI / MBIM 拨号引擎** | 内置独立自研的纯 Rust 拨号实现，**彻底摆脱厂商历史遗留 C 源码二进制**（如 `quectel-CM`、`fibocom-dial` 等）；原生支持 QMAP 绑定复用与 IPv4/IPv6 单接口双栈承载。 |
| 🌐 **13 家厂商 92 款型号全覆盖** | 覆盖移远、广和通、美格、芯讯通、华为、百旺、合宙、富士康、高新兴、有方、Sierra、Telit、Thales 等主流厂商全系 5G/4G 模组。 |
| 💎 **白色毛玻璃视觉规范 (QM Glass)** | 适度透明模糊玻璃质感、轻薄高光微边框、低饱和度状态强调色与规整弹性自适应栅格，带来耳目一新的现代交互体验。 |
| 📱 **二级单入口 + 优雅横滚 TAB** | 侧边栏收敛为单一入口（`移动网络 → 5G 终端`），杜绝菜单树冗余；页内 TAB 栏支持鼠标滚轮横滚、平滑微调与触控滑动手势。 |
| 📡 **动态矢量 SVG 信号量程系统** | 5 档天线信号条真实对接 3GPP 国际评级规范；RSRP / SINR / RSRQ / RSSI 4 宫格物理量程 SVG 进度刻度槽实时驱动。 |
| 🔄 **全生命周期自愈与重拨协调器** | AT 指令与 QMI/MBIM 会话共用意图、状态机、退避与连接代次的单一协调器；插拔、异常闪退、飞行模式与配额断网均可自动平滑回收与自愈重拨。 |
| 🧪 **794 项自动化用例保驾护航** | 拥有 794 项严格的 Rust 单元测试与回归测试（`cargo test --locked`），将厂商方言解析、端口规则与状态机逻辑逐条钉死。 |

---

## 🎛️ 业务视图与调试工具矩阵

系统采用「8 大核心业务页 + 10 大诊断调试工具箱」的完整矩阵设计：

### 8 大核心业务视图

| 视图模块 | 路由入口 | 核心能力说明 |
|:---|:---|:---|
| 📊 **总览概况 (Overview)** | `admin/modem/modemd` | 模组核心元数据大屏、5 档蜂窝天线动态信号计、4 宫格物理射频刻度槽、载波聚合（CA）详情与实时上下行吞吐火花线。 |
| 🌐 **网络拨号 (Dial)** | TAB: 拨号设置 | QMI / MBIM / NCM 拨号协议切换、APN 参数配置、IPv4/IPv6 双栈策略与会话拨号守护状态。 |
| 📡 **基站小区 (Cells)** | TAB: 小区信息 | 服务小区 PCI / EARFCN / TAC、频段信息、物理层信号参数与周边邻区基站扫描结果。 |
| 🔒 **频段锁定 (Lock)** | TAB: 频段锁定 | 支持一键锁定指定 5G NR / LTE 工作频段（Band）、指定频点及基站物理小区 ID（PCI）。 |
| 🧩 **模组管理 (Modules)** | TAB: 硬件模组 | 模组硬件详细信息、固件版本查询、USB / PCIe 接口拓扑识别与多模组平滑切换。 |
| 💬 **短消息中心 (SMS)** | TAB: 短信中心 | 现代会话式短信收发、长短信自动拼接重组、收发历史归档与未读标记管理。 |
| ⚙️ **系统服务 (Service)** | TAB: 服务配置 | 守护进程参数调优、API 端口绑定、事件通知策略与企业微信机器人 Webhook 告警。 |
| 🧰 **综合工具箱 (Tools)** | TAB: 调试工具 | 集成 10 大专业模组调试工具集，为射频调优与故障排查提供底层支撑。 |

### 10 项内置专业调试工具

1. 💻 **交互式 AT 终端**：常用指令自动补全、历史回溯、执行状态高亮与原语收发调试；
2. 🔄 **软硬件复位重启**：支持指令级软重启与模组物理拉高重启；
3. ✈️ **飞行模式切换**：一键关闭/开启射频前端，重置无线注册状态；
4. 🛰️ **GNSS / GPS 定位**：硬件卫星定位通道启闭、NMEA 语句输出与坐标解析；
5. 📶 **搜网与接入偏好**：手动搜索周边可用运营商网络、指定 5G/4G 接入优先级；
6. 📡 **射频特性开关**：NR 载波聚合、DSS 动态频谱共享与 VoNR 高清通话开关；
7. 🌡️ **芯片温度矩阵**：底层多路传感器结温实时监控；
8. ⚡ **功耗模式调节**：低功耗睡眠模式与唤醒策略管理；
9. 📝 **拨号日志追踪**：QMI/MBIM 握手过程与 netifd 协议状态日志实时回显；
10. 🧹 **出厂配置重置**：一键清除 NVRAM 缓存与射频杂项设定，恢复出厂默认值。

---

## 🚀 设备端快速上手

### 1. 服务启停与状态验证

安装完成后，守护进程自动由 `procd` 注册并接管：

```sh
# 查看守护进程运行状态
/etc/init.d/modemd status

# 重启模组管理服务
/etc/init.d/modemd restart

# 观察后台监听与 RPC 接口 (默认监听 127.0.0.1:9527 由 LuCI 同源代理)
netstat -tulpn | grep modemd
```

进入 Web 管理界面：**移动网络 (Mobile Network)** → **5G 终端** 即可直接访问。

### 2. 配置文件说明

全局核心配置文件位于 `/etc/modemd/config.toml`（声明为 `conffile`，固件升级与包更新均**不会覆盖**用户个性化配置）：

```toml
[server]
listen = "127.0.0.1:9527"        # 后台 API 监听地址
log_level = "info"               # 日志级别: debug / info / warn / error

[modem]
auto_detect = true               # 自动探测系统 USB / PCIe 模组
default_dial_mode = "qmi"        # 默认拨号方式: qmi / mbim / at

[notify]
wechat_webhook = ""              # 企业微信机器人告警 Webhook 地址
```

修改配置后执行 `/etc/init.d/modemd restart` 即可热加载生效。

---

## 🛠️ 从源码构建指南

将本仓库作为 OpenWrt 的一个 package feed 引入编译工程。

### 两种构建路径可选

| 构建方式 | 命令 / 开关 | 特点与适用场景 |
|:---|:---|:---|
| **路径 A：宿主 rustup 直编 (强烈推荐)** | `MODEM_PRO_HOST_RUST=1` | 利用宿主现成的 `rustc` 与 `aarch64-unknown-linux-musl` target 直编，**跳过耗时数小时的 host-rust 源码编译**，秒级极速构建。 |
| **路径 B：标准 feed 编译** | 默认行为 | 依赖 `feeds/packages` 提供的 `lang/rust` 在编译链中完成完整构建。 |

```sh
# 1. 引入本地或远程 feed
echo "src-link modem-pro /path/to/modem-pro-unified" >> feeds.conf.default

# 2. 更新并安装 feed
./scripts/feeds update -a && ./scripts/feeds install -a

# 3. 菜单勾选
make menuconfig
# 进入: Network -> Modem Manager -> modem-pro (勾选)
# (可选: 勾选「用宿主 rustup 工具链直编 modemd」即 CONFIG_MODEM_PRO_HOST_RUST=y)

# 4. 执行编译
# 方式 A：宿主 rustup 直编 (秒级)
make package/modem-pro/compile V=s MODEM_PRO_HOST_RUST=1

# 方式 B：标准 feed 编译
make package/modem-pro/compile V=s
```

---

## 📱 支持厂商与型号生态 (13 家 92 款)

本系统内置庞大的模组硬件知识底座与方言库，开箱支持以下厂商的主流通信模组：

- 🔹 **移远通信 (Quectel)**：RM500Q、RM520N、RG500Q、RG520N、RG650V、EC20 等全系模组；
- 🔹 **广和通 (Fibocom)**：FM350-GL、FM160、FM650、NL668、FG150、FG360 等；
- 🔹 **美格智能 (MeigLink)**：SRM825N、SLM828、SLM750 等；
- 🔹 **芯讯通 (SIMCom)**：SIM8200、SIM8202、SIM7600 等；
- 🔹 **华为 (Huawei)**：ME909s、MH5000 等；
- 🔹 **鼎桥 (TD Tech)**：MT5700M-CN 系列；
- 🔹 **合宙 (OpenLuat)** / **百旺 (Baiwang)** / **高新兴 (Gosuncn)** / **有方 (Neoway)**；
- 🔹 **海外主流模组**：Sierra Wireless、Telit、Thales (Cinterion)、Foxconn 等。

---

## 📂 仓库项目架构

```text
modem-pro-unified/
├── rust/                       # Rust 守护进程 modemd 核心源码
│   ├── src/
│   │   ├── http/               # REST API 路由与 HTTP 服务
│   │   ├── modem/              # 13 家模组厂商方言与 AT 控制层
│   │   │   └── vendor/         # 各厂商专有指令集与解析器 (quectel/fibocom/meig...)
│   │   ├── runtime/            # 拨号器与状态协调引擎
│   │   │   └── dialer/         # 纯 Rust 自研 QMI / MBIM 协议实现 (含 native 驱动)
│   │   ├── ubus/               # OpenWrt ubus 本地 RPC 桥接
│   │   └── wechat/             # 企业微信告警推送客户端
│   ├── Cargo.toml
│   └── Cargo.lock
├── files/                      # 系统运行期资产
│   ├── etc/modemd/config.toml  # 守护进程默认配置文件
│   └── lib/netifd/proto/       # OpenWrt netifd 协议集成脚本 (modem_pro.sh)
├── luci/                       # LuCI 现代前端
│   └── app/
│       ├── htdocs/resources/modemd/ # 前端 UI 资源 (SVG 图标、CSS 样式、API 封装)
│       └── js/view/modemd/     # 8 大核心视图组件脚本 (overview/dial/cells...)
├── packaging/                  # procd 启动脚本与安装后脚本
├── scripts/                    # 开发与敏感信息检查辅助脚本
├── Makefile                    # 顶层 OpenWrt 编译规则定义
├── VERSION                     # 统一版本号声明 (1.0.8-1)
└── README.md                   # 项目说明文档
```

---

## 💖 致敬与鸣谢

本项目在演进过程中，充分汲取并参考了开源社区诸多优秀项目的经验与接口规范：

| 引用工程 | 主要参考与致谢领域 |
|:---|:---|
| [**ouyangzq/modem-pro**](https://github.com/ouyangzq/modem-pro) | 模组守护进程接口契约、厂商方言解析思路、锁频锁网与短信托管等实机打磨经验。 |
| [**ouyangzq/luci-proto-modem-pro**](https://github.com/ouyangzq/luci-proto-modem-pro) | netifd 协议承载形态、IPv4/IPv6 单接口双栈绑定技巧及 `mp-client.sh` 架构。 |
| [**LianXia233/luci-app-fm350**](https://github.com/LianXia233/luci-app-fm350) | 广和通 350 系列模组（FM350/RW350）实机识别口径与 AT 方言参考。 |
| [**LianXia233/luci-app-mt5700**](https://github.com/LianXia233/luci-app-mt5700) | 鼎桥 MT5700M 私有 AT 命令族（`^MONSC`、`^HCSQ`、`^DSFLOWQRY`）的信号与流量换算标准。 |
| [**FUjr/QModem**](https://github.com/FUjr/QModem) | 模组型号清单知识库、USB 接口白名单与端口映射规则。 |
| [**6106757-lab/openvohive**](https://github.com/6106757-lab/openvohive) | QMI/MBIM 会话边界与 WDS/WDA 报文处理规范参考。 |

> ⚠️ **关于拨号器源码独立性的重要声明**：  
> 本项目的内置 QMI / MBIM 拨号引擎为**纯自研的独立 Rust 实现**（位于 `rust/src/runtime/dialer/`），**未包含亦未拷贝任何厂商闭源或历史 C 源码**。

---

> 📜 **开源许可**：本项目基于 [MIT 许可证](LICENSE) 发布。
