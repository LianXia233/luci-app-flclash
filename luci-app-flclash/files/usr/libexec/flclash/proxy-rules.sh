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
# The WebUI stores the config path in UCI; honour it instead of assuming the
# default (a user pointing config_file elsewhere used to get a state machine
# that parsed and rewrote the wrong file).
[ -n "${CFG:-}" ] || CFG=$(uci -q get flclash.main.config_file 2>/dev/null)
[ -n "${CFG:-}" ] || CFG=/etc/flclash/config.yaml
TABLE=flclash
CORE_TABLE=mihomo
FIREWALL_BACKEND=nftables
LAN_IF=${LAN_IF:-}
DNS_PORT=1053
REDIR_PORT=7893
TPROXY_PORT=7894
MIXED_PORT=$(uci -q get flclash.main.mixed_port 2>/dev/null)
case "$MIXED_PORT" in
	''|*[!0-9]*) MIXED_PORT=7892 ;;
esac
[ "$MIXED_PORT" -ge 1 ] 2>/dev/null && [ "$MIXED_PORT" -le 65535 ] 2>/dev/null || MIXED_PORT=7892
API_LISTEN=$(uci -q get flclash.api.listen 2>/dev/null)
API_PORT=${API_LISTEN##*:}
case "$API_PORT" in
	''|*[!0-9]*) API_PORT=9098 ;;
esac
[ "$API_PORT" -ge 1 ] 2>/dev/null && [ "$API_PORT" -le 65535 ] 2>/dev/null || API_PORT=9098
FWMARK=0x162
CORE_MARK=0x14d
STATE_FILE=/tmp/flclash-proxy.state
LAN_SIG=/tmp/flclash-lan.sig
FW4_INCLUDE_DIR=/var/run/flclash
FW4_INCLUDE=$FW4_INCLUDE_DIR/fw4-input.nft
LAN_IFS=""
LAN_IFS_NFT=""
LOCAL_TAKEOVER=${LOCAL_TAKEOVER:-$(uci -q get flclash.proxy.local_takeover 2>/dev/null || echo 1)}
PRIVATE="10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.0.0.0/24, 192.168.0.0/16, 224.0.0.0/4, 240.0.0.0/4"
PRIVATE_V6="::1/128, fc00::/7, fe80::/10, ff00::/8"

uci_enabled() {
	uci -q get flclash.proxy.enabled 2>/dev/null || echo 0
}

# Single source of truth for the live interception mode. The legacy
# `proxy.enabled` boolean and `tun.enabled` config key are only migration
# inputs; keeping them as independent switches made stop() unable to stop TUN.
proxy_mode() {
	local m
	m=$(uci -q get flclash.proxy.mode 2>/dev/null)
	case "$m" in
		off|redirect|tproxy|tun) echo "$m"; return 0 ;;
	esac
	if [ "$(uci -q get flclash.main.tun_enabled 2>/dev/null)" = "1" ] || tun_enabled; then
		echo tun
	elif [ "$(uci_enabled)" = "1" ]; then
		echo redirect
	else
		echo off
	fi
}

# off | proxy | direct. Legacy `filter_aaaa=1` maps to off (the old default
# filtered AAAA to avoid IPv4-only takeover leaks).
proxy_ipv6() {
	local v
	v=$(uci -q get flclash.proxy.ipv6 2>/dev/null)
	case "$v" in
		off|proxy|direct) echo "$v"; return 0 ;;
	esac
	if [ "$(uci -q get flclash.proxy.filter_aaaa 2>/dev/null)" = "1" ]; then
		echo off
	else
		echo proxy
	fi
}

uci_set() {
	local m="$1"
	case "$m" in
		off|redirect|tproxy|tun) ;;
		*) m=off ;;
	esac
	uci -q set flclash.proxy=proxy 2>/dev/null || true
	uci set flclash.proxy.mode="$m"
	if [ "$m" = "off" ]; then
		uci set flclash.proxy.enabled=0
	else
		uci set flclash.proxy.enabled=1
	fi
	uci set flclash.proxy.backend=nftables
	uci commit flclash
}

# --- LAN ACL (Nikki-style) ---------------------------------------------------
# Values are validated here so nothing user-supplied reaches nft
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
valid_ip6_or_cidr() {
	echo "$1" | grep -Eq '^[0-9A-Fa-f:]+(/[0-9]{1,3})?$' || return 1
	case "$1" in
		*/)
			return 1
			;;
		*/*)
			local mask=${1##*/}
			[ "$mask" -le 128 ] 2>/dev/null || return 1
			;;
	esac
	return 0
}
valid_port() { echo "$1" | grep -Eq '^[0-9]{1,5}$' && [ "$1" -ge 1 ] && [ "$1" -le 65535 ]; }

acl_ips() { uci -q get flclash.proxy.acl_ip 2>/dev/null | tr ' ' '\n' | while read -r v; do valid_ip_or_cidr "$v" && echo "$v"; done; }
acl_ips6() { uci -q get flclash.proxy.acl_ip6 2>/dev/null | tr ' ' '\n' | while read -r v; do valid_ip6_or_cidr "$v" && echo "$v"; done; }
acl_macs() { uci -q get flclash.proxy.acl_mac 2>/dev/null | tr ' ' '\n' | while read -r v; do valid_mac "$v" && echo "$v"; done; }
bypass_cidrs() { uci -q get flclash.proxy.bypass_cidr 2>/dev/null | tr ' ' '\n' | while read -r v; do valid_ip_or_cidr "$v" && echo "$v"; done; }
bypass_cidrs6() { uci -q get flclash.proxy.bypass_cidr6 2>/dev/null | tr ' ' '\n' | while read -r v; do valid_ip6_or_cidr "$v" && echo "$v"; done; }
bypass_ports() { uci -q get flclash.proxy.bypass_port 2>/dev/null | tr ' ' '\n' | while read -r v; do valid_port "$v" && echo "$v"; done; }

set_state() {
	echo "$1" > "$STATE_FILE" 2>/dev/null
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

valid_ifname() {
	case "$1" in
		''|*[!A-Za-z0-9_.-]*) return 1 ;;
	esac
	return 0
}

