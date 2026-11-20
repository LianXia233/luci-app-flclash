#!/bin/sh
# FlClash 全栈停止：后端(flclash-api/WebUI) + 内核(core) + 透明代理，一并清除代理状态。
# 由 LuCI「停止服务」按钮经 rpcd flclash.service_toggle 调用。
# 幂等：重复执行安全；guard 防止并发/递归触发。
#
# 注意：停止后 UCI 代理开关被清零（flclash.proxy.enabled=0），下一次启动不会
# 自动恢复透明代理——这是「清除代理、不残留」的预期行为；如需代理请在 WebUI
# 中重新开启，或在启动服务后手动启用透明代理。

GUARD=/tmp/flclash-stop-all.guard
RULES=/usr/libexec/flclash/proxy-rules.sh

[ -f "$GUARD" ] && exit 0
touch "$GUARD" 2>/dev/null
trap 'rm -f "$GUARD"' EXIT HUP INT TERM

logger -t flclash-stopall "stopping full stack (backend + core + proxy)"

# 1) 停三个 procd 服务：内核、透明代理服务、后端(其退出即关闭 9098 WebUI)。
#    core 的 stop_service 会延时 sync；由于稍后 wish 清零且 core 已停，那次
#    sync 只会再次清理，不会重建任何代理规则。
/etc/init.d/flclash-core  stop >/dev/null 2>&1
/etc/init.d/flclash-proxy stop >/dev/null 2>&1
/etc/init.d/flclash-api   stop >/dev/null 2>&1

# 2) 彻底清除代理：
#    stop  = uci_set 0 + teardown -> 删 nft/iptables 拦截规则、fw4 input 规则、DNS 劫持
#    purge = 删 mihomo nft table、策略路由(90x/2022)、Meta 网卡等内核残留
if [ -x "$RULES" ]; then
	"$RULES" stop  >/dev/null 2>&1
	"$RULES" purge >/dev/null 2>&1
fi

# 3) 兜底清理顽固/孤儿进程，确保后端、内核、WebUI 全部退出。
killall -q flclash-api flclash-core-bridge mihomo sing-box 2>/dev/null

# 4) 状态归零，让 LuCI/WebUI 显示「已停止」。
echo off > /tmp/flclash-proxy.state 2>/dev/null

logger -t flclash-stopall "full stack stopped; proxy state cleared"
exit 0
