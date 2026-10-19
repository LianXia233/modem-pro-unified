# SPDX-License-Identifier: GPL-2.0-only
#
# 统一包：modem-pro
#   · Rust 守护进程 modemd（rust/，内置 QMI/MBIM 拨号，不再依赖厂商 C 二进制）
#   · netifd 协议脚本（files/lib/netifd/proto/）
#   · LuCI 前端（luci/）
#   · 默认配置与 init 脚本（files/、packaging/）
#
# 版本号唯一来源：仓库根 VERSION（形如 1.0.0-1），打包时注入 MODEMD_VERSION。
# 构建说明见 docs/BUILD.md。

include $(TOPDIR)/rules.mk

PKG_NAME:=modem-pro
MODEM_PRO_VERSION:=$(strip $(shell cat $(CURDIR)/VERSION 2>/dev/null))
PKG_VERSION:=$(word 1,$(subst -, ,$(MODEM_PRO_VERSION)))
PKG_RELEASE:=$(word 2,$(subst -, ,$(MODEM_PRO_VERSION)))
PKG_VERSION:=$(if $(PKG_VERSION),$(PKG_VERSION),1.0.0)
PKG_RELEASE:=$(if $(PKG_RELEASE),$(PKG_RELEASE),1)

PKG_MAINTAINER:=LianXia233
PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE
PKG_BUILD_PARALLEL:=1
PKG_USE_MIPS16:=0

# ── Rust 守护进程的两种构建方式 ────────────────────────────────────────────────
# 默认（MODEM_PRO_HOST_RUST=0）走 feeds/packages 的 rust/host，行为不变：
#   · OpenWrt 从源码编译 rustc + LLVM，冷启动 1~2 小时；构建树与本地适用。
# 直编（MODEM_PRO_HOST_RUST=1）用【宿主 rustup 工具链】直接交叉编译：
#   · 宿主 rustc + 官方 aarch64-unknown-linux-musl target + 工具链自带 rust-lld
#     自包含链接，产出静态 musl 二进制（无运行时 libc 依赖，同架构固件通用）；
#   · 完全跳过 feeds 的 rust 源码编译，不再需要 rust/host 构建依赖 —— CI 走这条
#     （见 .github/workflows/build.yml）；
#   · 前置：宿主已 `rustup target add aarch64-unknown-linux-musl`；
#   · MODEM_PRO_CARGO_TARGET_DIR 可指向仓库外的稳定目录（如 CI 的 .cargo-target），
#     让 cargo 复用上次的编译产物。
#   开关取值来源（按优先级）：
#     ① 命令行 / 环境变量：本地或 CI 直接 `MODEM_PRO_HOST_RUST=1 make ...`；
#     ② 都没有时回退到 .config 符号 CONFIG_MODEM_PRO_HOST_RUST。
#   为何还要 ②：OpenWrt 的**包元数据扫描在 feeds update/install 期间就跑了**（子 make 在
#   每个包目录下 `make -r DUMP=1`），它读 .config 决定各包的 Build-Depends，结果落进
#   tmp/.packageinfo 并被后续 make 复用，且之后改 .config 不会触发重扫。因此 CI 必须把
#   CONFIG_MODEM_PRO_HOST_RUST=y 在 feeds 扫描【之前】就写进 .config，并把 ① 提到 job 级
#   env，让扫描阶段也带上——否则扫描会把 rust/host 记成构建依赖，之后怎么设都去源码编
#   rustc + LLVM（正是要绕开的那 1~2 小时）。
MODEM_PRO_HOST_RUST ?= $(if $(filter y,$(CONFIG_MODEM_PRO_HOST_RUST)),1,0)
MODEM_PRO_RUST_TARGET:=aarch64-unknown-linux-musl
MODEM_PRO_CARGO_TARGET_DIR ?= $(PKG_BUILD_DIR)/rust/target

# Rust 支持来自 feeds/packages（lang/rust）。缺失时在编译期给出明确提示。
RUST_PACKAGE_MK:=$(TOPDIR)/feeds/packages/lang/rust/rust-package.mk
ifeq ($(MODEM_PRO_HOST_RUST),1)
  HAVE_RUST:=0
else
  HAVE_RUST:=$(if $(wildcard $(RUST_PACKAGE_MK)),1,0)
endif

ifeq ($(HAVE_RUST),1)
  PKG_BUILD_DEPENDS:=rust/host
endif

include $(INCLUDE_DIR)/package.mk

ifeq ($(HAVE_RUST),1)
  MAKE_PATH:=rust
  include $(RUST_PACKAGE_MK)
  # 版本号注入走 CARGO_PKG_VARS：上游 Build/Compile/Cargo 的配方自带行首 `+`，
  # 外面不能再叠一层 `MODEMD_VERSION=... $(call ...)` —— 展开后会多出一个
  # 行首之外的 `+VAR=...`，shell 会把它当命令执行（not found）。
  CARGO_PKG_VARS+= MODEMD_VERSION="$(PKG_VERSION)-$(PKG_RELEASE)"
  define MODEMD/Compile
	$(call Build/Compile/Cargo)
  endef
