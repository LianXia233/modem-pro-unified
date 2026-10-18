# 整合方案（INTEGRATION）

本文回答 6 个问题：目录结构、单一顶层 Makefile、依赖与调用关系、命名与版本统一、
功能向后兼容、Rust 后端重构。构建步骤见 [BUILD.md](BUILD.md)，接口见 [API.md](API.md)。

---

## 1. 目录结构

```
modem-pro-unified/
├── Makefile                 # 唯一顶层 Makefile（OpenWrt package feed）
├── Config.in                # 厂商拨号器选装开关
├── VERSION                  # 版本号唯一来源，形如 1.0.0-1
├── LICENSE  README.md  CHANGELOG.md
├── .gitignore  .gitattributes   # 强制 LF
│
├── rust/                    # Rust 守护进程 modemd
│   ├── Cargo.toml  Cargo.lock
│   ├── web/                 # 内置调试台静态页（编译期嵌入二进制）
│   └── src/
│       ├── lib.rs           # 装配入口（run()）
│       ├── main.rs          # 最薄入口，转发到 lib
│       ├── error.rs  version.rs
│       ├── cli/             # 命令行参数
│       ├── config/          # 配置分节 + 加载（server/serial/sms/sim/net/dial/modem/ui/wechat）
│       ├── logging/         # 日志器 + 时间工具
│       ├── ubus/            # rpcd 会话（transport / rpc / session）
│       ├── modem/           # at / model / discovery / vendor / catalog / manager
│       ├── http/            # server / router / request / response / auth / static_ui / handlers
│       └── runtime/         # pid / signals / dialer / scheduler
│
├── third_party/             # 厂商源码（原样）
│   ├── quectel-cm/  fibocom-dial/  meig-cm/  simcom-cm/
│   └── kmods/               # 各家内核模块源码（QMI_WWAN / MHI / Gobinet / ndisc …）
│
├── files/                   # 运行时要落盘的文件
│   ├── lib/netifd/proto/{modem_pro.sh,quectel.sh}
│   ├── usr/lib/modem-pro/mp-client.sh
│   └── etc/modemd/config.toml
│
├── luci/
│   ├── proto/htdocs/luci-static/resources/protocol/modem_pro.js
│   └── app/
│       ├── ucode/controller/modemd_auth.uc
│       ├── js/view/modemd/{overview,service}.js
│       ├── menu.d/luci-app-modemd.json
│       └── acl.d/luci-app-modemd.json
│
├── packaging/
│   ├── etc/init.d/modemd    # procd 启动脚本
│   └── control/{postinst,prerm,postrm}
│
└── docs/{INTEGRATION,BUILD,API}.md
```

本地参考副本（若有）不参与构建。

---

## 2. 单一顶层 Makefile

一个 `Makefile` 产出一个包 `modem-pro`。要点：

- **Rust**：`MAKE_PATH:=rust` + `include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk`，
  用其 `Build/Compile/Cargo`（`cargo install --root $(PKG_INSTALL_DIR) --path $(PKG_BUILD_DIR)/rust`）。
  `PKG_BUILD_DEPENDS:=rust/host`。缺 rust feed 时编译期给出明确报错。
- **C 拨号器**：不使用各家自带 Makefile 的 `-Werror`，用 `TARGET_CC` 按显式源文件清单编译
  （清单与各家 Makefile 一致），输出到 `/usr/lib/modemd/`。用 `Config.in` 的
  `MODEM_PRO_{QUECTEL,FIBOCOM,MEIG,SIMCOM}` 裁剪。
- **版本注入**：`Build/Compile` 里以 `MODEMD_VERSION="$(PKG_VERSION)-$(PKG_RELEASE)"` 传给 cargo，
  代码用 `option_env!` 读取（直接 `cargo build` 时为 `dev`）。
- **安装**：`Package/modem-pro/install` 把守护进程、拨号器、netifd 脚本、默认配置、
  init 脚本、LuCI 文件装到目标路径；`config.toml` 声明为 conffile。
- **脚本**：`postinst/prerm/postrm` 由 `$(file <packaging/control/...)` 注入，避免内联大段 shell。

---

## 3. 依赖与调用关系

```
                        ┌──────────────────────────────────────────┐
   LuCI（:80）          │  uhttpd / rpcd(ubus) / netifd             │
   ├ modem_pro.js ──────┤  · interface proto=modem_pro             │
   ├ controller(modemd) │  · session.get / session.login           │
   └ view(overview/     │                                          │
        service)         └───────────────▲──────────────┬──────────┘
                                        │ ubus(HTTP)    │ 调起
                                        │               │
                           ┌────────────┴───────┐   ┌───▼───────────────┐
                           │  modemd (Rust)     │   │ mp-client.sh       │
   iframe → http://:9527/  │  · HTTP :9527      │   │  ├ udhcpc  (v4)    │
                           │  · config.toml     │   │  └ odhcp6c (v6)    │
                           │  · /dev/ttyUSB*    │   └───┬───────────────┘
                           └───┬───────────┬────┘       │
                               │ AT        │ 拨号器      │ proto_set_keep
                               ▼           ▼ 进程       ▼
                          5G 模组(AT口)   /usr/lib/modemd/<vendor>-*  → wwan0
```

- **配置**：`/etc/modemd/config.toml` 是唯一运行配置（conffile）。LuCI 的「服务控制」页
  直接读写其中 `[server]` 段（刷新间隔），改完重启服务生效。
