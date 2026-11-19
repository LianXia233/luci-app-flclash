#!/bin/sh
# FlClash transparent-proxy state machine for OpenWrt/ImmortalWrt.
#
# The UCI flag is the user's redirect-mode wish. The live state is converged
# from core liveness, tun.enable and that wish. TUN and REDIRECT never run
# together, a core failure always removes interception, and inbound management
# connections (SSH/LuCI) are exempted by connection direction, not just by IP.
#
# Usage: proxy-rules.sh start|stop|status|backend|prepare|sync|teardown|purge|boot

NFT=/usr/sbin/nft
DNSCONF=/tmp/dnsmasq.d/flclash-dns.conf
CFG=${CFG:-/etc/flclash/config.yaml}
TABLE=flclash
CORE_TABLE=mihomo
LAN_IF=${LAN_IF:-}
DNS_PORT=1053
REDIR_PORT=7893
TPROXY_PORT=7894
FWMARK=0x162
CORE_MARK=0x14d
STATE_FILE=/tmp/flclash-proxy.state
LOCAL_TAKEOVER=${LOCAL_TAKEOVER:-$(uci -q get flclash.proxy.local_takeover 2>/dev/null || echo 1)}
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

# --- LAN ACL (Nikki-style) ---------------------------------------------------
# Values are validated here so nothing user-supplied reaches nft/iptables
# unquoted or unchecked. acl_mode: all | allow | block.
acl_mode() {
	local mode
	mode=$(uci -q get flclash.proxy.acl_mode 2>/dev/null || echo all)
	case "$mode" in allow|block) echo "$mode" ;; *) echo all ;; esac
}

valid_ip_or_cidr() {
	echo "$1" | grep -Eq '^[0-9]{1,3}(\.[0-9]{1,3}){3}(/[0-9]{1,2})?$' || return 1
	local IFS='/' octets oct mask
	set -- $1
	octets=$1
	mask=$2
	if [ -n "$mask" ] && [ "$mask" -gt 32 ] 2>/dev/null; then return 1; fi
	IFS='.'
	for oct in $octets; do [ "$oct" -le 255 ] 2>/dev/null || return 1; done
	return 0
}

valid_mac() { echo "$1" | grep -Eq '^([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}$'; }
valid_port() { echo "$1" | grep -Eq '^[0-9]{1,5}$' && [ "$1" -ge 1 ] && [ "$1" -le 65535 ]; }

acl_ips() { uci -q get flclash.proxy.acl_ip 2>/dev/null | while read -r v; do valid_ip_or_cidr "$v" && echo "$v"; done; }
acl_macs() { uci -q get flclash.proxy.acl_mac 2>/dev/null | while read -r v; do valid_mac "$v" && echo "$v"; done; }
bypass_cidrs() { uci -q get flclash.proxy.bypass_cidr 2>/dev/null | while read -r v; do valid_ip_or_cidr "$v" && echo "$v"; done; }
bypass_ports() { uci -q get flclash.proxy.bypass_port 2>/dev/null | while read -r v; do valid_port "$v" && echo "$v"; done; }

set_state() {
	echo "$1" > "$STATE_FILE" 2>/dev/null
}

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

detect_lan_if() {
	local dev
	dev=$(uci -q get flclash.proxy.lan_if 2>/dev/null)
	if [ -z "$dev" ] && command -v ubus >/dev/null 2>&1 && command -v jsonfilter >/dev/null 2>&1; then
		dev=$(ubus -t 2 call network.interface.lan status 2>/dev/null | jsonfilter -e '@.l3_device' 2>/dev/null)
	fi
	[ -n "$dev" ] || dev=$(uci -q get network.lan.device 2>/dev/null)
	[ -n "$dev" ] || dev=br-lan
	case "$dev" in
		*[!A-Za-z0-9_.-]*) dev=br-lan ;;
	esac
	echo "$dev"
}

core_type() {
	[ "$(uci -q get flclash.main.core_type 2>/dev/null)" = "sing-box" ] && echo sing-box || echo mihomo
}

core_alive() {
	local port
	port=$(uci -q get flclash.main.external_controller 2>/dev/null | sed 's/.*://')
	[ -n "$port" ] || port=9097
	netstat -lnt 2>/dev/null | grep -q "127.0.0.1:$port "
}

core_proc() {
	ps w 2>/dev/null | grep -v grep | grep -qE "mihomo|sing-box"
}

wait_core() {
	local i=0
	while [ $i -lt 20 ]; do
		core_alive && return 0
		sleep 1
		i=$((i+1))
	done
	return 1
}

wait_core_if_starting() {
	core_alive && return 0
	core_proc || return 1
	wait_core
}

tun_enabled() {
	[ -f "$CFG" ] || return 1
	awk '/^tun:/{t=1; next} t && /^[^ \t#]/{t=0} t && /^[ \t]*enable:[[:space:]]*true/{found=1; exit} END{exit !found}' "$CFG"
}

tun_active() {
	nft list table inet "$CORE_TABLE" >/dev/null 2>&1 && return 0
	ip link show Meta >/dev/null 2>&1 && return 0
	ip link show tun0 >/dev/null 2>&1
}

wait_tun() {
	local i=0
	while [ $i -lt 15 ]; do
		tun_active && return 0
		sleep 1
		i=$((i+1))
	done
	return 1
}

