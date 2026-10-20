# 构建与安装验证

> 本文只写**当前仓库真实可执行的路径**。文中命令与 `Makefile`、`.github/workflows/build.yml`、
> `rust/Cargo.toml` 一一对应；最后核对 2026-10-20（`main`）。
>
> 版本号唯一来源：仓库根 `VERSION`（形如 `1.0.1-6`），打包时由 Makefile 以
> `MODEMD_VERSION` 注入，不在 `Cargo.toml` 里维护。

## 1. 两种后端构建方式

| 方式 | 开关 | 工具链 | 耗时 | 适用 |
| --- | --- | --- | --- | --- |
| feeds rust/host | 默认（`MODEM_PRO_HOST_RUST=0`） | OpenWrt 从源码编 rustc + LLVM | 冷启动 1~2 小时 | 构建树内完全自洽；无宿主 rustup |
| 宿主 rustup 直编 | `MODEM_PRO_HOST_RUST=1` | 宿主 rustc + `aarch64-unknown-linux-musl` | 数分钟 | **CI 走这条**；产物为静态 musl 二进制 |

直编路径的产物是静态 musl 二进制（无运行时 libc 依赖），同架构固件通用；链接交给工具链自带的
`rust-lld`（宿主 cc/ld 处理不了 aarch64 目标文件）。

## 2. 本机开发（不打包）

```sh
cd rust

cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked
cargo build --release --locked
```

产物 `rust/target/release/modemd`（Windows 为 `modemd.exe`）。开发期可直接运行：

```sh
./target/debug/modemd --config ./config.toml
```

`config.toml` 的完整字段见仓库 `files/etc/modemd/config.toml`；缺省值由
`rust/src/config/` 各分节自行归一化（读不到配置也会以「收紧后的默认值」启动）。

## 3. OpenWrt SDK / 构建树

### 3.1 把仓库放进包目录

任选一种：

```sh
# ① 直接拷进构建树
cp -r modem-pro-unified package/modem-pro

# ② 作为本地 feed（推荐，便于 git pull）
echo "src-link modem_pro /path/to/modem-pro-unified" >> feeds.conf.default
./scripts/feeds update modem_pro
./scripts/feeds install -a -p modem_pro
```

### 3.2 配置

```sh
make menuconfig
# Network -> Modem Manager -> modem-pro
```

需要宿主直编时勾选 `CONFIG_MODEM_PRO_HOST_RUST=y`（等价于命令行传 `MODEM_PRO_HOST_RUST=1`）。

> ⚠️ **顺序陷阱**：OpenWrt 的包元数据扫描发生在 `feeds update/install` 阶段
> （每个包目录跑一次 `make -r DUMP=1`），结果写进 `tmp/.packageinfo` 后被复用，
> 之后改 `.config` 不会重扫。所以走直编时，`CONFIG_MODEM_PRO_HOST_RUST=y` 必须在
> **feeds 扫描之前**就落进 `.config`，否则扫描仍会记下 `Build-Depends: rust/host`。
> CI 为此把开关放在 job 级 `env`（见 `build.yml` 注释）。

### 3.3 编译

```sh
# 默认：feeds 的 lang/rust
make package/modem-pro/compile V=s

# 宿主 rustup 直编（前置：rustup target add aarch64-unknown-linux-musl）
make package/modem-pro/compile V=s MODEM_PRO_HOST_RUST=1
```

可用环境变量：

| 变量 | 作用 |
| --- | --- |
| `MODEM_PRO_HOST_RUST=1` | 切宿主直编（优先于 `.config` 符号） |
| `MODEM_PRO_RUST_TARGET` | 交叉目标，默认 `aarch64-unknown-linux-musl` |
| `MODEM_PRO_CARGO_TARGET_DIR` | cargo 产物目录（CI 固定到工作区以复用缓存） |

## 4. 产物与依赖

包内容：