else ifeq ($(MODEM_PRO_HOST_RUST),1)
  # 宿主工具链直编：cargo 交叉编译到 $(MODEM_PRO_RUST_TARGET)，
  # 链接交给工具链自带的 rust-lld（宿主 cc/ld 处理不了 aarch64 目标文件）。
  # 产物是静态 musl 二进制；拷进 PKG_INSTALL_DIR/bin，与 feeds 路径的安装步骤对齐。
  define MODEMD/Compile
	+cd $(PKG_BUILD_DIR)/rust && \
		CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
		MODEMD_VERSION="$(PKG_VERSION)-$(PKG_RELEASE)" \
		cargo build --release --locked \
			--target $(MODEM_PRO_RUST_TARGET) \
			--target-dir $(MODEM_PRO_CARGO_TARGET_DIR) && \
		$(INSTALL_DIR) $(PKG_INSTALL_DIR)/bin && \
		$(INSTALL_BIN) $(MODEM_PRO_CARGO_TARGET_DIR)/$(MODEM_PRO_RUST_TARGET)/release/modemd $(PKG_INSTALL_DIR)/bin/modemd
  endef
else
  define MODEMD/Compile
	$(error 未找到 rust 包支持：请先启用 feeds/packages（lang/rust），或设 MODEM_PRO_HOST_RUST=1 用宿主工具链直编。详见 docs/BUILD.md)
  endef
endif

define Package/modem-pro
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Modem Manager
  TITLE:=Unified 5G modem manager (modemd + LuCI)
  URL:=https://github.com/LianXia233/modem-pro-unified
  DEPENDS:=+luci-base +odhcp6c
endef

define Package/modem-pro/config
	config MODEM_PRO_HOST_RUST
		bool "用宿主 rustup 工具链直编 modemd（跳过 feeds 的 rust/host 源码编译）"
		depends on PACKAGE_modem-pro
		default n
endef

define Package/modem-pro/conffiles
/etc/modemd/config.toml
endef

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
	$(CP) $(CURDIR)/rust $(PKG_BUILD_DIR)/rust
endef

define Build/Compile
	# Rust 守护进程：注入统一版本号（单一来源 VERSION）。
	# QMI / MBIM 拨号（含 QMAP 绑 mux）都在 modemd 内完成，不再编译厂商 C 拨号器。
	$(call MODEMD/Compile)
endef

define Package/modem-pro/install
	# 守护进程
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/modemd $(1)/usr/bin/modemd

	# netifd 协议
	$(INSTALL_DIR) $(1)/lib/netifd/proto
	$(INSTALL_BIN) $(CURDIR)/files/lib/netifd/proto/modem_pro.sh $(1)/lib/netifd/proto/modem_pro.sh
	$(INSTALL_DIR) $(1)/usr/lib/modem-pro
	$(INSTALL_BIN) $(CURDIR)/files/usr/lib/modem-pro/mp-client.sh $(1)/usr/lib/modem-pro/mp-client.sh

	# 默认配置 + init
	$(INSTALL_DIR) $(1)/etc/modemd
	$(INSTALL_CONF) $(CURDIR)/files/etc/modemd/config.toml $(1)/etc/modemd/config.toml
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) $(CURDIR)/packaging/etc/init.d/modemd $(1)/etc/init.d/modemd

	# LuCI 协议 JS（interface proto=modem_pro 的表单）
	$(INSTALL_DIR) $(1)/www/luci-static/resources/protocol
	$(INSTALL_DATA) $(CURDIR)/luci/proto/htdocs/luci-static/resources/protocol/modem_pro.js $(1)/www/luci-static/resources/protocol/modem_pro.js

	# LuCI（ucode / JS 世代，2026-10-18 起唯一世代）
	# 菜单来自 menu.d/*.json（ucode dispatcher 只认这个 + Lua 控制器，
	# 已退役 Lua 世代，所以菜单标题就是这里的英文原文）；页面能力在
	# js/view/ 里，鉴权端点在 ucode/controller/ 里。
	$(INSTALL_DIR) $(1)/usr/share/ucode/luci/controller
	$(INSTALL_DATA) $(CURDIR)/luci/app/ucode/controller/modemd_auth.uc $(1)/usr/share/ucode/luci/controller/modemd_auth.uc
	$(INSTALL_DATA) $(CURDIR)/luci/app/ucode/controller/modemd_api.uc $(1)/usr/share/ucode/luci/controller/modemd_api.uc
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/modemd
	$(INSTALL_DATA) $(CURDIR)/luci/app/js/view/modemd/*.js $(1)/www/luci-static/resources/view/modemd/
	# 共享 JS 模块（api/task/store/fmt）：异步化架构的页面侧
	$(INSTALL_DIR) $(1)/www/luci-static/resources/modemd
	$(INSTALL_DATA) $(CURDIR)/luci/app/htdocs/luci-static/resources/modemd/*.js $(1)/www/luci-static/resources/modemd/

	# 菜单 + ACL
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) $(CURDIR)/luci/app/menu.d/luci-app-modemd.json $(1)/usr/share/luci/menu.d/luci-app-modemd.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) $(CURDIR)/luci/app/acl.d/luci-app-modemd.json $(1)/usr/share/rpcd/acl.d/luci-app-modemd.json
endef

define Package/modem-pro/postinst
$(file <$(CURDIR)/packaging/control/postinst)
endef

define Package/modem-pro/prerm
$(file <$(CURDIR)/packaging/control/prerm)
endef

define Package/modem-pro/postrm
$(file <$(CURDIR)/packaging/control/postrm)
endef

$(eval $(call BuildPackage,modem-pro))