# All LAN-facing devices, not just br-lan: multi-bridge/guest/VLAN setups used
# to be rejected by the WAN guard (`iifname != "br-lan"`). The explicit UCI
# `lan_if` may contain a space-separated list; LAN* firewall zones are resolved
# through ubus. Returns a space-separated device list.
detect_lan_ifs() {
	local devs="" dev net sec
	add_dev() {
		valid_ifname "$1" || return 0
		case " $devs " in
			*" $1 "*) return 0 ;;
		esac
		devs="$devs $1"
	}
	for dev in $(uci -q get flclash.proxy.lan_if 2>/dev/null); do
		add_dev "$dev"
	done
	for sec in $(uci -q show firewall 2>/dev/null | sed -n "s/^firewall\.\([^.=]*\)=zone$/\1/p"); do
		for net in $(uci -q get "firewall.$sec.network" 2>/dev/null); do
			case "$net" in
				lan|lan[0-9]*|lan_*|*_lan) ;;
				*) continue ;;
			esac
			dev=$(ubus -t 2 call "network.interface.$net" status 2>/dev/null | jsonfilter -e '@.l3_device' 2>/dev/null)
			add_dev "$dev"
		done
	done
	dev=$(uci -q get network.lan.device 2>/dev/null)
	add_dev "$dev"
	dev=$(ubus -t 2 call network.interface.lan status 2>/dev/null | jsonfilter -e '@.l3_device' 2>/dev/null)
	add_dev "$dev"
	[ -n "$devs" ] || devs=" br-lan "
	echo "${devs# }"
}

# nft anonymous set syntax: "br-lan", "br-guest"
nft_if_list() {
	echo "$LAN_IFS" | awk '{for (i=1; i<=NF; i++) printf "%s\"%s\"", (i>1 ? ", " : ""), $i}'
}

core_type() {
	[ "$(uci -q get flclash.main.core_type 2>/dev/null)" = "sing-box" ] && echo sing-box || echo mihomo
}

core_alive() {
	local addr port
	addr=$(uci -q get flclash.main.external_controller 2>/dev/null)
	[ -n "$addr" ] || addr=127.0.0.1:9097
	port=${addr##*:}
	case "$port" in ''|*[!0-9]*) return 1 ;; esac
	# Match on the listen port only: 127.0.0.1:9097, the 0.0.0.0:9097 wildcard
	# and IPv6 binds (rendered by busybox netstat as :::9097 or [::1]:9097)
	# must all pass. The previous host-side greps missed every IPv6 form and
	# made sync tear down a healthy takeover every 5s; a host string injected
	# into grep also distorted the pattern ([::1] parses as a bracket class).
	netstat -lnt 2>/dev/null | grep -q ":$port "
}

core_proc() {
	# Ask /proc directly: `ps | grep mihomo` also matched unrelated instances
	# and awk pipelines, which kept sync() from cleaning up stale rules.
	local comm
	for comm in /proc/[0-9]*/comm; do
		[ -r "$comm" ] || continue
		case "$(cat "$comm" 2>/dev/null)" in
			mihomo|sing-box) return 0 ;;
		esac
	done
	return 1
}

# True when a mihomo/sing-box process that is NOT ours is running. Our IPC core
# is identifiable by the `/tmp/flclash-core.sock` argv passed by the bridge.
core_other_running() {
	local pid comm cmd
	for pid in /proc/[0-9]*; do
		comm=$(cat "$pid/comm" 2>/dev/null) || continue
		case "$comm" in
			mihomo|sing-box) ;;
			*) continue ;;
		esac
		cmd=$(tr '\0' ' ' < "$pid/cmdline" 2>/dev/null)
		case "$cmd" in
			*flclash-core.sock*) continue ;;
		esac
		return 0
	done
	return 1
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

# Persist a prepared config atomically. The temp file lives on the same
# filesystem as $CFG; sync() flushes data before rename, and rename never
# exposes a truncated file to the core or to a power cut.
commit_config() {
	local tmp="$1"
	[ -s "$tmp" ] || { rm -f "$tmp"; return 1; }
	sync
	mv -f "$tmp" "$CFG" || { rm -f "$tmp"; return 1; }
	chmod 0600 "$CFG" 2>/dev/null
	return 0
}

# Subscriptions/editors may hand us a UTF-8 BOM and/or CRLF. A BOM defeats the
# `^dns:` / `^tun:` anchors below and can produce duplicate top-level keys.
normalize_config() {
	[ -f "$CFG" ] || return 0
	local tmp="${CFG}.norm.$$"
	awk 'NR==1{sub(/^\357\273\277/, "")} {sub(/\r$/, ""); print}' "$CFG" > "$tmp" || {
		rm -f "$tmp"
		return 1
	}
	if cmp -s "$tmp" "$CFG"; then
		rm -f "$tmp"
		return 0
	fi
	commit_config "$tmp" || return 1
	logger -t flclash-proxy "normalized config (BOM/CRLF)"
	return 0
}