- **ubus**：modemd 通过 `http://127.0.0.1/ubus` 调 `session.login` / `session.get`，
  用来实现「跟随 LuCI 登录」：页面把会话 ID 交给 modemd，modemd 存 cookie 后逐请求校验。
- **netifd**：`interface proto=modem_pro` 由 `modem_pro.sh` 处理；它只起一个
  `mp-client.sh`，由后者同时拉 `udhcpc`(IPv4) 与 `odhcp6c`(IPv6)，两侧上报脚本都调
  `proto_set_keep 1`，实现「一个接口同时承载 v4+v6 且互不清空」。
- **拨号器**：数据面（QMI/MBIM 建 PDN、拉起 wwan0）由厂商拨号器负责，两者按
  `AT+GMI` 识别的厂商选择，**不可互相顶替**。**MBIM 一律用 `quectel-CM`**（只有它
  实现了 MBIM 那套，且认 MBIM 靠标准 USB CDC 描述符、与厂商无关）。拨号器由
  modemd 托管：组网循环（15s 一轮）拉起、看着、退出自动重试，模组拔了/手动断开/
  拨号总开关关掉时按进程树收起（连它拉起的 `udhcpc`），见 `rust/src/runtime/dialer/`。
- **前端**：LuCI 页面（JS 世代 `.js` 视图）内嵌 modemd 自带调试台；
  协议 JS 让 `proto=modem_pro` 出现在接口配置表单里。

---

## 4. 命名与版本统一

| 项 | 约定 |
| --- | --- |
| 仓库/包名 | `modem-pro`（单一包） |
| 二进制 | `/usr/bin/modemd` |
| 拨号器 | `/usr/lib/modemd/{quectel-CM,fibocom-dial,meig-cm,simcom-cm}` |
| 协议名 | `modem_pro`（netifd 注册名；**不能用连字符**——shell 函数名非法） |
| 配置 | `/etc/modemd/config.toml` |
| 版本 | 仓库根 `VERSION` 单一来源；打包注入 `MODEMD_VERSION` |

> 迁移提示：若曾安装旧包（`modemd-pro` / `luci-app-modemd-pro` / `luci-proto-modem-pro`），
> 请先卸载再装 `modem-pro`，避免文件与菜单重复。

---

## 5. 功能与向后兼容

- **路径不变**：二进制、配置、init 名、netifd 协议名全部沿用既有约定，老配置开箱可用。
- **配置兼容**：`config.toml` 为 conffile；缺失键一律有默认值兜底，新增功能不需要改配置。
- **前端双世代**：同一包内同时提供 Lua LuCI 与 ucode/JS LuCI 两套文件，固件是哪代就用哪套。
- **LuCI 缓存**：`postinst/prerm/postrm` 会清 `/tmp/luci-indexcache*` 与 `/tmp/luci-modulecache`，
  避免改了控制器/菜单却不生效。

### 迁移状态

Rust 后端按域逐步等价迁移，**当前已落地**：版本/配置/日志、串口发现与 AT 收发、厂商识别、
信号（`AT+CSQ`）、网络（`AT+COPS?`）、直接发 AT、会话鉴权（ubus）、内置调试台静态页。

**待迁移**（按域推进，接口保持 `{code,data,msg}` 契约）：短信收发与未读、小区信息、锁频/
锁网、流量配额与统计、定时任务、企业微信推送、PCIe/MHI 管理、自动组网（WAN 编排）、
URC 主动上报与分帧、拨号器托管与回收（✅ `runtime/dialer/`，见 `PROGRESS.md` §25）、否定缓存与探测退避。清单同时是本项目 TODO。

---

## 6. Rust 后端重构（模块划分）

原后端为上千文件的工程，无法在一个改动内等价重写；本仓库采用「**单进程 + 按域细分模块**」
的方式渐进迁移。模块职责：

| 模块 | 职责 |
| --- | --- |
| `cli` | 参数解析（`-config`），纯函数 + 单测 |
| `config` | 按段拆文件（server/serial/sms/sim/net/dial/modem/ui/wechat），各段自带 `normalize()` |
| `logging` | 无第三方依赖的日志器（文件 + stderr）与 UTC 时间戳 |
| `ubus` | `transport`(HTTP) / `rpc`(JSON-RPC 编解码) / `session`(语义) 三层 |
| `modem::at` | `port`(打开/配置) + `command`(发送/读终结符) + `parser`(终结符/字段解析) |
| `modem::model` | `modem`(对象) + `view`(DTO) + `signal` + `network`，每个查询独立文件 |
| `modem::discovery` | `ports`(枚举) + `scanner`(探活) |
| `modem::manager` | 发现/列表/查询门面 |
| `modem::vendor` / `catalog` | 厂商枚举 / 支持模块清单 |
| `http` | `server` / `router` / `request` / `response` / `auth` / `static_ui` / `handlers/*` |
| `runtime` | `pid` / `signals` / `dialer` / `scheduler` |

依赖刻意精简（`serde`/`serde_json`/`toml`/`tiny_http`/`serialport`/`log`/`ctrlc`），
减少交叉编译与拉取的负担。`lib.rs` 提供可测试的库，`main.rs` 仅转发。

本地自测（任意平台）：

```sh
cd rust
cargo test
cargo build
```
