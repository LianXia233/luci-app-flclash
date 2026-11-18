#!/bin/sh
# /usr/libexec/flclash/proxy-rules.sh — LAN transparent proxy takeover.
#
# ShellClash-style rules (nft REDIRECT for TCP, TPROXY for UDP) combined with
# HomeProxy's DNS hand-off (a dnsmasq conf snippet pointing at the core's DNS
# listener). The router's own outbound traffic and the management plane
# (SSH/LuCI) are never touched: only prerouting on the LAN bridge is hooked.
#
# Usage: proxy-rules.sh start | stop | status
# State: /etc/config/flclash named section proxy (option enabled), so the
# takeover survives reboots via /etc/init.d/flclash-proxy.

NFT=/usr/sbin/nft
DNSCONF=/tmp/dnsmasq.d/flclash-dns.conf
TABLE=flclash
LAN_IF=${LAN_IF:-br-lan}
DNS_PORT=1053
REDIR_PORT=7893
TPROXY_PORT=7894

# RFC1918 + loopback + CGN + link-local + multicast + reserved. fake-ip
# (198.18.0.0/15) is deliberately NOT excluded: traffic to those addresses is
# exactly what must enter the core.
PRIVATE="10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.0.0.0/24, 192.168.0.0/16, 224.0.0.0/4, 240.0.0.0/4"

uci_enabled() {
	uci -q get flclash.proxy.enabled 2>/dev/null || echo 0
}

uci_set() {
	uci -q set flclash.proxy=proxy 2>/dev/null || true
	uci set flclash.proxy.enabled="$1"
	uci commit flclash
}

dns_on() {
	mkdir -p /tmp/dnsmasq.d
	printf 'no-poll\nno-resolv\nserver=127.0.0.1#%s\n' "$DNS_PORT" > "$DNSCONF"
	/etc/init.d/dnsmasq restart >/dev/null 2>&1
}

dns_off() {
	rm -f "$DNSCONF"
	/etc/init.d/dnsmasq restart >/dev/null 2>&1
}

rules_on() {
	nft list table inet "$TABLE" >/dev/null 2>&1 && return 0

	# TPROXY routing: packets marked 1 must be delivered locally.
	ip rule add fwmark 1 table 100 2>/dev/null
	ip route add local default dev lo table 100 2>/dev/null

	"$NFT" -f - <<EOF
table inet $TABLE {
	set private_v4 {
		type ipv4_addr
		flags interval
		elements = { $PRIVATE }
	}

	chain prerouting_dns {
		type nat hook prerouting priority dstnat - 10; policy accept;
		iifname "$LAN_IF" udp dport 53 redirect to :$DNS_PORT
		iifname "$LAN_IF" tcp dport 53 redirect to :$DNS_PORT
	}

	chain prerouting_tcp {
		type nat hook prerouting priority dstnat + 10; policy accept;
		iifname "$LAN_IF" ip daddr != @private_v4 tcp dport 1-65535 redirect to :$REDIR_PORT
	}

	chain prerouting_udp {
		type filter hook prerouting priority mangle - 10; policy accept;
		iifname "$LAN_IF" ip daddr != @private_v4 meta l4proto udp udp dport 1-65535 meta mark set 1 tproxy ip to 127.0.0.1:$TPROXY_PORT accept
	}
}
EOF
	[ $? -eq 0 ] || return 1
	return 0
}

rules_off() {
	nft delete table inet "$TABLE" 2>/dev/null
	ip rule del fwmark 1 table 100 2>/dev/null
	ip route del local default dev lo table 100 2>/dev/null
	dns_off
	return 0
}

rules_active() {
	nft list table inet "$TABLE" >/dev/null 2>&1
}

case "$1" in
	start)
		[ "$(uci_enabled)" = "1" ] && rules_active && { echo "already active"; exit 0; }
		# The core must be up first: the rules send LAN traffic to it.
		/etc/init.d/flclash-core start >/dev/null 2>&1
		sleep 2
		dns_on || { echo "dnsmasq hand-off failed"; exit 1; }
		if rules_on; then
			uci_set 1
			echo "transparent proxy: ON (tcp->$REDIR_PORT udp->TPROXY:$TPROXY_PORT dns->$DNS_PORT)"
			exit 0
		fi
		dns_off
		echo "nft rules failed"
		exit 1
		;;
	stop)
		rules_off
		uci_set 0
		echo "transparent proxy: OFF"
		;;
	status)
		if rules_active; then
			echo "ACTIVE"
			exit 0
		fi
		echo "INACTIVE"
		exit 1
		;;
	boot)
		# Called by /etc/init.d/flclash-proxy at boot: restore the takeover
		# only when the UCI flag says it was enabled before the reboot.
		[ "$(uci_enabled)" = "1" ] || exit 0
		sleep 5
		dns_on
		rules_on
		;;
	*)
		echo "usage: $0 start|stop|status|boot"
		exit 2
		;;
esac