# Mode owns TUN: `proxy.mode=tun` forces tun.enable=true; every other mode
# forces false. Without this, stop() could not stop TUN (the core kept the
# old config) and redirect mode silently coexisted with a TUN takeover.
ensure_tun_enable() {
	local want=false tmp
	[ "$(proxy_mode)" = "tun" ] && want=true
	tmp="${CFG}.flclash.tmp.$$"
	if grep -q '^tun:' "$CFG"; then
		if ! awk -v w="$want" '
			/^tun:/ {t=1; next}
			t && /^[^ \t#]/ {t=0}
			t && /^[ \t]*enable:/ { if ($0 ~ ("enable:[ \t]*" w)) f=1; exit }
			END { exit !f }
		' "$CFG"; then
			awk -v w="$want" '
				/^tun:/ {t=1; print; next}
				t && /^[^ \t#]/ {t=0}
				t && !s && /^[ \t]*enable:/ { match($0, /^[ \t]*/); print substr($0, 1, RLENGTH) "enable: " w; s=1; next }
				t && !s && /^[ \t]+[^ \t]/ { match($0, /^[ \t]*/); print substr($0, 1, RLENGTH) "enable: " w; s=1 }
				{print}
			' "$CFG" > "$tmp" && commit_config "$tmp" && PREPARE_CHANGED=1
		fi
	elif [ "$want" = "true" ]; then
		cat >> "$CFG" <<EOF

tun:
  enable: true
  stack: mixed
  auto-route: true
  auto-detect-interface: true
  route-exclude-address:
    - 10.0.0.0/8
    - 100.64.0.0/10
    - 127.0.0.0/8
    - 169.254.0.0/16
    - 172.16.0.0/12
    - 192.0.0.0/24
    - 192.168.0.0/16
    - 224.0.0.0/4
    - 240.0.0.0/4
    - ::1/128
    - fc00::/7
    - fe80::/10
    - ff00::/8
  dns-hijack:
    - any:53
  mtu: 1500
EOF
		PREPARE_CHANGED=1
	fi
}

# Top-level `ipv6` is what mihomo uses for its own v6 handling; keep it aligned
# with the takeover policy so a mode switch is not silently ignored.
ensure_top_ipv6() {
	local want=true tmp
	[ "$(proxy_ipv6)" = "off" ] && want=false
	tmp="${CFG}.flclash.tmp.$$"
	if grep -qE '^ipv6:' "$CFG"; then
		grep -qE "^ipv6:[[:space:]]*$want([[:space:]]|$)" "$CFG" && return 0
		awk -v w="$want" '{ if ($0 ~ /^ipv6:/) print "ipv6: " w; else print }' "$CFG" > "$tmp" && \
			commit_config "$tmp" && PREPARE_CHANGED=1
	else
		printf 'ipv6: %s\n' "$want" > "$tmp" && cat "$CFG" >> "$tmp" && \
			commit_config "$tmp" && PREPARE_CHANGED=1
	fi
	return 0
}

# Prepare device-critical config before the core starts. The native FlClash IPC
# core only applies TUN during listener.start, so adding these keys after a
# controller reload is too late. Existing custom keys/lists are left intact.
prepare_config() {
	PREPARE_CHANGED=0
	[ -f "$CFG" ] || return 0
	normalize_config
	ensure_tun_enable
	ensure_top_ipv6
	# Per-invocation scratch name: the 5s sync loop, core restarts and the API
	# can each run prepare_config; a shared fixed temp file let two writers
	# interleave and publish half of each other's rewrite.
	local tmp
	tmp="${CFG}.flclash.tmp.$$"

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
		commit_config "$tmp" && PREPARE_CHANGED=1
	fi

	if grep -qE '^routing-mark:' "$CFG"; then
		if ! grep -qE '^routing-mark:[[:space:]]*333([[:space:]]|$)' "$CFG"; then
			awk '{if ($0 ~ /^routing-mark:/) print "routing-mark: 333"; else print}' "$CFG" > "$tmp" && \
				commit_config "$tmp" && PREPARE_CHANGED=1
		fi
	else
		if grep -q '^external-controller:' "$CFG"; then
			awk '{print; if ($0 ~ /^external-controller:/) print "routing-mark: 333"}' "$CFG" > "$tmp"
		else
			printf 'routing-mark: 333\n' > "$tmp"
			cat "$CFG" >> "$tmp"
		fi
		commit_config "$tmp" && PREPARE_CHANGED=1
	fi

	# A dedicated HTTP listener cannot share the mixed listener's port. The
	# mixed listener already provides HTTP and SOCKS, so disable the duplicate.
	local mixed_port http_port
	mixed_port=$(sed -n 's/^mixed-port:[[:space:]]*//p' "$CFG" | head -1)
	http_port=$(sed -n 's/^port:[[:space:]]*//p' "$CFG" | head -1)
	if [ -n "$mixed_port" ] && [ "$http_port" = "$mixed_port" ]; then
		awk '{if ($0 ~ /^port:/) print "port: 0"; else print}' "$CFG" > "$tmp" && \
			commit_config "$tmp" && PREPARE_CHANGED=1
	fi

	if [ "$(proxy_mode)" = "tun" ] && ! awk '/^tun:/{t=1;next} t && /^[^ \t#]/{t=0} t && /^[ \t]*route-exclude-address:/{found=1;exit} END{exit !found}' "$CFG"; then
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
		' "$CFG" > "$tmp" && commit_config "$tmp" && PREPARE_CHANGED=1
	fi

	# The prerouting REDIRECT delivers LAN packets to the INCOMING interface's
	# address (192.168.x.1:$REDIR_PORT), not to 127.0.0.1. With allow-lan false
	# mihomo binds the redir/tproxy listeners to 127.0.0.1 only, so every
	# redirected LAN connection is refused while the router's own traffic
	# (loopback-destined) keeps working — the classic "本机可代理、下挂设备不通"
	# failure. allow-lan true makes the listeners bind "*" so LAN takeover
	# actually connects; the WAN-side exposure is closed by fw4_input_on().
	local want_allow=true
	[ "$(proxy_mode)" = "off" ] && want_allow=false
	if [ "$want_allow" = "true" ]; then
		if ! grep -qE '^allow-lan:[[:space:]]*true([[:space:]]|$)' "$CFG"; then
			if grep -qE '^allow-lan:' "$CFG"; then
				awk '{if ($0 ~ /^allow-lan:/) print "allow-lan: true"; else print}' "$CFG" > "$tmp" && \
					commit_config "$tmp" && PREPARE_CHANGED=1
			else
				printf 'allow-lan: true\n' > "$tmp" && cat "$CFG" >> "$tmp" && \
					commit_config "$tmp" && PREPARE_CHANGED=1
			fi
		fi
	elif grep -qE '^allow-lan:[[:space:]]*true([[:space:]]|$)' "$CFG"; then
		awk '{if ($0 ~ /^allow-lan:/) print "allow-lan: false"; else print}' "$CFG" > "$tmp" && \
			commit_config "$tmp" && PREPARE_CHANGED=1
	fi

	# Client DNS queries are hijacked straight to the core's :$DNS_PORT
	# listener and never pass through dnsmasq, so a dnsmasq-side filter_aaaa
	# can never see them. Filter AAAA at the source instead: with
	# flclash.proxy.filter_aaaa=1 (the default) mihomo's DNS stops answering
	# AAAA, keeping dual-stack LAN clients on the v4 path the takeover covers.
	local want_dns_ipv6=true
	[ "$(proxy_ipv6)" = "off" ] && want_dns_ipv6=false
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
			' "$CFG" > "$tmp" && commit_config "$tmp" && PREPARE_CHANGED=1
		fi
	else
		# Insert at the dns block's own child indent.
		awk -v w="$want_dns_ipv6" '
			/^dns:/ {d=1; print; next}
			d && /^[^ \t#]/ {d=0}
			d && !ins && /^[ \t]+[^ \t]/ { match($0, /^[ \t]*/); print substr($0, 1, RLENGTH) "ipv6: " w; ins=1 }
			{print}
			END { if (d && !ins) print "  ipv6: " w }
		' "$CFG" > "$tmp" && commit_config "$tmp" && PREPARE_CHANGED=1
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
	# In allow/block mode only a subset of clients is hijacked by nft. A global
	# dnsmasq upstream of 127.0.0.1:1053 would still route the *other* clients'
	# DNS through the proxy resolver, defeating the ACL. Restore the original
	# upstream and let nft redirect only the selected clients.
	if [ "$(acl_mode)" != "all" ]; then
		dns_off
		local change=0
		if [ "$(proxy_ipv6)" = "off" ] && [ "$(uci -q get dhcp.@dnsmasq[0].filter_aaaa)" != "1" ]; then
			uci -q set dhcp.@dnsmasq[0].filter_aaaa=1 && change=1
			uci -q set flclash.proxy.dns_filter_managed=1
		elif [ "$(proxy_ipv6)" != "off" ] && [ "$(uci -q get flclash.proxy.dns_filter_managed)" = "1" ]; then
			uci -q delete dhcp.@dnsmasq[0].filter_aaaa 2>/dev/null && change=1
			uci -q delete flclash.proxy.dns_filter_managed 2>/dev/null
		fi
		if [ "$change" = "1" ]; then
			uci -q commit dhcp 2>/dev/null
			uci -q commit flclash 2>/dev/null
			dnsmasq_restart_robust
		fi
		return 0
	fi
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
	filter=0
	[ "$(proxy_ipv6)" = "off" ] && filter=1
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
	# Also drop a leftover .new: dnsmasq's conf-dir reads every non-hidden
	# file, so an interrupted dns_on otherwise keeps pointing dnsmasq at a
	# possibly dead core listener long after dns_off ran.
	rm -f "$DNSCONF" "$DNSCONF.new"
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
	# Never delete another proxy's table/routes, and never rip resources out
	# from under a running core (ours or theirs).
	if core_other_running; then
		logger -t flclash-proxy "another mihomo/sing-box is running; skipping core artifact purge"
		return 0
	fi
	core_proc && return 0
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

# --- fw4 integration --------------------------------------------------------
#
# The WAN guard/accept rules live in an fw4 `include` (chain-prepend input,
# /var/run/flclash/fw4-input.nft) instead of runtime `nft insert`. fw4 reload
# replays the include, so a firewall restart can no longer silently drop the
# guard and expose the proxy ports. The file is regenerated only when its
# content changes; a reload is skipped otherwise.

fw4_legacy_cleanup() {
	local h
	[ -f "$FW4_INCLUDE_DIR/.legacy-cleaned" ] && return 0
	mkdir -p "$FW4_INCLUDE_DIR" 2>/dev/null
	# With the fw4 include configured, the next firewall reload rebuilds the
	# input chain from the include and drops any runtime-inserted leftovers.
	# Without it (very old install), delete the legacy handles explicitly.
	if uci -q get firewall.flclash >/dev/null 2>&1; then
		: > "$FW4_INCLUDE_DIR/.legacy-cleaned" 2>/dev/null
		return 0
	fi
	for h in $(nft -a list chain inet fw4 input 2>/dev/null | awk '/FlClash (Redirect|TPROXY) Accept|FlClash WAN Proxy Guard/ {print $NF}'); do
		nft delete rule inet fw4 input handle "$h" 2>/dev/null
	done
	rm -f "$FW4_INCLUDE"
	: > "$FW4_INCLUDE_DIR/.legacy-cleaned" 2>/dev/null
	return 0
}

fw4_reload() {
	if command -v fw4 >/dev/null 2>&1; then
		fw4 reload >/dev/null 2>&1 && return 0
	fi
	[ -x /etc/init.d/firewall ] && /etc/init.d/firewall reload >/dev/null 2>&1
	return 0
}

# Renders the include. Returns 0 when the content changed (caller reloads).
fw4_input_render() {
	local mode="$1" ports tmp template
	tmp="$FW4_INCLUDE.new"
	template=/usr/share/ucode/flclash/fw4_input.ut
	mkdir -p "$FW4_INCLUDE_DIR" 2>/dev/null
	chmod 0700 "$FW4_INCLUDE_DIR" 2>/dev/null
	ports="$MIXED_PORT, $REDIR_PORT, $TPROXY_PORT, $DNS_PORT, $API_PORT"
	if [ "$mode" = "on" ]; then
		if [ -f "$template" ] && command -v utpl >/dev/null 2>&1; then
			FLCLASH_LAN_NFT="$LAN_IFS_NFT" \
			FLCLASH_PORTS="$ports" \
			FLCLASH_REDIR_PORT="$REDIR_PORT" \
			FLCLASH_DNS_PORT="$DNS_PORT" \
			FLCLASH_FWMARK="$FWMARK" \
				utpl -S "$template" > "$tmp" 2>/dev/null || rm -f "$tmp"
		fi
		if [ ! -s "$tmp" ]; then
			# utpl/ucode missing or template failed: identical shell fallback.
			{
				echo "meta mark $FWMARK accept comment \"FlClash TPROXY Accept\""
				echo "iifname { $LAN_IFS_NFT } ct status dnat meta l4proto { tcp, udp } th dport { $REDIR_PORT, $DNS_PORT } accept comment \"FlClash Redirect Accept\""
				echo "iifname != { $LAN_IFS_NFT } iifname != \"lo\" meta l4proto { tcp, udp } th dport { $ports } reject comment \"FlClash WAN Proxy Guard\""
			} > "$tmp" || return 1
		fi
	else
		# A missing include is normal before the first start. Create the empty
		# file but do not force a firewall reload for it.
		if [ ! -f "$FW4_INCLUDE" ]; then
			: > "$FW4_INCLUDE"
			chmod 0600 "$FW4_INCLUDE" 2>/dev/null
			return 1
		fi
		: > "$tmp"
	fi
	if cmp -s "$tmp" "$FW4_INCLUDE" 2>/dev/null; then
		rm -f "$tmp"
		return 1
	fi
	mv -f "$tmp" "$FW4_INCLUDE" || { rm -f "$tmp"; return 1; }
	chmod 0600 "$FW4_INCLUDE" 2>/dev/null
	return 0
}

fw4_input_on() {
	fw4_legacy_cleanup
	fw4_input_render on && fw4_reload
	return 0
}

fw4_input_off() {
	fw4_legacy_cleanup
	fw4_input_render off && fw4_reload
	return 0
}

# The core inserts `!mihomo: Accept traffic from tun` rules into fw4 input.
# They survive a normal stop, so remove them once TUN is gone. Only touches
# rules whose comment identifies mihomo's own TUN hook.
fw4_cleanup_core_rules() {
	local h
	for h in $(nft -a list chain inet fw4 input 2>/dev/null | awk '/!mihomo:/ {print $NF}'); do
		nft delete rule inet fw4 input handle "$h" 2>/dev/null
	done
}

# QUIC policy: `reject` forces browsers to fall back to TCP (useful when the
# node does not carry UDP reliably); `bypass` leaves UDP/443 direct.
nft_quic_rules() {
	case "$QUIC_MODE" in
		reject) echo "iifname { $LAN_IFS_NFT } meta l4proto udp udp dport 443 reject" ;;
		bypass) echo "iifname { $LAN_IFS_NFT } meta l4proto udp udp dport 443 accept" ;;
	esac
}

# In allow mode with no MAC entries there is no v6 catch-all (IPv6 client
# addresses are not modelled yet), so readiness must not demand one either.
ipv6_intercept_enabled() {
	[ "$IPV6_MODE" = "proxy" ] || return 1
	if [ "$(acl_mode)" = "allow" ]; then
		[ -n "$(acl_ips6)$(acl_macs)" ] || return 1
	fi
	return 0
}

nft_lan_ready() {
	[ "$(cat "$LAN_SIG" 2>/dev/null)" = "$LAN_SIG_VALUE" ] || return 1
	if [ "$PROXY_MODE" = "tproxy" ]; then
		nft list chain inet "$TABLE" prerouting_tcp_tproxy 2>/dev/null | grep -Eq 'iifname( {)? "'$LAN_IF'"' || return 1
		nft list chain inet "$TABLE" prerouting_tcp_tproxy 2>/dev/null | grep -q 'tproxy ip to 127.0.0.1:'"$TPROXY_PORT" || return 1
	else
		nft list chain inet "$TABLE" prerouting_tcp 2>/dev/null | grep -Eq 'iifname( {)? "'$LAN_IF'"' || return 1
		nft list chain inet "$TABLE" prerouting_tcp 2>/dev/null | grep -q 'redirect to :'"$REDIR_PORT" || return 1
	fi
	nft list chain inet "$TABLE" prerouting_udp 2>/dev/null | grep -Eq 'iifname( {)? "'$LAN_IF'"' || return 1
	nft list chain inet "$TABLE" prerouting_udp 2>/dev/null | grep -q 'tproxy ip to 127.0.0.1:'"$TPROXY_PORT" || return 1
	if ipv6_intercept_enabled; then
		nft list chain inet "$TABLE" prerouting_udp 2>/dev/null | grep -q 'ip6 daddr' || return 1
	fi
	if [ "$IPV6_MODE" = "off" ]; then
		nft list chain inet "$TABLE" v6_block 2>/dev/null | grep -q 'drop' || return 1
	fi
	[ "$(core_type)" = "sing-box" ] && return 0
	nft list chain inet "$TABLE" prerouting_dns 2>/dev/null | grep -q 'redirect to :'"$DNS_PORT"
}

nft_output_local() {
	# Static rules: rebuilding the output chain on every 5s sync was pure
	# netlink churn. Rebuild only when it is missing or the redir port changed.
	if nft list chain inet "$TABLE" output_tcp 2>/dev/null | grep -q "redirect to :$REDIR_PORT"; then
		if [ "$IPV6_MODE" != "proxy" ] || nft list chain inet "$TABLE" output_tcp 2>/dev/null | grep -q 'ip6 daddr != @private_v6'; then
			return 0
		fi
	fi
	nft list table inet "$TABLE" >/dev/null 2>&1 || "$NFT" -f - <<EOF
table inet $TABLE {
	set private_v4 { type ipv4_addr; flags interval; elements = { $PRIVATE } }
	set private_v6 { type ipv6_addr; flags interval; auto-merge; elements = { $PRIVATE_V6 } }
	chain output_tcp { type nat hook output priority dstnat - 10; policy accept; }
}
EOF
	# flush + rebuild in ONE netlink transaction. The previous flush-then-add
	# sequence ran a zero-rule window every 5s sync in which mihomo's own
	# replies re-hit the catch-all redirect and locally originated connections
	# leaked past the exemption rules.
	local v6_rules=""
	if [ "$IPV6_MODE" = "proxy" ]; then
		v6_rules="ip6 daddr @private_v6 accept
		ct original ip6 daddr @private_v6 accept
		ip6 daddr != @private_v6 meta l4proto tcp redirect to :$REDIR_PORT"
	fi
	"$NFT" -f - <<EOF
table inet $TABLE {
	chain output_tcp {
		flush
		meta mark $CORE_MARK accept
		# Every reply to an inbound connection bypasses REDIRECT, including SSH/LuCI
		# sessions to a public WAN address. This is stronger than an IP-only guard.
		ct direction reply accept
		ip daddr @private_v4 accept
		ct original ip daddr @private_v4 accept
		$v6_rules
		tcp dport 53 accept
		tcp dport $REDIR_PORT accept
		tcp dport $TPROXY_PORT accept
		meta nfproto ipv4 meta l4proto tcp redirect to :$REDIR_PORT
	}
}
EOF
}

# Emits nft bypass rules for the prerouting chains.
#   block mode: listed clients / CIDRs / ports accept (never intercepted);
#   allow mode: the chain's catch-all redirect is gated on the client list,
#               so unlisted clients fall through to the default accept.
# `nft_acl_rules <tcp|udp>` prints the bypass lines for that chain.
nft_acl_rules() {
	local proto="$1" mode ip ip6 mac cidr cidr6 port
	mode=$(acl_mode)
	if [ "$mode" = "block" ]; then
		for ip in $(acl_ips); do echo "ip saddr $ip accept"; done
		for ip6 in $(acl_ips6); do echo "ip6 saddr $ip6 accept"; done
		for mac in $(acl_macs); do echo "ether saddr $mac accept"; done
	fi
	for cidr in $(bypass_cidrs); do echo "ip daddr $cidr accept"; done
	for cidr6 in $(bypass_cidrs6); do echo "ip6 daddr $cidr6 accept"; done
	for port in $(bypass_ports); do echo "$proto dport $port accept"; done
}

# Emits bypass lines for the DNS hijack chain: block-mode clients fall through
# to dnsmasq instead of the core's listener, and destination bypass CIDRs are
# never hijacked either. (Before this, "block" clients kept losing their DNS.)
nft_acl_dns_rules() {
	local mode ip ip6 mac cidr cidr6
	mode=$(acl_mode)
	if [ "$mode" = "block" ]; then
		for ip in $(acl_ips); do echo "ip saddr $ip accept"; done
		for ip6 in $(acl_ips6); do echo "ip6 saddr $ip6 accept"; done
		for mac in $(acl_macs); do echo "ether saddr $mac accept"; done
	fi
	for cidr in $(bypass_cidrs); do echo "ip daddr $cidr accept"; done
	for cidr6 in $(bypass_cidrs6); do echo "ip6 daddr $cidr6 accept"; done
}

# DNS hijack rules. In allow mode a global hijack would send unlisted clients'
# DNS through the proxy resolver even though their traffic stays direct, so the
# redirect is emitted per allowed client (MAC-only for IPv6 until v6 ACLs are
# modelled).
nft_dns_hijack_rules() {
	local mode ip ip6 mac listed=0
	[ "$(core_type)" = "sing-box" ] && return 0
	mode=$(acl_mode)
	emit() {
		local prefix="$1"
		echo "iifname { $LAN_IFS_NFT } $prefix udp dport 53 redirect to :$DNS_PORT"
		echo "iifname { $LAN_IFS_NFT } $prefix tcp dport 53 redirect to :$DNS_PORT"
		if [ "$IPV6_MODE" = "proxy" ]; then
			echo "iifname { $LAN_IFS_NFT } $prefix meta nfproto ipv6 udp dport 53 redirect to :$DNS_PORT"
			echo "iifname { $LAN_IFS_NFT } $prefix meta nfproto ipv6 tcp dport 53 redirect to :$DNS_PORT"
		fi
	}
	if [ "$mode" != "allow" ]; then
		emit ""
		return 0
	fi
	for ip in $(acl_ips); do emit "ip saddr $ip"; listed=1; done
	for ip6 in $(acl_ips6); do emit "ip6 saddr $ip6"; listed=1; done
	for mac in $(acl_macs); do emit "ether saddr $mac"; listed=1; done
	if [ "$listed" = "0" ]; then
		logger -t flclash-proxy "acl_mode=allow but client list is empty; DNS falls back to all"
		emit ""
	fi
}

# Emits the catch-all interception rules, one per allowed client in allow
# mode (nft cannot OR ip saddr with ether saddr in a single match).
# `nft_intercept_rules <tcp|udp>` prints full rule lines.
nft_intercept_rules() {
	local proto="$1" mode ip mac tail
	if [ "$proto" = "tcp" ]; then
		if [ "$PROXY_MODE" = "tproxy" ]; then
			tail="ip daddr != @private_v4 tcp dport 1-65535 meta mark set $FWMARK tproxy ip to 127.0.0.1:$TPROXY_PORT accept"
		else
			tail="ip daddr != @private_v4 tcp dport 1-65535 redirect to :$REDIR_PORT"
		fi
	else
		tail="ip daddr != @private_v4 meta l4proto udp udp dport 1-65535 meta mark set $FWMARK tproxy ip to 127.0.0.1:$TPROXY_PORT accept"
	fi
	mode=$(acl_mode)
	if [ "$mode" != "allow" ]; then
		echo "iifname { $LAN_IFS_NFT } $tail"
		return
	fi
	local listed=0
	for ip in $(acl_ips); do echo "iifname { $LAN_IFS_NFT } ip saddr $ip $tail"; listed=1; done
	for mac in $(acl_macs); do echo "iifname { $LAN_IFS_NFT } ether saddr $mac $tail"; listed=1; done
	if [ "$listed" = "0" ]; then
		logger -t flclash-proxy "acl_mode=allow but client list is empty; falling back to all"
		echo "iifname { $LAN_IFS_NFT } $tail"
	fi
}

# IPv6 counterpart. ACL IPv6 client addresses are not modelled yet, so in
# allow mode only MAC-matched clients get a v6 catch-all (unlisted v6 traffic
# stays direct rather than silently bypassing the whitelist).
nft_intercept_rules6() {
	local proto="$1" mode ip6 mac tail
	[ "$IPV6_MODE" = "proxy" ] || return 0
	if [ "$proto" = "tcp" ]; then
		if [ "$PROXY_MODE" = "tproxy" ]; then
			tail="ip6 daddr != @private_v6 tcp dport 1-65535 meta mark set $FWMARK tproxy ip6 to [::1]:$TPROXY_PORT accept"
		else
			tail="ip6 daddr != @private_v6 tcp dport 1-65535 redirect to :$REDIR_PORT"
		fi
	else
		tail="ip6 daddr != @private_v6 meta l4proto udp udp dport 1-65535 meta mark set $FWMARK tproxy ip6 to [::1]:$TPROXY_PORT accept"
	fi
	mode=$(acl_mode)
	if [ "$mode" != "allow" ]; then
		echo "iifname { $LAN_IFS_NFT } $tail"
		return
	fi
	local listed=0
	for ip6 in $(acl_ips6); do echo "iifname { $LAN_IFS_NFT } ip6 saddr $ip6 $tail"; listed=1; done
	for mac in $(acl_macs); do echo "iifname { $LAN_IFS_NFT } ether saddr $mac $tail"; listed=1; done
	[ "$listed" = "1" ] || logger -t flclash-proxy "acl_mode=allow has no IPv6 client entry; IPv6 stays direct"
}

ensure_kernel_modules() {
	local mod
	for mod in nft_nat nft_tproxy; do
		grep -q "^$mod " /proc/modules 2>/dev/null && continue
		modprobe "$mod" 2>/dev/null && continue
		logger -t flclash-proxy "ERROR: kernel module $mod is unavailable; REDIRECT/TPROXY cannot work (install kmod-nft-nat / kmod-nft-tproxy)"
	done
}

nft_on() {
	ensure_kernel_modules
	if ! nft_lan_ready; then
		nft delete table inet "$TABLE" 2>/dev/null
		ip rule add fwmark "$FWMARK" table 100 2>/dev/null
		ip route add local default dev lo table 100 2>/dev/null
		[ "$IPV6_MODE" = "proxy" ] && {
			ip -6 rule add fwmark "$FWMARK" table 100 2>/dev/null
			ip -6 route add local default dev lo table 100 2>/dev/null
		}
		local tcp_chain="" v6_block=""
		if [ "$PROXY_MODE" = "tproxy" ]; then
			tcp_chain="chain prerouting_tcp_tproxy {
		type filter hook prerouting priority mangle - 10; policy accept;
		$(nft_acl_rules tcp)
		$(nft_intercept_rules tcp)
		$(nft_intercept_rules6 tcp)
	}"
		else
			tcp_chain="chain prerouting_tcp {
		type nat hook prerouting priority dstnat + 10; policy accept;
		$(nft_acl_rules tcp)
		$(nft_intercept_rules tcp)
		$(nft_intercept_rules6 tcp)
	}"
		fi
		if [ "$IPV6_MODE" = "off" ]; then
			v6_block="chain v6_block {
		type filter hook forward priority filter; policy accept;
		ip6 daddr != @private_v6 drop comment \"FlClash IPv6 leak guard\"
	}"
		fi
		"$NFT" -f - <<EOF
