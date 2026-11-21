#!/bin/sh
# /usr/libexec/flclash/singbox-convert.sh — translate the Clash-format
# config.yaml into a runnable sing-box JSON config for the core switch.
#
# Usage: singbox-convert.sh <clash.yaml> <out.json>
#
# This is intentionally a pragmatic converter, not a universal Clash->sing-box
# translator. It reliably maps the scalars the WebUI manages (mixed-port,
# external-controller, mode) plus DOMAIN/DOMAIN-SUFFIX rules, and always emits
# working direct/reject outbounds and clash_api so that after a core switch:
#   - the mixed proxy still listens on the same port
#   - the WebUI controller (9097) still works
#   - rule routing (accept/reject) still behaves
# Full proxy-node translation (ss/trojan/vmess/...) is intentionally left to
# the sing-box native config: if a file named <yaml>.singbox.json exists it is
# used verbatim as the base and only the managed scalars are patched in.
# Full "lossless" conversion lives in the flclash-api Rust binary: it parses
# proxies/groups/rules/DNS/TUN into a native sing-box 1.14 config. Prefer it;
# fall back to the pragmatic awk converter below on older installs where the
# binary predates the --singbox-convert flag.
set -eu

IN="${1:-/etc/flclash/config.yaml}"
OUT="${2:-/tmp/flclash-singbox.json}"

[ -f "$IN" ] || exit 1

if [ -x /usr/bin/flclash-api ] \
	&& /usr/bin/flclash-api --singbox-convert "$IN" "$OUT" >/dev/null 2>&1 \
	&& [ -s "$OUT" ]; then
	# Normalise Clash-only cipher aliases that sing-box rejects. Some
	# subscriptions ship metadata/placeholder nodes with `cipher: chacha20`;
	# mihomo tolerates them but sing-box aborts startup with
	# "unknown method: chacha20". Map to the standard AEAD name so a single
	# bad node cannot take down the whole core.
	sed -i \
		-e 's/"method"[[:space:]]*:[[:space:]]*"chacha20"/"method": "chacha20-ietf-poly1305"/g' \
		-e 's/"method"[[:space:]]*:[[:space:]]*"chacha20-ietf"/"method": "chacha20-ietf-poly1305"/g' \
		"$OUT"
	exit 0
fi

# Scalar extraction (top-level keys only). Defaults match the project UCI.
scalar() {
	awk -v key="$1" -v def="$2" '
		/^[[:space:]]/ { next }
		$0 ~ "^"key":" {
			sub("^"key":[[:space:]]*", "", $0)
			gsub(/^["'"'"']|["'"'"']$/, "", $0)
			print $0; found=1; exit
		}
		END { if (!found) print def }
	' "$IN"
}

MIXED_PORT=$(scalar mixed-port 7892)
REDIR_PORT=$(scalar redir-port 7893)
TPROXY_PORT=$(scalar tproxy-port 7894)
CONTROLLER=$(scalar external-controller 127.0.0.1:9097)
MODE=$(scalar mode rule)

# Sanitize: numeric ports, controller host:port, valid mode. The controller
# reaches a sed replacement below, so anything outside the host:port
# alphabet falls back to the default rather than corrupting the output.
case "$MIXED_PORT" in ''|*[!0-9]*) MIXED_PORT=7892 ;; esac
case "$REDIR_PORT" in ''|*[!0-9]*) REDIR_PORT=7893 ;; esac
case "$TPROXY_PORT" in ''|*[!0-9]*) TPROXY_PORT=7894 ;; esac
[ -n "$CONTROLLER" ] || CONTROLLER=127.0.0.1:9097
case "$CONTROLLER" in *[!0-9A-Za-z.:\[\]]*) CONTROLLER=127.0.0.1:9097 ;; esac
case "$MODE" in rule|global|direct|script) ;; *) MODE=rule ;; esac

