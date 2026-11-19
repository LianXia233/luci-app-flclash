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
set -eu

IN="${1:-/etc/flclash/config.yaml}"
OUT="${2:-/tmp/flclash-singbox.json}"

[ -f "$IN" ] || exit 1

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
CONTROLLER=$(scalar external-controller 127.0.0.1:9097)
MODE=$(scalar mode rule)

# Sanitize: numeric port, controller host:port, valid mode.
case "$MIXED_PORT" in ''|*[!0-9]*) MIXED_PORT=7892 ;; esac
[ -n "$CONTROLLER" ] || CONTROLLER=127.0.0.1:9097
case "$MODE" in rule|global|direct|script) ;; *) MODE=rule ;; esac

# Optional native base config supplied by the user (full node translation).
BASE="${IN}.singbox.json"
if [ -f "$BASE" ]; then
	# Patch managed scalars with sed (keys are unique in sing-box schema).
	sed -e "s/\"listen_port\"[[:space:]]*:[[:space:]]*[0-9]*/\"listen_port\": $MIXED_PORT/" \
		-e "s/\"external_controller\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"external_controller\": \"$CONTROLLER\"/" \
		-e "s/\"default_mode\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"default_mode\": \"$MODE\"/" \
		"$BASE" > "$OUT"
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

# Emit the sing-box config. TUN is enabled only when config.yaml asks for it.
TUN_ENABLE=$(scalar tun-enable 0)
TUN_BLOCK=""
if [ "$TUN_ENABLE" = "true" ] || [ "$TUN_ENABLE" = "1" ]; then
	TUN_BLOCK=',{"type":"tun","tag":"tun-in","address":"172.19.0.1/30","auto_route":true,"stack":"gvisor","route_address_set":[],"strict_route":false}'
fi

cat > "$OUT" <<EOF
{
  "log": {"level": "info"},
  "inbounds": [
    {"type": "mixed", "tag": "mixed-in", "listen": "0.0.0.0", "listen_port": $MIXED_PORT}$TUN_BLOCK
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