table inet $TABLE {
	set private_v4 { type ipv4_addr; flags interval; elements = { $PRIVATE } }
	set private_v6 { type ipv6_addr; flags interval; auto-merge; elements = { $PRIVATE_V6 } }
	chain prerouting_dns {
		type nat hook prerouting priority dstnat - 10; policy accept;
		$(nft_acl_dns_rules)
		$(nft_dns_hijack_rules)
	}
	$tcp_chain
	chain prerouting_udp {
		type filter hook prerouting priority mangle - 10; policy accept;
		$(nft_acl_rules udp)
		$(nft_quic_rules)
		$(nft_intercept_rules udp)
		$(nft_intercept_rules6 udp)
	}
	$v6_block
}
EOF
		[ $? -eq 0 ] || return 1
	fi
	fw4_input_on
	[ "$LOCAL_TAKEOVER" = "1" ] && [ "$(core_type)" != "sing-box" ] && nft_output_local
	echo "$LAN_SIG_VALUE" > "$LAN_SIG" 2>/dev/null
	nft_lan_ready
}

nft_off() {
	nft delete table inet "$TABLE" 2>/dev/null
	ip rule del fwmark "$FWMARK" table 100 2>/dev/null
	ip route del local default dev lo table 100 2>/dev/null
	ip -6 rule del fwmark "$FWMARK" table 100 2>/dev/null
	ip -6 route del local default dev lo table 100 2>/dev/null
	rm -f "$LAN_SIG"
	fw4_input_off
	return 0
}

