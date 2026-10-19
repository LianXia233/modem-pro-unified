# SPDX-License-Identifier: GPL-2.0-only
#
# 统一包：modem-pro
#   · Rust 守护进程 modemd（rust/）
#   · 厂商主机侧拨号器（third_party/，按包内 config 子选项 MODEM_PRO_* 裁剪）
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

# 厂商拨号器源文件清单（与各家自带 Makefile 保持一致，但不使用其 -Werror）。
QUECTEL_SRC:=QmiWwanCM.c GobiNetCM.c main.c QCQMUX.c QMIThread.c util.c qmap_bridge_mode.c mbim-cm.c device.c atc.c atchannel.c at_tok.c udhcpc.c
FIBOCOM_SRC:=QmiWwanCM.c GobiNetCM.c main.c MPQMUX.c QMIThread.c util.c qmap_bridge_mode.c query_pcie_mode.c udhcpc_netlink.c libmnl/ifutils.c libmnl/attr.c libmnl/callback.c libmnl/nlmsg.c libmnl/socket.c
MEIG_SRC:=QmiWwanCM.c GobiNetCM.c main.c MPQMUX.c QMIThread.c util.c udhcpc.c qmap_bridge_mode.c mbim-cm.c device.c
SIMCOM_SRC:=QmiWwanCM.c GobiNetCM.c main.c MPQMUX.c QMIThread.c util.c udhcpc.c

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

TP:=$(PKG_BUILD_DIR)/third_party
DL_OUT:=$(PKG_BUILD_DIR)/dialers

# $(1)=源目录 $(2)=源文件清单 $(3)=输出 $(4)=链接库 $(5)=额外 CFLAGS
define cc_dialer
	$(TARGET_CC) $(TARGET_CPPFLAGS) $(TARGET_CFLAGS) $(5) $(addprefix $(1)/,$(2)) -o $(3) $(4)
endef

define Package/modem-pro
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Modem Manager
  TITLE:=Unified 5G modem manager (modemd + dialers + LuCI)
  URL:=https://github.com/LianXia233/modem-pro-unified
  DEPENDS:=+luci-base +odhcp6c
endef

# 四家拨号器的 QMI 初始化序列互不兼容，必须按模组厂商选装（装错会拨不上）。
# 默认全开，方便一次装齐；可以把用不到的品牌关掉以省固件空间。
# 子选项内联在此（OpenWrt 包内 config 机制，生成 CONFIG_MODEM_PRO_* 符号，
# 被 Build/Compile 的 $(if $(CONFIG_MODEM_PRO_*),...) 引用决定各家拨号器是否编译）。
define Package/modem-pro/config
	config MODEM_PRO_HOST_RUST
		bool "用宿主 rustup 工具链直编 modemd（跳过 feeds 的 rust/host 源码编译）"
		depends on PACKAGE_modem-pro
		default n

	config MODEM_PRO_QUECTEL
		bool "打包移远 Quectel 拨号器 (quectel-CM)"
		depends on PACKAGE_modem-pro
		default y

	config MODEM_PRO_FIBOCOM
		bool "打包广和通 Fibocom 拨号器 (fibocom-dial)"
		depends on PACKAGE_modem-pro
		default y

	config MODEM_PRO_MEIG
		bool "打包美格 Meig 拨号器 (meig-cm)"
		depends on PACKAGE_modem-pro
		default y

	config MODEM_PRO_SIMCOM
		bool "打包芯讯通 SIMCom 拨号器 (simcom-cm)"
		depends on PACKAGE_modem-pro
		default y
endef

define Package/modem-pro/conffiles
/etc/modemd/config.toml
endef

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
	$(CP) $(CURDIR)/rust $(PKG_BUILD_DIR)/rust
	$(CP) $(CURDIR)/third_party $(PKG_BUILD_DIR)/third_party
endef

define Build/Compile
	# Rust 守护进程：注入统一版本号（单一来源 VERSION）
	$(call MODEMD/Compile)
	# 厂商拨号器
	$(INSTALL_DIR) $(DL_OUT)
	$(if $(CONFIG_MODEM_PRO_QUECTEL),$(call cc_dialer,$(TP)/quectel-cm,$(QUECTEL_SRC),$(DL_OUT)/quectel-CM,-lpthread -ldl -lrt,))
	$(if $(CONFIG_MODEM_PRO_FIBOCOM),$(call cc_dialer,$(TP)/fibocom-dial/src,$(FIBOCOM_SRC),$(DL_OUT)/fibocom-dial,-lpthread -ldl,-DGHT_FEATURE_PCIE_AUTO))
	$(if $(CONFIG_MODEM_PRO_MEIG),$(call cc_dialer,$(TP)/meig-cm/src,$(MEIG_SRC),$(DL_OUT)/meig-cm,-lpthread -ldl,))
	$(if $(CONFIG_MODEM_PRO_SIMCOM),$(call cc_dialer,$(TP)/simcom-cm/src,$(SIMCOM_SRC),$(DL_OUT)/simcom-cm,-lpthread,))
endef

define Package/modem-pro/install
	# 守护进程
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/modemd $(1)/usr/bin/modemd

	# 厂商拨号器（按开关裁剪）
	$(INSTALL_DIR) $(1)/usr/lib/modemd
	$(if $(CONFIG_MODEM_PRO_QUECTEL),$(INSTALL_BIN) $(DL_OUT)/quectel-CM $(1)/usr/lib/modemd/quectel-CM)
	$(if $(CONFIG_MODEM_PRO_FIBOCOM),$(INSTALL_BIN) $(DL_OUT)/fibocom-dial $(1)/usr/lib/modemd/fibocom-dial)
	$(if $(CONFIG_MODEM_PRO_MEIG),$(INSTALL_BIN) $(DL_OUT)/meig-cm $(1)/usr/lib/modemd/meig-cm)
	$(if $(CONFIG_MODEM_PRO_SIMCOM),$(INSTALL_BIN) $(DL_OUT)/simcom-cm $(1)/usr/lib/modemd/simcom-cm)

	# netifd 协议
	$(INSTALL_DIR) $(1)/lib/netifd/proto
	$(INSTALL_BIN) $(CURDIR)/files/lib/netifd/proto/modem_pro.sh $(1)/lib/netifd/proto/modem_pro.sh
	$(INSTALL_BIN) $(CURDIR)/files/lib/netifd/proto/quectel.sh $(1)/lib/netifd/proto/quectel.sh
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
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/modemd
	$(INSTALL_DATA) $(CURDIR)/luci/app/js/view/modemd/overview.js $(1)/www/luci-static/resources/view/modemd/overview.js
	$(INSTALL_DATA) $(CURDIR)/luci/app/js/view/modemd/service.js $(1)/www/luci-static/resources/view/modemd/service.js

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
