#!/bin/sh
# /usr/libexec/flclash/proxy-rules.sh — LAN transparent proxy takeover.
#
# ShellClash-style rules (nft REDIRECT/TPROXY or iptables REDIRECT/TPROXY)
# combined with HomeProxy's DNS hand-off (a dnsmasq conf snippet pointing at
# the core's DNS listener). The router's own outbound traffic and the
# management plane (SSH/LuCI) are never touched: only prerouting on the LAN
# bridge is hooked.
#
# Firewall backend follows OpenClash's detection: `fw4` present -> nftables,
# otherwise iptables. Override with FIREWALL_BACKEND=nftables|iptables.
#
# Usage: proxy-rules.sh start | stop | status | backend
# State: /etc/config/flclash named section proxy (option enabled), so the
# takeover survives reboots via /etc/init.d/flclash-proxy.

NFT=/usr/sbin/nft
DNSCONF=/tmp/dnsmasq.d/flclash-dns.conf
TABLE=flclash
LAN_IF=${LAN_IF:-br-lan}
DNS_PORT=1053
REDIR_PORT=7893
TPROXY_PORT=7894
FWMARK=0x162

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
	uci set flclash.proxy.backend="$FIREWALL_BACKEND"
	uci commit flclash
}

# --- backend detection (OpenClash style: command -v fw4) -------------------
detect_backend() {
	if [ -n "$FIREWALL_BACKEND" ]; then
		case "$FIREWALL_BACKEND" in
			nftables|iptables) echo "$FIREWALL_BACKEND"; return ;;
		esac
	fi
	if command -v fw4 >/dev/null 2>&1; then
		echo nftables
	elif command -v iptables >/dev/null 2>&1; then
		echo iptables
	else
		echo nftables
	fi
}

# --- DNS hand-off (shared by both backends) --------------------------------
dns_on() {
	mkdir -p /tmp/dnsmasq.d
	printf 'no-poll\nno-resolv\nserver=127.0.0.1#%s\n' "$DNS_PORT" > "$DNSCONF"
	/etc/init.d/dnsmasq restart >/dev/null 2>&1
}

dns_off() {
	rm -f "$DNSCONF"
	/etc/init.d/dnsmasq restart >/dev/null 2>&1
}

# --- nftables backend ------------------------------------------------------
nft_on() {
	nft list table inet "$TABLE" >/dev/null 2>&1 && return 0

	ip rule add fwmark "$FWMARK" table 100 2>/dev/null
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
		iifname "$LAN_IF" ip daddr != @private_v4 meta l4proto udp udp dport 1-65535 meta mark set $FWMARK tproxy ip to 127.0.0.1:$TPROXY_PORT accept
	}
}
EOF
	[ $? -eq 0 ] || return 1
	return 0
}

nft_off() {
	nft delete table inet "$TABLE" 2>/dev/null
	ip rule del fwmark "$FWMARK" table 100 2>/dev/null
	ip route del local default dev lo table 100 2>/dev/null
	return 0
}

nft_active() {
	nft list table inet "$TABLE" >/dev/null 2>&1
}