nft_active() { nft_lan_ready; }

# --- backend dispatch (pure nftables/fw4) ------------------------------------
rules_on() { nft_on; }
rules_off() { nft_off; }
rules_active() { nft_active; }
output_local_on() {
	[ "$LOCAL_TAKEOVER" = "1" ] && [ "$(core_type)" != "sing-box" ] || return 0
	nft_output_local
}

offload_guard() {
	local changed=0 opt val cur
	OFFLOAD_CHANGED=0
	[ "$(uci -q get flclash.proxy.auto_disable_offload 2>/dev/null)" != "0" ] || return 0
	# One-shot: once we disabled offload, a user re-enabling it in LuCI must
	# not be fought every 5 seconds.
	[ "$(uci -q get flclash.proxy.offload_disabled_by_us 2>/dev/null)" = "1" ] && return 0
	# Remember the pre-proxy values ONCE so stopping the proxy can put the
	# router back instead of leaving hardware offloading disabled forever.
	if [ "$(uci -q get flclash.proxy.offload_saved 2>/dev/null)" != "1" ]; then
		uci -q set flclash.proxy=proxy
		uci -q set flclash.proxy.offload_saved=1
		for opt in flow_offloading flow_offloading_hw; do
			uci -q delete "flclash.proxy.saved_$opt" 2>/dev/null
			val=$(uci -q get "firewall.@defaults[0].$opt" 2>/dev/null)
			[ -n "$val" ] && uci -q set "flclash.proxy.saved_$opt=$val"
		done
		uci -q commit flclash
	fi
	for opt in flow_offloading flow_offloading_hw; do
		cur=$(uci -q get "firewall.@defaults[0].$opt" 2>/dev/null)
		if [ -n "$cur" ] && [ "$cur" != "0" ]; then
			uci -q set "firewall.@defaults[0].$opt='0'" && changed=1
		fi
	done
	if [ "$changed" = "1" ]; then
		OFFLOAD_CHANGED=1
		uci -q set flclash.proxy.offload_disabled_by_us=1
		uci -q commit firewall
		uci -q commit flclash
		# `reload` rebuilds the flowtable without dropping every conntrack
		# entry (the old `restart` + conntrack -F killed active SSH/downloads).
		fw4_reload
		logger -t flclash-proxy "flow offloading disabled while transparent proxy is active"
	fi
	return 0
}

