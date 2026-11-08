#!/bin/sh
# modem-pro —— netifd 侧最薄 shim。
#
# 原来这个脚本里的一切（生成带 proto_set_keep 的上报脚本、拉起 udhcpc/odhcp6c、
# 每 3 秒巡检、连续失败计数、孤儿清理、信号 trap）都已迁到 Rust：
#     /usr/bin/modemd wan-client
# 见 rust/src/wanclient/。
#
# 这里只保留 netifd 契约要求的东西：同一条路径、同一个启动方式、把进程树原样交接。
# 用 exec 而不是后台拉起，是为了让 modemd(wan-client) 直接顶替本 shim 的 PID ——
# netifd 的 proto_kill_command 按 PID 发 TERM、超时发 KILL，只有这样才能直达客户端，
# 不会留下占着网卡的孤儿 udhcpc/odhcp6c。
#
# 环境变量契约不变：INTERFACE / MP_IFACE / MP_UOPTS / MP_OOPTS / MP_NO_V6 / USERPREFIX /
# ZONE / EXTENDPREFIX 全部由 proto handler 通过 proto_export 注入，本脚本不解析、不改写。
# 新增的只有 MP_FIX_ACCEPT_RA（由 proto handler 判定是否要把网卡 accept_ra 置 2）。

set -u

if [ ! -x /usr/bin/modemd ]; then
	logger -t modem-pro "mp-client: /usr/bin/modemd 不存在或不可执行，无法拉起接口 ${INTERFACE:-?}"
	exit 1
fi

exec /usr/bin/modemd wan-client "$@"