# Optional native base config supplied by the user (full node translation).
BASE="${IN}.singbox.json"
if [ -f "$BASE" ]; then
	# Patch managed scalars with sed (keys are unique in sing-box schema),
	# except listen_port: a multi-inbound base owns several of them and a
	# global rewrite used to point every inbound at the mixed port. Only
	# patch when the base has exactly one (the mixed inbound).
	if [ "$(grep -o '"listen_port"' "$BASE" | wc -l)" = "1" ]; then
		sed -e "s/\"listen_port\"[[:space:]]*:[[:space:]]*[0-9]*/\"listen_port\": $MIXED_PORT/" \
			-e "s/\"external_controller\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"external_controller\": \"$CONTROLLER\"/" \
			-e "s/\"default_mode\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"default_mode\": \"$MODE\"/" \
			"$BASE" > "$OUT"
	else
		sed -e "s/\"external_controller\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"external_controller\": \"$CONTROLLER\"/" \
			-e "s/\"default_mode\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"default_mode\": \"$MODE\"/" \
			"$BASE" > "$OUT"
	fi
	exit 0
fi

# Translate Clash DOMAIN rules into sing-box route rules. REJECT -> action,
# otherwise -> direct-out. Accept both "DOMAIN-SUFFIX,x,DIRECT" and with policy.
RULES=$(awk -F, '
	/^[[:space:]]*-[[:space:]]*DOMAIN(-SUFFIX|-KEYWORD)?,/ {
		# $1 = "- DOMAIN-SUFFIX", $2 = value, $3 = policy
		type=$1; gsub(/^[[:space:]]*-[[:space:]]*/, "", type)
		val=$2;  gsub(/^[[:space:]]+|[[:space:]]+$/, "", val)
		policy=$3; gsub(/^[[:space:]]+|[[:space:]]+$/, "", policy)
		gsub(/"/, "", val)
		if (val=="") next
		key = (type=="DOMAIN-SUFFIX") ? "domain_suffix" : ((type=="DOMAIN-KEYWORD") ? "domain_keyword" : "domain")
		if (policy ~ /REJECT/) {
			printf "{\"%s\":[\"%s\"],\"action\":\"reject\"},", key, val
		} else {
			printf "{\"%s\":[\"%s\"],\"outbound\":\"direct-out\"},", key, val
		}
	}
' "$IN" | sed 's/,$//')

# TUN is enabled only when config.yaml asks for it. Clash spells that as a
# `tun:` mapping with an `enable:` child — the previous top-level
# `tun-enable:` probe never existed in a real config, so TUN silently never
# came up on the fallback converter.
TUN_ENABLE=$(awk '
	/^tun:/ { t=1; next }
	t && /^[^ \t#]/ { t=0 }
	t && /^[ \t]*enable:/ { print $2; found=1; exit }
	END { if (!found) print "0" }
' "$IN")
TUN_BLOCK=""
case "$TUN_ENABLE" in true|1|True|TRUE) TUN_BLOCK=',{"type":"tun","tag":"tun-in","address":["172.19.0.1/30"],"auto_route":true,"stack":"gvisor","strict_route":false}' ;; esac

cat > "$OUT" <<EOF
{
  "log": {"level": "info"},
  "inbounds": [
    {"type": "mixed", "tag": "mixed-in", "listen": "0.0.0.0", "listen_port": $MIXED_PORT},
    {"type": "redirect", "tag": "redir-in", "listen": "0.0.0.0", "listen_port": $REDIR_PORT},
    {"type": "tproxy", "tag": "tproxy-in", "listen": "0.0.0.0", "listen_port": $TPROXY_PORT}$TUN_BLOCK
  ],
  "outbounds": [
    {"type": "direct", "tag": "direct-out"}
  ],
  "route": {
    "rules": [$RULES],
    "final": "direct-out",
    "auto_detect_interface": true
  },
  "experimental": {
    "clash_api": {"external_controller": "$CONTROLLER", "default_mode": "$MODE"}
  }
}
EOF

exit 0