# Restores the flow-offloading values saved by offload_guard(). Runs from
# teardown(); a no-op when nothing was saved, so the 5s sync loop is cheap.
offload_restore() {
	[ "$(uci -q get flclash.proxy.offload_saved 2>/dev/null)" = "1" ] || return 0
	local changed=0 opt val cur
	for opt in flow_offloading flow_offloading_hw; do
		val=$(uci -q get "flclash.proxy.saved_$opt" 2>/dev/null)
		cur=$(uci -q get "firewall.@defaults[0].$opt" 2>/dev/null)
		# Only undo our own disable. If the user turned offload back on while
		# the proxy ran, keep their choice.
		if [ "$cur" = "0" ] || [ "$cur" = "$val" ]; then
			if [ -n "$val" ]; then
				[ "$cur" != "$val" ] && uci -q set "firewall.@defaults[0].$opt=$val" && changed=1
			else
				[ -n "$cur" ] && uci -q delete "firewall.@defaults[0].$opt" && changed=1
			fi
		else
			logger -t flclash-proxy "keeping user-selected $opt=$cur"
		fi
		uci -q delete "flclash.proxy.saved_$opt" 2>/dev/null
	done
	uci -q delete flclash.proxy.offload_saved 2>/dev/null
	uci -q delete flclash.proxy.offload_disabled_by_us 2>/dev/null
	# Older installs disabled bridger as a side effect; restore only those.
	if [ "$(uci -q get flclash.proxy.bridger_saved 2>/dev/null)" = "1" ]; then
		uci -q delete flclash.proxy.bridger_saved 2>/dev/null
		if [ -x /etc/init.d/bridger ]; then
			/etc/init.d/bridger enable >/dev/null 2>&1
			/etc/init.d/bridger start >/dev/null 2>&1
			logger -t flclash-proxy "bridger restored after proxy stop"
		fi
	fi
	uci -q commit flclash 2>/dev/null
	if [ "$changed" = "1" ]; then
		uci -q commit firewall 2>/dev/null
		fw4_reload
		logger -t flclash-proxy "flow offloading restored after proxy stop"
	fi
	return 0
}