# Prepare device-critical config before the core starts. The native FlClash IPC
# core only applies TUN during listener.start, so adding these keys after a
# controller reload is too late. Existing custom keys/lists are left intact.
prepare_config() {
	PREPARE_CHANGED=0
	[ -f "$CFG" ] || return 0
	local tmp="$CFG.flclash.tmp"

	if ! grep -q '^dns:' "$CFG"; then
		cat >> "$CFG" <<EOF

dns:
  enable: true
  listen: 0.0.0.0:$DNS_PORT
  enhanced-mode: redir-host
  default-nameserver:
    - 223.5.5.5
    - 119.29.29.29
  nameserver:
    - 223.5.5.5
    - 119.29.29.29
EOF
		PREPARE_CHANGED=1
	elif ! grep -qE '^[[:space:]]*listen:[[:space:]]*0\.0\.0\.0:'"$DNS_PORT" "$CFG"; then
		if awk '/^dns:/{d=1;next} d && /^[^ \t#]/{d=0} d && /^[ \t]*listen:/{found=1;exit} END{exit !found}' "$CFG"; then
			# Rewrite the existing listen: but KEEP ITS INDENTATION — dns
			# blocks written by other tools may use 4-space children, and a
			# hardcoded 2-space rewrite produces mixed indentation the YAML
			# parser rejects ("mapping values are not allowed").
			awk -v p="$DNS_PORT" '
				/^dns:/ {d=1; print; next}
				d && /^[^ \t#]/ {d=0}
				d && /^[ \t]*listen:/ {if (!seen) { match($0, /^[ \t]*/); print substr($0, 1, RLENGTH) "listen: 0.0.0.0:" p; seen=1 } else { print } ; next}
				{print}
			' "$CFG" > "$tmp"
		else
			# No listen key: insert one at the dns block's own child indent
			# (first indented child of the block), not at a hardcoded width.
			awk -v p="$DNS_PORT" '
				/^dns:/ {d=1; print; next}
				d && /^[^ \t#]/ {d=0}
				d && !ins && /^[ \t]+[^ \t]/ { match($0, /^[ \t]*/); print substr($0, 1, RLENGTH) "listen: 0.0.0.0:" p; ins=1 }
				{print}
				END { if (d && !ins) print "  listen: 0.0.0.0:" p }
			' "$CFG" > "$tmp"
		fi
		cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
	fi

	if grep -qE '^routing-mark:' "$CFG"; then
		if ! grep -qE '^routing-mark:[[:space:]]*333([[:space:]]|$)' "$CFG"; then
			awk '{if ($0 ~ /^routing-mark:/) print "routing-mark: 333"; else print}' "$CFG" > "$tmp" && \
				cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
		fi
	else
		if grep -q '^external-controller:' "$CFG"; then
			awk '{print; if ($0 ~ /^external-controller:/) print "routing-mark: 333"}' "$CFG" > "$tmp"
		else
			printf 'routing-mark: 333\n' > "$tmp"
			cat "$CFG" >> "$tmp"
		fi
		cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
	fi

	# A dedicated HTTP listener cannot share the mixed listener's port. The
	# mixed listener already provides HTTP and SOCKS, so disable the duplicate.
	local mixed_port http_port
	mixed_port=$(sed -n 's/^mixed-port:[[:space:]]*//p' "$CFG" | head -1)
	http_port=$(sed -n 's/^port:[[:space:]]*//p' "$CFG" | head -1)
	if [ -n "$mixed_port" ] && [ "$http_port" = "$mixed_port" ]; then
		awk '{if ($0 ~ /^port:/) print "port: 0"; else print}' "$CFG" > "$tmp" && \
			cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
	fi

	if tun_enabled && ! awk '/^tun:/{t=1;next} t && /^[^ \t#]/{t=0} t && /^[ \t]*route-exclude-address:/{found=1;exit} END{exit !found}' "$CFG"; then
		awk '
			function exclusions() {
				print "  route-exclude-address:"
				print "    - 10.0.0.0/8"
				print "    - 100.64.0.0/10"
				print "    - 127.0.0.0/8"
				print "    - 169.254.0.0/16"
				print "    - 172.16.0.0/12"
				print "    - 192.0.0.0/24"
				print "    - 192.168.0.0/16"
				print "    - 224.0.0.0/4"
				print "    - 240.0.0.0/4"
				print "    - ::1/128"
				print "    - fc00::/7"
				print "    - fe80::/10"
				print "    - ff00::/8"
			}
			/^tun:/ {t=1; print; next}
			t && /^[^ \t#]/ {exclusions(); t=0}
			{print}
			END {if (t) exclusions()}
		' "$CFG" > "$tmp" && cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
	fi

	# The prerouting REDIRECT delivers LAN packets to the INCOMING interface's
	# address (192.168.x.1:$REDIR_PORT), not to 127.0.0.1. With allow-lan false
	# mihomo binds the redir/tproxy listeners to 127.0.0.1 only, so every
	# redirected LAN connection is refused while the router's own traffic
	# (loopback-destined) keeps working — the classic "本机可代理、下挂设备不通"
	# failure. allow-lan true makes the listeners bind "*" so LAN takeover
	# actually connects; the WAN-side exposure is closed by fw4_input_on().
	if ! grep -qE '^allow-lan:[[:space:]]*true([[:space:]]|$)' "$CFG"; then
		if grep -qE '^allow-lan:' "$CFG"; then
			awk '{if ($0 ~ /^allow-lan:/) print "allow-lan: true"; else print}' "$CFG" > "$tmp" && \
				cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
		else
			printf 'allow-lan: true\n' > "$tmp" && cat "$CFG" >> "$tmp" && \
				cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
		fi
	fi

	# Client DNS queries are hijacked straight to the core's :$DNS_PORT
	# listener and never pass through dnsmasq, so a dnsmasq-side filter_aaaa
	# can never see them. Filter AAAA at the source instead: with
	# flclash.proxy.filter_aaaa=1 (the default) mihomo's DNS stops answering
	# AAAA, keeping dual-stack LAN clients on the v4 path the takeover covers.
	local want_dns_ipv6=true
	[ "$(uci -q get flclash.proxy.filter_aaaa 2>/dev/null)" != "0" ] && want_dns_ipv6=false
	local cur_dns_ipv6
	cur_dns_ipv6=$(awk '/^dns:/{d=1;next} d && /^[^ \t#]/{d=0} d && /^[ \t]*ipv6:/{print $2; exit}' "$CFG")
	if [ -n "$cur_dns_ipv6" ]; then
		if [ "$cur_dns_ipv6" != "$want_dns_ipv6" ]; then
			# Preserve the existing line's indentation (see the listen
			# insertion above — mixed indents break the YAML parser).
			awk -v w="$want_dns_ipv6" '
				/^dns:/ {d=1; print; next}
				d && /^[^ \t#]/ {d=0}
				d && /^[ \t]*ipv6:/ { match($0, /^[ \t]*/); print substr($0, 1, RLENGTH) "ipv6: " w; next}
				{print}
			' "$CFG" > "$tmp" && cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
		fi
	else
		# Insert at the dns block's own child indent.
		awk -v w="$want_dns_ipv6" '
			/^dns:/ {d=1; print; next}
			d && /^[^ \t#]/ {d=0}
			d && !ins && /^[ \t]+[^ \t]/ { match($0, /^[ \t]*/); print substr($0, 1, RLENGTH) "ipv6: " w; ins=1 }
			{print}
			END { if (d && !ins) print "  ipv6: " w }
		' "$CFG" > "$tmp" && cp "$tmp" "$CFG" && rm -f "$tmp" && PREPARE_CHANGED=1
	fi
	rm -f "$tmp"
	return 0
}

# --- DNS hand-off -----------------------------------------------------------

dnsmasq_restart_robust() {
	local i=0
	/etc/init.d/dnsmasq restart >/dev/null 2>&1
	while [ $i -lt 3 ]; do
		sleep 1
		netstat -lnu 2>/dev/null | grep -q ":53 " && return 0
		/etc/init.d/dnsmasq stop >/dev/null 2>&1
		sleep 1
		/etc/init.d/dnsmasq start >/dev/null 2>&1
		i=$((i+1))
	done
	netstat -lnu 2>/dev/null | grep -q ":53 "
}

dns_save() {
	[ "$(uci -q get flclash.proxy.dns_saved 2>/dev/null)" = "1" ] && return 0
	local cur noresolv filter
	cur=$(uci -q get dhcp.@dnsmasq[0].server 2>/dev/null)
	case " $cur " in *"127.0.0.1#$DNS_PORT"*) return 0 ;; esac
	noresolv=$(uci -q get dhcp.@dnsmasq[0].noresolv 2>/dev/null)
	filter=$(uci -q get dhcp.@dnsmasq[0].filter_aaaa 2>/dev/null)
	uci -q set flclash.proxy=proxy
	uci -q set flclash.proxy.dns_saved=1
	uci -q set flclash.proxy.saved_noresolv="${noresolv:-0}"
	uci -q set flclash.proxy.saved_filter_aaaa="${filter:-0}"
	uci -q delete flclash.proxy.saved_server 2>/dev/null
	for cur in $cur; do uci -q add_list "flclash.proxy.saved_server=$cur"; done
	uci -q commit flclash
}

dns_on() {
	[ "$(core_type)" = "sing-box" ] && { dns_off; return $?; }
	prepare_config
	if [ "$PREPARE_CHANGED" = "1" ]; then
		/etc/init.d/flclash-core restart >/dev/null 2>&1
		wait_core || return 1
	fi
	dns_save
	mkdir -p /tmp/dnsmasq.d
	local change=0 cur filter
	printf 'no-poll\nno-resolv\nserver=127.0.0.1#%s\n' "$DNS_PORT" > "$DNSCONF.new"
	if ! cmp -s "$DNSCONF.new" "$DNSCONF"; then
		mv "$DNSCONF.new" "$DNSCONF"
		change=1
	else
		rm -f "$DNSCONF.new"
	fi
	cur=$(uci -q get dhcp.@dnsmasq[0].server 2>/dev/null)
	if [ "$cur" != "127.0.0.1#$DNS_PORT" ]; then
		uci -q delete dhcp.@dnsmasq[0].server 2>/dev/null
		uci -q add_list "dhcp.@dnsmasq[0].server=127.0.0.1#$DNS_PORT"
		change=1
	fi
	if [ "$(uci -q get dhcp.@dnsmasq[0].noresolv)" != "1" ]; then
		uci -q set dhcp.@dnsmasq[0].noresolv=1 && change=1
	fi
	filter=$(uci -q get flclash.proxy.filter_aaaa 2>/dev/null)
	if [ "$filter" = "1" ] && [ "$(uci -q get dhcp.@dnsmasq[0].filter_aaaa)" != "1" ]; then
		uci -q set dhcp.@dnsmasq[0].filter_aaaa=1 && change=1
		uci -q set flclash.proxy.dns_filter_managed=1
		uci -q commit flclash
	elif [ "$filter" != "1" ] && [ "$(uci -q get flclash.proxy.dns_filter_managed)" = "1" ]; then
		uci -q delete dhcp.@dnsmasq[0].filter_aaaa 2>/dev/null && change=1
		uci -q delete flclash.proxy.dns_filter_managed 2>/dev/null
		uci -q commit flclash
	fi
	if [ "$change" = "1" ]; then
		uci -q commit dhcp 2>/dev/null
		dnsmasq_restart_robust
	fi
}

dns_off() {
	local change=0 cur saved saved_servers noresolv filter managed
	rm -f "$DNSCONF"
	cur=$(uci -q get dhcp.@dnsmasq[0].server 2>/dev/null)
	saved=$(uci -q get flclash.proxy.dns_saved 2>/dev/null)
	managed=$(uci -q get flclash.proxy.dns_filter_managed 2>/dev/null)
	if [ "$saved" = "1" ]; then
		[ -n "$cur" ] && { uci -q delete dhcp.@dnsmasq[0].server && change=1; }
		saved_servers=$(uci -q get flclash.proxy.saved_server 2>/dev/null)
		for cur in $saved_servers; do uci -q add_list "dhcp.@dnsmasq[0].server=$cur" && change=1; done
		noresolv=$(uci -q get flclash.proxy.saved_noresolv 2>/dev/null)
		[ "$(uci -q get dhcp.@dnsmasq[0].noresolv)" != "${noresolv:-0}" ] && {
			uci -q set dhcp.@dnsmasq[0].noresolv="${noresolv:-0}" && change=1
		}
		filter=$(uci -q get flclash.proxy.saved_filter_aaaa 2>/dev/null)
		uci -q delete dhcp.@dnsmasq[0].filter_aaaa 2>/dev/null && change=1
		[ "$filter" = "1" ] && { uci -q set dhcp.@dnsmasq[0].filter_aaaa=1 && change=1; }
		for key in dns_saved saved_noresolv saved_filter_aaaa saved_server dns_filter_managed; do
			uci -q delete "flclash.proxy.$key" 2>/dev/null
		done
		uci -q commit flclash
	else
		case " $cur " in
			*"127.0.0.1#$DNS_PORT"*)
				uci -q del_list "dhcp.@dnsmasq[0].server=127.0.0.1#$DNS_PORT" 2>/dev/null && change=1
				[ -n "$(uci -q get dhcp.@dnsmasq[0].server 2>/dev/null)" ] || uci -q delete dhcp.@dnsmasq[0].server 2>/dev/null
				[ "$(uci -q get dhcp.@dnsmasq[0].noresolv)" != "0" ] && {
					uci -q set dhcp.@dnsmasq[0].noresolv=0 && change=1
				}
				;;
		esac
		if [ "$managed" = "1" ]; then
			uci -q delete dhcp.@dnsmasq[0].filter_aaaa 2>/dev/null && change=1
			uci -q delete flclash.proxy.dns_filter_managed 2>/dev/null
			uci -q commit flclash
		fi
	fi
	if [ "$change" = "1" ]; then
		uci -q commit dhcp 2>/dev/null
		dnsmasq_restart_robust
	fi
	return 0
}

# --- leftovers --------------------------------------------------------------

purge_core_artifacts() {
	nft delete table inet "$CORE_TABLE" 2>/dev/null
	local n=0 prio
	while [ $n -lt 16 ]; do
		prio=$(ip rule 2>/dev/null | awk '$1 ~ /^90(0[0-9]|10)$/ {print $1; exit}')
		[ -n "$prio" ] || break
		ip rule del priority "$prio" 2>/dev/null || break
		n=$((n+1))
	done
	ip route flush table 2022 2>/dev/null
	ip link del Meta 2>/dev/null
	return 0
}

# --- nftables ---------------------------------------------------------------

fw4_input_on() {
	nft list chain inet fw4 input >/dev/null 2>&1 || return 0
	# allow-lan binds the proxy listeners to "*" — keep them LAN-only so an
	# open WAN zone cannot turn the router into an open proxy. Inserted first,
	# ahead of the accept rules below.
	nft list chain inet fw4 input 2>/dev/null | grep -q 'FlClash WAN Proxy Guard' || \
		nft insert rule inet fw4 input position 0 "iifname != \"$LAN_IF\" meta l4proto { tcp, udp } th dport { $REDIR_PORT, $TPROXY_PORT } reject comment \"FlClash WAN Proxy Guard\"" 2>/dev/null
	nft list chain inet fw4 input 2>/dev/null | grep -q 'FlClash Redirect Accept' || \
		nft insert rule inet fw4 input position 0 ct status dnat accept comment '"FlClash Redirect Accept"' 2>/dev/null
	nft list chain inet fw4 input 2>/dev/null | grep -q 'FlClash TPROXY Accept' || \
		nft insert rule inet fw4 input position 0 meta mark "$FWMARK" accept comment '"FlClash TPROXY Accept"' 2>/dev/null
}

fw4_input_off() {
	local handles h
	handles=$(nft -a list chain inet fw4 input 2>/dev/null | awk '/FlClash (Redirect|TPROXY) Accept|FlClash WAN Proxy Guard/ {print $NF}')
	for h in $handles; do nft delete rule inet fw4 input handle "$h" 2>/dev/null; done
}

nft_lan_ready() {
	nft list chain inet "$TABLE" prerouting_tcp 2>/dev/null | grep -q 'iifname "'"$LAN_IF"'"' || return 1
	nft list chain inet "$TABLE" prerouting_tcp 2>/dev/null | grep -q 'redirect to :'"$REDIR_PORT" || return 1
	nft list chain inet "$TABLE" prerouting_udp 2>/dev/null | grep -q 'iifname "'"$LAN_IF"'"' || return 1
	nft list chain inet "$TABLE" prerouting_udp 2>/dev/null | grep -q 'tproxy ip to 127.0.0.1:'"$TPROXY_PORT" || return 1
	[ "$(core_type)" = "sing-box" ] && return 0
	nft list chain inet "$TABLE" prerouting_dns 2>/dev/null | grep -q 'redirect to :'"$DNS_PORT"
}

nft_output_local() {
	nft list table inet "$TABLE" >/dev/null 2>&1 || "$NFT" -f - <<EOF
table inet $TABLE {
	set private_v4 { type ipv4_addr; flags interval; elements = { $PRIVATE } }
	chain output_tcp { type nat hook output priority dstnat - 10; policy accept; }
}
EOF
	nft add chain inet "$TABLE" output_tcp '{ type nat hook output priority dstnat - 10; policy accept; }' 2>/dev/null
	nft flush chain inet "$TABLE" output_tcp 2>/dev/null
	nft add rule inet "$TABLE" output_tcp "meta mark $CORE_MARK accept" 2>/dev/null
	# Every reply to an inbound connection bypasses REDIRECT, including SSH/LuCI
	# sessions to a public WAN address. This is stronger than an IP-only guard.
	nft add rule inet "$TABLE" output_tcp 'ct direction reply accept' 2>/dev/null
	nft add rule inet "$TABLE" output_tcp 'ip daddr @private_v4 accept' 2>/dev/null
	nft add rule inet "$TABLE" output_tcp 'ct original ip daddr @private_v4 accept' 2>/dev/null
	nft add rule inet "$TABLE" output_tcp 'tcp dport 53 accept' 2>/dev/null
	nft add rule inet "$TABLE" output_tcp "tcp dport $REDIR_PORT accept" 2>/dev/null
	nft add rule inet "$TABLE" output_tcp "tcp dport $TPROXY_PORT accept" 2>/dev/null
	nft add rule inet "$TABLE" output_tcp "meta l4proto tcp redirect to :$REDIR_PORT" 2>/dev/null
}

# Emits nft bypass rules for the prerouting chains.
#   block mode: listed clients / CIDRs / ports accept (never intercepted);
#   allow mode: the chain's catch-all redirect is gated on the client list,
#               so unlisted clients fall through to the default accept.
# `nft_acl_rules <tcp|udp>` prints the bypass lines for that chain.
nft_acl_rules() {
	local proto="$1" mode ip mac cidr port
	mode=$(acl_mode)
	if [ "$mode" = "block" ]; then
		for ip in $(acl_ips); do echo "ip saddr $ip accept"; done
		for mac in $(acl_macs); do echo "ether saddr $mac accept"; done
	fi
	for cidr in $(bypass_cidrs); do echo "ip daddr $cidr accept"; done
	for port in $(bypass_ports); do echo "$proto dport $port accept"; done
}

# Emits the catch-all interception rules, one per allowed client in allow
# mode (nft cannot OR ip saddr with ether saddr in a single match).
# `nft_intercept_rules <tcp|udp>` prints full rule lines.
nft_intercept_rules() {
	local proto="$1" mode ip mac tail
	if [ "$proto" = "tcp" ]; then
		tail="ip daddr != @private_v4 tcp dport 1-65535 redirect to :$REDIR_PORT"
	else
		tail="ip daddr != @private_v4 meta l4proto udp udp dport 1-65535 meta mark set $FWMARK tproxy ip to 127.0.0.1:$TPROXY_PORT accept"
	fi
	mode=$(acl_mode)
	if [ "$mode" != "allow" ]; then
		echo "iifname \"$LAN_IF\" $tail"
		return
	fi
	local listed=0
	for ip in $(acl_ips); do echo "iifname \"$LAN_IF\" ip saddr $ip $tail"; listed=1; done
	for mac in $(acl_macs); do echo "iifname \"$LAN_IF\" ether saddr $mac $tail"; listed=1; done
	if [ "$listed" = "0" ]; then
		logger -t flclash-proxy "acl_mode=allow but client list is empty; falling back to all"
		echo "iifname \"$LAN_IF\" $tail"
	fi
}

nft_on() {
	if ! nft_lan_ready; then
		nft delete table inet "$TABLE" 2>/dev/null
		ip rule add fwmark "$FWMARK" table 100 2>/dev/null
		ip route add local default dev lo table 100 2>/dev/null
		local dns_hijack=""
		[ "$(core_type)" = "sing-box" ] || dns_hijack="iifname \"$LAN_IF\" udp dport 53 redirect to :$DNS_PORT
		iifname \"$LAN_IF\" tcp dport 53 redirect to :$DNS_PORT"
		"$NFT" -f - <<EOF
table inet $TABLE {
	set private_v4 { type ipv4_addr; flags interval; elements = { $PRIVATE } }
	chain prerouting_dns {
		type nat hook prerouting priority dstnat - 10; policy accept;
		$dns_hijack
	}
	chain prerouting_tcp {
		type nat hook prerouting priority dstnat + 10; policy accept;
		$(nft_acl_rules tcp)
		$(nft_intercept_rules tcp)
	}
	chain prerouting_udp {
		type filter hook prerouting priority mangle - 10; policy accept;
		$(nft_acl_rules udp)
		$(nft_intercept_rules udp)
	}
}
EOF
		[ $? -eq 0 ] || return 1
	fi
	fw4_input_on
	[ "$LOCAL_TAKEOVER" = "1" ] && [ "$(core_type)" != "sing-box" ] && nft_output_local
	nft_lan_ready
}

nft_off() {
	nft delete table inet "$TABLE" 2>/dev/null
	ip rule del fwmark "$FWMARK" table 100 2>/dev/null
	ip route del local default dev lo table 100 2>/dev/null
	fw4_input_off
	return 0
}

nft_active() { nft_lan_ready; }

# --- iptables ---------------------------------------------------------------

ipt_output_local() {
	iptables -w -t nat -N FLCLASH_OUTPUT 2>/dev/null
	iptables -w -t nat -C OUTPUT -j FLCLASH_OUTPUT 2>/dev/null || iptables -w -t nat -A OUTPUT -j FLCLASH_OUTPUT
	iptables -w -t nat -F FLCLASH_OUTPUT
	iptables -w -t nat -A FLCLASH_OUTPUT -m mark --mark "$CORE_MARK" -j RETURN
	iptables -w -t nat -A FLCLASH_OUTPUT -m conntrack --ctdir REPLY -j RETURN
	for net in 10.0.0.0/8 100.64.0.0/10 127.0.0.0/8 169.254.0.0/16 172.16.0.0/12 192.0.0.0/24 192.168.0.0/16 224.0.0.0/4 240.0.0.0/4; do
		iptables -w -t nat -A FLCLASH_OUTPUT -d "$net" -j RETURN
	done
	iptables -w -t nat -A FLCLASH_OUTPUT -p tcp --dport 53 -j RETURN
	iptables -w -t nat -A FLCLASH_OUTPUT -p tcp --dport "$REDIR_PORT" -j RETURN
	iptables -w -t nat -A FLCLASH_OUTPUT -p tcp --dport "$TPROXY_PORT" -j RETURN
	iptables -w -t nat -A FLCLASH_OUTPUT -p tcp -j REDIRECT --to-ports "$REDIR_PORT"
}

# iptables bypass rules for both FLCLASH (nat) and FLCLASH_MANGLE (udp).
ipt_acl_bypass() {
	local mode ip mac cidr port chain
	mode=$(acl_mode)
	for chain in FLCLASH FLCLASH_MANGLE; do
		if [ "$mode" = "block" ]; then
			for ip in $(acl_ips); do
				local tbl=nat; [ "$chain" = FLCLASH_MANGLE ] && tbl=mangle
				iptables -w -t "$tbl" -A "$chain" -s "$ip" -j RETURN
			done
			for mac in $(acl_macs); do
				local tbl=nat; [ "$chain" = FLCLASH_MANGLE ] && tbl=mangle
				iptables -w -t "$tbl" -A "$chain" -m mac --mac-source "$mac" -j RETURN
			done
		fi
	done
	local tbl
	for cidr in $(bypass_cidrs); do
		iptables -w -t nat -A FLCLASH -d "$cidr" -j RETURN
		iptables -w -t mangle -A FLCLASH_MANGLE -d "$cidr" -j RETURN
	done
	for port in $(bypass_ports); do
		iptables -w -t nat -A FLCLASH -p tcp --dport "$port" -j RETURN
		iptables -w -t mangle -A FLCLASH_MANGLE -p udp --dport "$port" -j RETURN
	done
}

# Final catch-all rules; in allow mode they are per-client instead of global.
ipt_intercept() {
	local mode ip mac listed=0
	mode=$(acl_mode)
	if [ "$mode" != "allow" ]; then
		iptables -w -t nat -A FLCLASH -p tcp -j REDIRECT --to-ports "$REDIR_PORT"
		iptables -w -t mangle -A FLCLASH_MANGLE -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "$FWMARK"
		return
	fi
	for ip in $(acl_ips); do
		iptables -w -t nat -A FLCLASH -s "$ip" -p tcp -j REDIRECT --to-ports "$REDIR_PORT"
		iptables -w -t mangle -A FLCLASH_MANGLE -s "$ip" -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "$FWMARK"
		listed=1
	done
	for mac in $(acl_macs); do
		iptables -w -t nat -A FLCLASH -m mac --mac-source "$mac" -p tcp -j REDIRECT --to-ports "$REDIR_PORT"
		iptables -w -t mangle -A FLCLASH_MANGLE -m mac --mac-source "$mac" -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "$FWMARK"
		listed=1
	done
	if [ "$listed" = "0" ]; then
		logger -t flclash-proxy "acl_mode=allow but client list is empty; falling back to all"
		iptables -w -t nat -A FLCLASH -p tcp -j REDIRECT --to-ports "$REDIR_PORT"
		iptables -w -t mangle -A FLCLASH_MANGLE -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "$FWMARK"
	fi
}

ipt_chain() {
	ipt_off
	iptables -w -t nat -N FLCLASH
	iptables -w -t nat -A PREROUTING -i "$LAN_IF" -j FLCLASH
	iptables -w -t mangle -N FLCLASH_MANGLE
	iptables -w -t mangle -A PREROUTING -i "$LAN_IF" -j FLCLASH_MANGLE
	[ "$(core_type)" = "sing-box" ] || {
		iptables -w -t nat -A FLCLASH -p udp --dport 53 -j REDIRECT --to-ports "$DNS_PORT"
		iptables -w -t nat -A FLCLASH -p tcp --dport 53 -j REDIRECT --to-ports "$DNS_PORT"
	}
	for net in 10.0.0.0/8 100.64.0.0/10 127.0.0.0/8 169.254.0.0/16 172.16.0.0/12 192.0.0.0/24 192.168.0.0/16 224.0.0.0/4 240.0.0.0/4; do
		iptables -w -t nat -A FLCLASH -d "$net" -j RETURN
		iptables -w -t mangle -A FLCLASH_MANGLE -d "$net" -j RETURN
	done
	ipt_acl_bypass
	ip rule add fwmark "$FWMARK" table 100 2>/dev/null
	ip route add local default dev lo table 100 2>/dev/null
	ipt_intercept
	iptables -w -t filter -C INPUT -m mark --mark "$FWMARK" -j ACCEPT 2>/dev/null || \
		iptables -w -t filter -I INPUT 1 -m mark --mark "$FWMARK" -j ACCEPT
	[ "$LOCAL_TAKEOVER" = "1" ] && [ "$(core_type)" != "sing-box" ] && ipt_output_local
}

ipt_off() {
	iptables -w -t nat -D PREROUTING -i "$LAN_IF" -j FLCLASH 2>/dev/null
	iptables -w -t nat -F FLCLASH 2>/dev/null
	iptables -w -t nat -X FLCLASH 2>/dev/null
	iptables -w -t mangle -D PREROUTING -i "$LAN_IF" -j FLCLASH_MANGLE 2>/dev/null
	iptables -w -t mangle -F FLCLASH_MANGLE 2>/dev/null
	iptables -w -t mangle -X FLCLASH_MANGLE 2>/dev/null
	iptables -w -t nat -D OUTPUT -j FLCLASH_OUTPUT 2>/dev/null
	iptables -w -t nat -F FLCLASH_OUTPUT 2>/dev/null
	iptables -w -t nat -X FLCLASH_OUTPUT 2>/dev/null
	iptables -w -t filter -D INPUT -m mark --mark "$FWMARK" -j ACCEPT 2>/dev/null
	ip rule del fwmark "$FWMARK" table 100 2>/dev/null
	ip route del local default dev lo table 100 2>/dev/null
	return 0
}

ipt_active() {
	iptables -w -t nat -C FLCLASH -p tcp -j REDIRECT --to-ports "$REDIR_PORT" 2>/dev/null && \
		iptables -w -t mangle -C FLCLASH_MANGLE -p udp -j TPROXY --on-port "$TPROXY_PORT" --tproxy-mark "$FWMARK" 2>/dev/null
}

ipt_on() {
	ipt_active || ipt_chain || return 1
	ipt_active
}

# --- backend dispatch -------------------------------------------------------

rules_on() { if [ "$FIREWALL_BACKEND" = "iptables" ]; then ipt_on; else nft_on; fi; }
rules_off() { if [ "$FIREWALL_BACKEND" = "iptables" ]; then ipt_off; else nft_off; fi; }
rules_active() { if [ "$FIREWALL_BACKEND" = "iptables" ]; then ipt_active; else nft_active; fi; }
output_local_on() {
	[ "$LOCAL_TAKEOVER" = "1" ] && [ "$(core_type)" != "sing-box" ] || return 0
	if [ "$FIREWALL_BACKEND" = "iptables" ]; then ipt_output_local; else nft_output_local; fi
}

offload_guard() {
	local changed=0 opt
	OFFLOAD_CHANGED=0
	for opt in flow_offloading flow_offloading_hw; do
		if [ "$(uci -q get firewall.@defaults[0].$opt)" != "0" ]; then
			uci -q set firewall.@defaults[0].$opt='0' && changed=1
		fi
	done
	if [ "$changed" = "1" ]; then
		OFFLOAD_CHANGED=1
		uci -q commit firewall
		/etc/init.d/firewall restart >/dev/null 2>&1
		command -v conntrack >/dev/null && conntrack -F >/dev/null 2>&1
		logger -t flclash-proxy "flow offloading disabled while transparent proxy is active"
	fi
	if [ -x /etc/init.d/bridger ] && /etc/init.d/bridger enabled 2>/dev/null; then
		/etc/init.d/bridger stop 2>/dev/null
		/etc/init.d/bridger disable 2>/dev/null
	fi
	return 0
}

# --- convergence ------------------------------------------------------------

teardown() {
	rules_off
	dns_off
	set_state off
	return 0
}

redirect_on() {
	offload_guard
	dns_on || { set_state off; return 1; }
	if rules_on; then
		set_state on
		return 0
	fi
	dns_off
	set_state off
	return 1
}

sync() {
	local tun=0 core=0 wish state
	tun_enabled && tun=1
	core_alive && core=1
	wish=$(uci_enabled)

	if [ "$core" = "0" ]; then
		# Do not purge while the IPC process is still initializing its controller.
		core_proc && return 0
		rules_off
		dns_off
		purge_core_artifacts
		set_state off
		return 0
	fi

	if [ "$tun" = "1" ]; then
		if ! tun_active && ! wait_tun; then
			# A controller reload in the native IPC core does not start TUN. Keep
			# REDIRECT as a working fallback until the next core restart/sync.
			logger -t flclash-proxy "tun.enable is true but no TUN device is active; keeping REDIRECT fallback"
			if [ "$wish" = "1" ]; then redirect_on; else teardown; fi
			return 1
		fi
		offload_guard
		if [ "$OFFLOAD_CHANGED" = "1" ]; then
			# fw4 restart also removes mihomo's input accepts. Restart the core
			# once so listener.start recreates them before enabling TUN traffic.
			/etc/init.d/flclash-core restart >/dev/null 2>&1
			wait_core || { set_state off; return 1; }
			wait_tun || { set_state off; return 1; }
		fi
		rules_off
		# Keep dnsmasq -> core DNS in TUN mode too. This preserves fake-ip and
		# domain rules; TUN's any:53 hijack and this loopback hand-off do not loop.
		dns_on || { set_state off; return 1; }
		output_local_on
		set_state tun
		return 0
	fi

	purge_core_artifacts
	if [ "$wish" = "1" ]; then
		wait_core_if_starting || true
		redirect_on && uci_set 1
		return $?
	fi
	teardown
}

FIREWALL_BACKEND=$(detect_backend)
LAN_IF=$(detect_lan_if)

# API maintenance, core hooks and user actions may run concurrently. Serialize
# state-changing operations so one sync cannot delete a table another is adding.
case "$1" in
	start|stop|teardown|sync|boot|reapply)
		LOCKDIR=/tmp/flclash-proxy.lock
		if ! mkdir "$LOCKDIR" 2>/dev/null; then
			oldpid=$(cat "$LOCKDIR/pid" 2>/dev/null)
			if [ -n "$oldpid" ] && ! kill -0 "$oldpid" 2>/dev/null; then
				rm -rf "$LOCKDIR"
				mkdir "$LOCKDIR" 2>/dev/null || exit 0
			else
				exit 0
			fi
		fi
		echo $$ > "$LOCKDIR/pid"
		trap 'rm -rf "$LOCKDIR"' EXIT INT TERM
		;;
esac

case "$1" in
	backend)
		echo "$FIREWALL_BACKEND"
		;;
	lan)
		echo "$LAN_IF"
		;;
	prepare)
		prepare_config
		;;
	start)
		uci_set 1
		/etc/init.d/flclash-core start >/dev/null 2>&1
		wait_core_if_starting || true
		sync
		rc=$?
		state=$(cat "$STATE_FILE" 2>/dev/null || echo off)
		echo "transparent proxy: $state ($FIREWALL_BACKEND, lan=$LAN_IF)"
		[ "$rc" = "0" ] && [ "$state" != "off" ]
		;;
	stop)
		uci_set 0
		teardown
		echo "transparent proxy: off ($FIREWALL_BACKEND)"
		;;
	teardown)
		teardown
		echo "transparent proxy: torn down (uci wish preserved)"
		;;
	purge)
		purge_core_artifacts
		;;
	reapply)
		# Rebuild interception rules after an ACL change. Only meaningful
		# while REDIRECT/TPROXY is live; TUN intercepts at L3 (no LAN ACL)
		# and "off" has nothing to rebuild.
		state=$(cat "$STATE_FILE" 2>/dev/null || echo off)
		case "$state" in
			on)
				rules_off
				redirect_on
				rc=$?
				echo "transparent proxy: $(cat "$STATE_FILE" 2>/dev/null) (rules rebuilt)"
				exit "$rc"
				;;
			tun)
				echo "transparent proxy: tun (LAN ACL not applicable in TUN mode)"
				exit 0
				;;
			*)
				echo "transparent proxy: off (nothing to rebuild)"
				exit 0
				;;
		esac
		;;
	sync)
		wait_core_if_starting || true
		sync
		rc=$?
		state=$(cat "$STATE_FILE" 2>/dev/null || echo off)
		echo "transparent proxy: $state ($FIREWALL_BACKEND, lan=$LAN_IF)"
		exit "$rc"
		;;
	status)
		state=$(cat "$STATE_FILE" 2>/dev/null || echo off)
		if tun_enabled && core_alive && tun_active; then
			echo "ACTIVE-TUN ($state)"
			exit 0
		fi
		if rules_active; then
			echo "ACTIVE ($FIREWALL_BACKEND)"
			exit 0
		fi
		echo "INACTIVE ($FIREWALL_BACKEND)"
		exit 1
		;;
	boot)
		wait_core_if_starting || true
		sync
		;;
	*)
		echo "usage: $0 start|stop|status|backend|prepare|sync|teardown|purge|boot|reapply"
		exit 2
		;;
esac