# --- iptables backend ------------------------------------------------------
ipt_chain() {
	iptables -w -t nat -N FLCLASH 2>/dev/null
	iptables -w -t nat -A PREROUTING -i "$LAN_IF" -j FLCLASH 2>/dev/null
	iptables -w -t mangle -N FLCLASH_MANGLE 2>/dev/null
	iptables -w -t mangle -A PREROUTING -i "$LAN_IF" -j FLCLASH_MANGLE 2>/dev/null
	# DNS hijack (TCP/UDP 53) -> core DNS listener
	iptables -w -t nat -A FLCLASH -p udp --dport 53 -j REDIRECT --to-ports "$DNS_PORT" 2>/dev/null
	iptables -w -t nat -A FLCLASH -p tcp --dport 53 -j REDIRECT --to-ports "$DNS_PORT" 2>/dev/null
	# TCP non-LAN -> REDIR port; anti-loop via owner/cgroup is handled by the core
	for net in 10.0.0.0/8 100.64.0.0/10 127.0.0.0/8 169.254.0.0/16 172.16.0.0/12 192.0.0.0/24 192.168.0.0/16 224.0.0.0/4 240.0.0.0/4; do
		iptables -w -t nat -A FLCLASH -d "$net" -j RETURN 2>/dev/null
	done
	iptables -w -t nat -A FLCLASH -p tcp -j REDIRECT --to-ports "$REDIR_PORT" 2>/dev/null
	# UDP -> TPROXY with fwmark; TPROXY needs a local route table
	ip rule add fwmark "$FWMARK" table 100 2>/dev/null
	ip route add local default dev lo table 100 2>/dev/null
	for net in 10.0.0.0/8 100.64.0.0/10 127.0.0.0/8 169.254.0.0/16 172.16.0.0/12 192.0.0.0/24 192.168.0.0/16 224.0.0.0/4 240.0.0.0/4; do
		iptables -w -t mangle -A FLCLASH_MANGLE -d "$net" -j RETURN 2>/dev/null
	done
	iptables -w -t mangle -A FLCLASH_MANGLE -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "$FWMARK" 2>/dev/null
	iptables -w -t mangle -A FLCLASH_MANGLE -p udp -j MARK --set-mark "$FWMARK" 2>/dev/null
	return 0
}

ipt_off() {
	iptables -w -t nat -D PREROUTING -i "$LAN_IF" -j FLCLASH 2>/dev/null
	iptables -w -t nat -F FLCLASH 2>/dev/null
	iptables -w -t nat -X FLCLASH 2>/dev/null
	iptables -w -t mangle -D PREROUTING -i "$LAN_IF" -j FLCLASH_MANGLE 2>/dev/null
	iptables -w -t mangle -F FLCLASH_MANGLE 2>/dev/null
	iptables -w -t mangle -X FLCLASH_MANGLE 2>/dev/null
	ip rule del fwmark "$FWMARK" table 100 2>/dev/null
	ip route del local default dev lo table 100 2>/dev/null
	return 0
}

ipt_active() {
	iptables -w -t nat -n -L FLCLASH >/dev/null 2>&1
}

rules_on() {
	if [ "$FIREWALL_BACKEND" = "iptables" ]; then ipt_on; else nft_on; fi
}
rules_off() {
	if [ "$FIREWALL_BACKEND" = "iptables" ]; then ipt_off; else nft_off; fi
}
rules_active() {
	if [ "$FIREWALL_BACKEND" = "iptables" ]; then ipt_active; else nft_active; fi
}

FIREWALL_BACKEND=$(detect_backend)

case "$1" in
	backend)
		echo "$FIREWALL_BACKEND"
		;;
	start)
		[ "$(uci_enabled)" = "1" ] && rules_active && { echo "already active ($FIREWALL_BACKEND)"; exit 0; }
		# The core must be up first: the rules send LAN traffic to it.
		/etc/init.d/flclash-core start >/dev/null 2>&1
		sleep 2
		dns_on || { echo "dnsmasq hand-off failed"; exit 1; }
		if rules_on; then
			uci_set 1
			echo "transparent proxy: ON (backend=$FIREWALL_BACKEND tcp->$REDIR_PORT udp->TPROXY:$TPROXY_PORT dns->$DNS_PORT)"
			exit 0
		fi
		dns_off
		echo "firewall rules failed ($FIREWALL_BACKEND)"
		exit 1
		;;
	stop)
		rules_off
		uci_set 0
		echo "transparent proxy: OFF"
		;;
	status)
		if rules_active; then
			echo "ACTIVE ($FIREWALL_BACKEND)"
			exit 0
		fi
		echo "INACTIVE ($FIREWALL_BACKEND)"
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
		echo "usage: $0 start|stop|status|backend"
		exit 2
		;;
esac