| 路径 | 内容 |
| --- | --- |
| `/usr/sbin/modemd` | Rust 守护进程 |
| `/etc/init.d/modemd` | procd 服务 |
| `/etc/modemd/config.toml` | 默认配置（conffiles，升级保留） |
| `/lib/netifd/proto/modem_pro.sh` | netifd 协议 |
| `/usr/lib/modem-pro/mp-client.sh` | 主机侧地址/路由客户端 |
| `/usr/share/luci/menu.d/`、`/www/luci-static/` | LuCI 页面 |

`DEPENDS`（见 `Makefile`）：`luci-base rpcd uhttpd-mod-ubus uhttpd-mod-ucode ucode-mod-fs
ucode-mod-math netifd wwan usbutils odhcp6c`，以及内核模块
`kmod-usb-net-qmi-wwan`、`kmod-usb-net-cdc-mbim`、`kmod-usb-net-rndis`、
`kmod-usb-net-cdc-ether`、`kmod-usb-serial-option`。

## 5. CI

`.github/workflows/build.yml`：

- **触发**：推 `v*` 标签（编译 + 建 Release 并上传）或手动 `workflow_dispatch`（只出 artifact）。
  **推 `main` 不触发**。
- **矩阵**：`armsr/armv8`（包架构 `aarch64_generic`）与 `mediatek/filogic`
  （`aarch64_cortex-a53`），各出 APK（snapshots）与 IPK（24.10.x）。
- **Rust**：`RUST_TOOLCHAIN: 1.93.0` + `MODEM_PRO_HOST_RUST=1`。
- **stub**：feeds 只拉本地 `modem_pro` 与 `ci_stub`，`luci-base` / `odhcp6c` 用空包满足元数据，
  避免去编 LuCI 组件而缺 base feed 头文件。

## 6. 安装后验证

```sh
# 1) 服务起来了吗
/etc/init.d/modemd status
logread -e modemd | tail -20

# 2) 版本与模组
curl -s http://127.0.0.1:8765/api/version
curl -s http://127.0.0.1:8765/api/modems

# 3) 发现是否按端口规则选了命令口（日志里有 group / 规则 / 证据）
logread -e "发现模组" | tail -5

# 4) 拨号
curl -s http://127.0.0.1:8765/api/dial
curl -s -X POST http://127.0.0.1:8765/api/dial/connect -d 'apn=<APN>&modem_id=0'

# 5) netifd 侧
ifstatus modem0
```

预期：`/api/modems` 只出现**一台物理设备一条**（多 tty 不再变多台）；
日志里能看到 `规则=端口规则 …`、`证据=…`；`/api/dial` 的 `phase` 依次从
`session` 走到 `iface_pending` / `route_pending` / `online`。

## 7. 常见问题

| 现象 | 原因 / 处理 |
| --- | --- |
| 编译时去编 rustc + LLVM（1~2 小时） | 没在 feeds 扫描前设置 `CONFIG_MODEM_PRO_HOST_RUST=y`；或命令行没传 `MODEM_PRO_HOST_RUST=1` |
| `missing lua.h / ucode/module.h` | 拉了真实 luci feed 又没 base feed 头文件；按 CI 的 stub 方案只拉本地 feed |
| 安装后模组不存在 | 先看 `logread -e modemd`：无 tty ⇒ `[serial] usb_autobind`；有 tty 但探活失败 ⇒ 看 `probe_identity` 日志与端口规则 |
| PCIe 模组不出现 | `[serial] pcie_autobind` 是否被关；`ls /sys/bus/pci/drivers/mhi_pci_generic` 是否有 bind |
| 一直「一台模组都没认出来」 | 可临时打开 `[serial] auto_rescue=true`（会短暂断开数据面），或手动 `echo 0 > …/authorized; echo 1 > …/authorized` |

## 8. 本仓库当前验证边界

`cargo fmt/test/clippy/build --release` 在开发机（Linux x86_64, rustc 1.93）已实际执行并通过；
**OpenWrt 包编译未在本次会话执行**（无 SDK/feeds 与交叉工具链）。首次在 SDK 里构建时，
请按 §7 的顺序陷阱检查一遍开关落点。
