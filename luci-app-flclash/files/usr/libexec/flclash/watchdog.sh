#!/bin/sh
# Independent DNS/core watchdog.
#
# The 5s reconciler lives inside flclash-api. If that process is OOM-killed or
# stopped while the core crashes, dnsmasq stays pointed at 127.0.0.1:1053 with
# no listener and the whole LAN loses DNS. This loop is deliberately outside
# the API process and only asks proxy-rules.sh to converge; it never starts the
# core by itself.

MISSES=0
while :; do
	state=$(cat /tmp/flclash-proxy.state 2>/dev/null || echo off)
	if [ "$state" != "off" ]; then
		bad=0
		addr=$(uci -q get flclash.main.external_controller 2>/dev/null)
		port=${addr##*:}
		case "$port" in
			''|*[!0-9]*) port=9097 ;;
		esac
		netstat -lnt 2>/dev/null | grep -q ":$port " || bad=1
		if [ "$(uci -q get flclash.main.core_type 2>/dev/null)" != "sing-box" ]; then
			netstat -lnu 2>/dev/null | grep -q ":1053 " || bad=1
		fi
		if [ "$bad" = "1" ]; then
			MISSES=$((MISSES + 1))
			if [ "$MISSES" -ge 2 ]; then
				logger -t flclash-watchdog "core/DNS listener missing; reconciling"
				/usr/libexec/flclash/proxy-rules.sh sync >/dev/null 2>&1
				MISSES=0
			fi
		else
			MISSES=0
		fi
	fi
	sleep 10
done