# --- convergence ------------------------------------------------------------

teardown() {
	# Stopping must also stop TUN: `off` forces tun.enable=false and restarts
	# the core once if a TUN device was actually up.
	if [ "$(cat "$STATE_FILE" 2>/dev/null || echo off)" = "tun" ] || tun_active; then
		prepare_config
		if [ "${PREPARE_CHANGED:-0}" = "1" ] && core_alive; then
			/etc/init.d/flclash-core restart >/dev/null 2>&1
		fi
	fi
	rules_off
	dns_off
	fw4_cleanup_core_rules
	offload_restore
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
	local core=0 state
	# Re-read mode/ipv6: `start`/`stop` change UCI in this same process.
	PROXY_MODE=$(proxy_mode)
	IPV6_MODE=$(proxy_ipv6)
	QUIC_MODE=$(uci -q get flclash.proxy.quic 2>/dev/null)
	case "$QUIC_MODE" in
		proxy|reject|bypass) ;;
		*) QUIC_MODE=proxy ;;
	esac
	LAN_SIG_VALUE="$PROXY_MODE|$IPV6_MODE|$QUIC_MODE|$LAN_IFS"
	state=$(cat "$STATE_FILE" 2>/dev/null || echo off)
	core_alive && core=1

	if [ "$core" = "0" ]; then
		# Do not purge while the IPC process is still initializing its controller.
		core_proc && return 0
		rules_off
		dns_off
		purge_core_artifacts
		set_state off
		return 0
	fi

	case "$PROXY_MODE" in
		off)
			# `off` is a real stop: force tun.enable=false in the config too, so
			# a later core start cannot silently re-enable TUN.
			prepare_config
			if [ "${PREPARE_CHANGED:-0}" = "1" ] && core_alive; then
				/etc/init.d/flclash-core restart >/dev/null 2>&1
				wait_core || true
			fi
			rules_off
			dns_off
			purge_core_artifacts
			set_state off
			return 0
			;;
		tun)
			# Make sure the running core actually has TUN enabled in its config.
			prepare_config
			if [ "${PREPARE_CHANGED:-0}" = "1" ]; then
				/etc/init.d/flclash-core restart >/dev/null 2>&1
				wait_core || { set_state off; return 1; }
			fi
			# listener.start is the only phase that creates the TUN device in
			# the IPC core; a controller ApplyConfig never does. Entering tun
			# mode therefore needs one core restart.
			if ! tun_active && [ "$state" != "tun" ]; then
				/etc/init.d/flclash-core restart >/dev/null 2>&1
				wait_core || { set_state off; return 1; }
			fi
			if ! tun_active && ! wait_tun; then
				# A controller reload in the native IPC core does not start TUN.
				logger -t flclash-proxy "tun requested but no TUN device is active; falling back to REDIRECT"
				redirect_on
				return 1
			fi
			offload_guard
			if [ "$OFFLOAD_CHANGED" = "1" ]; then
				/etc/init.d/flclash-core restart >/dev/null 2>&1
				wait_core || { set_state off; return 1; }
				wait_tun || { set_state off; return 1; }
			fi
			# Only tear down REDIRECT leftovers on the transition into TUN.
			[ "$state" = "tun" ] || rules_off
			dns_on || { set_state off; return 1; }
			if [ "$state" != "tun" ] || ! nft list chain inet "$TABLE" output_tcp >/dev/null 2>&1; then
				output_local_on
			fi
			fw4_input_on
			set_state tun
			return 0
			;;
		redirect|tproxy)
			purge_core_artifacts
			wait_core_if_starting || true
			redirect_on
			return $?
			;;
	esac
	return 1
}

