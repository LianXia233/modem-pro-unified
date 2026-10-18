# 构建与安装验证（BUILD）

本文以 **OpenWrt 25.x（apk）** 为主，同时适用于 24.10/23.05（opkg）。核心思路：
把本仓库当作一个 **package feed**，用官方 SDK 交叉编译出单个包 `modem-pro`。

---

## 0. 前置条件

- 官方 SDK 或完整源码树（`openwrt-25.x`，与目标固件**同版本、同 target**）。
- **必须启用 `feeds/packages`**（提供 `lang/rust` 工具链）。
- 主机侧需要联网拉取 crates（`rust/Cargo.lock` 已锁定版本）。

```sh
# 官方 SDK 里通常已带 feeds，确认存在 lang/rust：
ls feeds/packages/lang/rust/rust-package.mk
```

如缺失：

```sh
./scripts/feeds update packages
./scripts/feeds install -a -p packages
```

---

## 1. 把本仓库加入 feed

两种方式，任选其一。

### A. src-link（推荐本地开发）

```sh
cd /path/to/openwrt            # SDK 或源码树根
echo "src-link modem-pro /path/to/modem-pro-unified" >> feeds.conf.default
./scripts/feeds update modem-pro
./scripts/feeds install -a -p modem-pro
```

### B. git feed（发布后用）

```sh
echo "src-git modem-pro https://github.com/LianXia233/modem-pro-unified.git" >> feeds.conf.default
./scripts/feeds update modem-pro
./scripts/feeds install -a -p modem-pro
```

`src-link` 会直接把本目录映射进 `package/feeds/modem-pro/modem-pro`，改代码后无需重复 update。

---

## 2. 选择包与拨号器

```sh
make menuconfig
# Network -> Modem Manager -> <*> modem-pro
#   ├─ <*> 打包移远 Quectel 拨号器 (quectel-CM)
#   ├─ <*> 打包广和通 Fibocom 拨号器 (fibocom-dial)
#   ├─ <*> 打包美格 Meig 拨号器 (meig-cm)
#   └─ <*> 打包芯讯通 SIMCom 拨号器 (simcom-cm)
```

默认四家全开；按实际模组裁剪可省固件空间。

---

## 3. 编译

```sh
# 只编这个包（快）
make package/feeds/modem-pro/modem-pro/compile V=s

# 或者
make package/modem-pro/{clean,compile} V=s
```

产物：

- apk：`bin/packages/<arch>/modem-pro/modem-pro-<ver>.apk`
- opkg：`bin/packages/<arch>/modem-pro/modem-pro_<ver>_<arch>.ipk`

> 版本号来自仓库根 `VERSION`，如 `1.0.0-1` → `PKG_VERSION=1.0.0`、`PKG_RELEASE=1`。

---

## 4. 安装验证（真机）

```sh
# 传输到路由器
scp bin/packages/*/modem-pro/modem-pro-*.apk root@<router-ip>:/tmp/

# ---- OpenWrt 25.x（apk）----
ssh root@<router-ip>
apk add --allow-untrusted /tmp/modem-pro-*.apk

# ---- 24.10/23.05（opkg）----
opkg install /tmp/modem-pro_*.ipk
```

### 验证清单

```sh
# 1) 服务
/etc/init.d/modemd status
/etc/init.d/modemd restart
pgrep -f /usr/bin/modemd          # 有 pid

# 2) 接口（无需登录）
wget -qO- http://127.0.0.1:9527/api/version
#   → {"code":0,"data":{"version":"1.0.0-1",...},"msg":"ok"}
wget -qO- http://127.0.0.1:9527/api/modems
wget -qO- http://127.0.0.1:9527/api/modules

# 3) 模组被识别（插好模组后）
wget -qO- http://127.0.0.1:9527/api/modems?discover=1
#   → data 里出现 {id, at_port:"/dev/ttyUSBx", vendor, ...}

# 4) 直发 AT
wget -qO- "http://127.0.0.1:9527/api/at?modem_id=0&cmd=AT%2BCGSN"

# 5) LuCI
#    浏览器打开 http://<路由器>/cgi-bin/luci → 菜单「5G终端」（概览 / 服务控制）
#    接口页面选 proto=modem_pro（协议下拉里应出现）

# 6) 日志
logread -e modemd
tail -n 50 /var/log/modemd.log
```

### 常见问题

| 现象 | 排查 |
| --- | --- |
| 编译报「未找到 rust 包支持」 | 未启用 `feeds/packages` 的 `lang/rust`，见第 0 节 |
| `cargo install --locked` 失败 | `Cargo.lock` 缺失或被改；提交 `rust/Cargo.lock` 或用 `RUST_PKG_LOCKED:=0` |
| 服务起来但找不到模组 | 确认 `/dev/ttyUSB*` 存在且驱动（qmi_wwan/option）已加载；看日志 |
| LuCI 菜单不出现 | 清缓存：`rm -f /tmp/luci-indexcache*; rm -rf /tmp/luci-modulecache; /etc/init.d/rpcd reload` |
| apk 装不上（签名） | 加 `--allow-untrusted`，或把包纳入自己的签名流程 |

---

## 5. 本地快速自测（不依赖 OpenWrt）

Rust 部分可在任意平台编译与单测：

```sh
cd rust
cargo test
cargo build
# 用一个测试配置启动（示例）
cargo run -- -config ../docs/example-config.toml
```

守护进程在非 OpenWrt 环境会自动降低依赖：ubus 不可用时「跟随 LuCI 登录」放行并记日志，
串口枚举在 Windows 上返回 `COMx`。

---

## 5. GitHub Actions 云编译

`.github/workflows/build.yml` 提供两条官方 SDK 云编译流水线（**只要 Arm64 一档**，
`armsr/armv8` → 包架构 `aarch64_generic`，Cortex-A53 及以上 64 位设备通用）：

| 矩阵项 | SDK | 产物格式 |
| --- | --- | --- |
| 主线 APK | snapshots（25.x 主线） | `modem-pro-*.apk` |
| 24.10 IPK | 最新 24.10.x | `modem-pro_*.ipk` |

流程细分（每步一个 Action step，失败直接定位）：
安装依赖 → 生成 odhcp6c stub → 下载 SDK → 恢复缓存 → feeds → 选包配置 →
编译（V=s）→ 收集产物 + SHA256 → 上传 artifact。

要点：
- **触发**：推 `v*` 标签（编译 + 自动建 Release 上传产物）或手动 `workflow_dispatch`（只编译）；
- **缓存**：`dl/` 与 `staging_dir/host*`（含 rust host 工具链）。rust host 是**源码
  编译**（首跑 1~2 小时），二次起走缓存只需几分钟；
- **odhcp6c stub**：SDK 不带 base 包源码，`DEPENDS:+odhcp6c` 由本地 stub feed
  满足；stub 不发布，真机由固件自带；
- **发布**：`v1.0.0` 起 Release 附 APK/IPK 与 SHA256SUMS。
