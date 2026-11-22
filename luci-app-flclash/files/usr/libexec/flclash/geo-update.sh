#!/bin/sh
# /usr/libexec/flclash/geo-update.sh — install sing-box binary rule-sets.
#
# Usage: geo-update.sh [sing-box]
#
# mihomo reads GeoIP.dat/GeoSite.dat; sing-box reads its own .srs files. The
# converter references /etc/flclash/{geoip-cn.srs,geosite-cn.srs} for
# GEOSITE,cn / GEOIP,CN rules. This script downloads both from jsDelivr
# (optionally behind flclash.update.mirror), validates each file with
# `sing-box rule-set match`, and replaces the previous copy atomically so a
# failed download can never leave a truncated rule-set behind.
set -u

CORE="${1:-sing-box}"
DEST_DIR=${GEO_DIR:-/etc/flclash}
LOG=/tmp/flclash-geo.log
STATE=/tmp/flclash-geo.state
SB=${SING_BOX:-/usr/bin/sing-box}
GEOIP_URL=${GEOIP_URL:-https://cdn.jsdelivr.net/gh/SagerNet/sing-geoip@rule-set/geoip-cn.srs}
GEOSITE_URL=${GEOSITE_URL:-https://cdn.jsdelivr.net/gh/SagerNet/sing-geosite@rule-set-unstable/geosite-cn.srs}

umask 077

say() {
	printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG"
}

fail() {
	say "[geo] FAILED: $1" >&2
	echo failed > "$STATE"
	exit 1
}

case "$CORE" in
	sing-box) ;;
	*) fail "unsupported core: $CORE (expected sing-box)" ;;
esac

# One updater at a time: two runs would race on the same temporary names and
# on the final rename. mkdir is atomic on every OpenWrt filesystem.
LOCK=/tmp/flclash-geo.lock
if ! mkdir "$LOCK" 2>/dev/null; then
	oldpid=$(cat "$LOCK/pid" 2>/dev/null)
	if [ -n "$oldpid" ] && kill -0 "$oldpid" 2>/dev/null; then
		say "[geo] another update is already running (pid $oldpid)" >&2
		exit 3
	fi
	rm -rf "$LOCK"
	mkdir "$LOCK" 2>/dev/null || fail "cannot acquire update lock"
fi
echo $$ > "$LOCK/pid"
trap 'rm -rf "$LOCK"' EXIT INT TERM
: > "$LOG"
echo running > "$STATE"

mkdir -p "$DEST_DIR" || fail "cannot create $DEST_DIR"
chmod 0700 "$DEST_DIR" 2>/dev/null

# Optional asset mirror prefix, same UCI key as the core updater.
MIRROR=$(uci -q get flclash.update.mirror 2>/dev/null)
case "$MIRROR" in
	http://*|https://*) ;;
	*) MIRROR="" ;;
esac
mirror_url() {
	if [ -n "$MIRROR" ]; then
		echo "${MIRROR%/}/$1"
	else
		echo "$1"
	fi
}

download() {
	local url tmp
	url=$(mirror_url "$1")
	tmp="$2"
	say "[geo] fetching $url"
	if ! curl -fsSL --connect-timeout 10 -m 90 -o "$tmp" "$url" 2>>"$LOG"; then
		rm -f "$tmp"
		return 1
	fi
	[ -s "$tmp" ] || {
		rm -f "$tmp"
		return 1
	}
	return 0
}

# `rule-set match` fully parses the binary set; an HTML error page or a
# truncated download fails here instead of at the next core start.
validate() {
	[ -x "$SB" ] || return 0
	"$SB" rule-set match -f binary "$1" 192.0.2.1 >/dev/null 2>&1
}

install_set() {
	local name="$1" url="$2"
	local tmp="$DEST_DIR/.$name.srs.$$"
	local dest="$DEST_DIR/$name.srs"
	download "$url" "$tmp" || fail "download failed: $name"
	validate "$tmp" || {
		rm -f "$tmp"
		fail "invalid rule-set: $name"
	}
	sync
	if ! mv -f "$tmp" "$dest"; then
		rm -f "$tmp"
		fail "cannot replace $dest"
	fi
	chmod 0644 "$dest" 2>/dev/null
	say "[geo] installed $dest ($(wc -c < "$dest") bytes)"
}

install_set geoip-cn "$GEOIP_URL"
install_set geosite-cn "$GEOSITE_URL"

echo done > "$STATE"
say "[geo] done"
exit 0