LAN_IFS=$(detect_lan_ifs)
LAN_IF=${LAN_IFS%% *}
[ -n "$LAN_IF" ] || LAN_IF=br-lan
LAN_IFS_NFT=$(nft_if_list)
[ -n "$LAN_IFS_NFT" ] || LAN_IFS_NFT='"br-lan"'
PROXY_MODE=$(proxy_mode)
IPV6_MODE=$(proxy_ipv6)
QUIC_MODE=$(uci -q get flclash.proxy.quic 2>/dev/null)
case "$QUIC_MODE" in
	proxy|reject|bypass) ;;
	*) QUIC_MODE=proxy ;;
esac
LAN_SIG_VALUE="$PROXY_MODE|$IPV6_MODE|$QUIC_MODE|$LAN_IFS"

# API maintenance, core hooks and user actions may run concurrently. Serialize
# state-changing operations so one sync cannot delete a table another is adding.
# flock is preferred (released automatically on exit, no stale-pid races);
# mkdir remains a fallback for images without the applet.
case "$1" in
	start|stop|teardown|sync|boot|reapply|prepare|purge)
		# `start` holds this lock while it invokes /etc/init.d/flclash-core,
		# whose start_service calls `purge`/`prepare` as child processes. The
		# env flag tells those children the lock is already held by their
		# parent (flock is not reentrant across processes).
		if [ "${FLCLASH_LOCK_HELD:-0}" = "1" ]; then
			:
		elif command -v flock >/dev/null 2>&1; then
			LOCKFILE=/var/run/flclash-proxy.lock
			exec 9>"$LOCKFILE" || exit 1
			n=0
			while ! flock -n 9; do
				n=$((n+1))
				if [ "$n" -ge 10 ]; then
					logger -t flclash-proxy "another operation holds $LOCKFILE; skipping $1"
					exit 0
				fi
				sleep 1
			done
		else
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
		fi
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
		mode=$(proxy_mode)
		[ "$mode" = "off" ] && mode=redirect
		uci_set "$mode"
		PROXY_MODE="$mode"
		FLCLASH_LOCK_HELD=1 /etc/init.d/flclash-core start >/dev/null 2>&1
		wait_core_if_starting || true
		sync
		rc=$?
		state=$(cat "$STATE_FILE" 2>/dev/null || echo off)
		echo "transparent proxy: $state ($FIREWALL_BACKEND, lan=$LAN_IF)"
		[ "$rc" = "0" ] && [ "$state" != "off" ]
		;;
	stop)
		uci_set off
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
