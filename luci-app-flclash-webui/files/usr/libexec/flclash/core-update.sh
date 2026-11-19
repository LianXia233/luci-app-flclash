#!/bin/sh
# /usr/libexec/flclash/core-update.sh — update an installed proxy core.
#
# Usage: core-update.sh [mihomo|sing-box]
#   mihomo    (default) — MetaCubeX/mihomo static build (OpenWrt CLI runtime).
#   sing-box  — SagerNet/sing-box static musl build.
#
# The selected core is installed to its canonical path and the matching init
# service is restarted. Progress goes to /tmp/flclash-update.log, final state to
# /tmp/flclash-update.state (running -> done | failed). LuCI polls
# core_update_status. The binary is validated by running `-v`/`version` before
# an atomic rename replaces the live one, so a bad download cannot take the
# working core down.
#
# Architecture is detected from `uname -m` so the same script works on arm64,
# amd64, armv7 and mipsel devices (previously arm64 was hard-coded).

CORE="${1:-mihomo}"
STATE=/tmp/flclash-update.state
set_state() { echo "$1" > "$STATE"; }

fail() {
	echo "[update] FAILED: $1"
	set_state failed
	exit 1
}

# Map kernel arch to the asset tokens used by mihomo / sing-box releases.
case "$(uname -m)" in
	x86_64|amd64)   MIHOMO_ARCH=amd64;  SB_ARCH=amd64;  SB_LIB=musl ;;
	aarch64|arm64)  MIHOMO_ARCH=arm64;  SB_ARCH=arm64;  SB_LIB=musl ;;
	armv7l|armv6l|arm) MIHOMO_ARCH=armv7; SB_ARCH=armv7; SB_LIB=musl ;;
	mipsel|mipsle)  MIHOMO_ARCH=mipsle; SB_ARCH=mipsel; SB_LIB=musl ;;
	*) fail "unsupported architecture: $(uname -m)" ;;
esac
echo "[update] arch: mihomo=$MIHOMO_ARCH sing-box=$SB_ARCH/$SB_LIB"

echo "[update] $(date) start core=$CORE"
case "$CORE" in
	mihomo)
		DEST=/usr/bin/mihomo
		INIT=/etc/init.d/flclash-core
		API='https://api.github.com/repos/MetaCubeX/mihomo/releases/latest'
		META=$(curl -sS -m 30 -H 'Accept: application/vnd.github+json' "$API") || fail "cannot reach GitHub API"
		# Prefer the GOAMD64=v1 "compatible" amd64 asset (works on any x86_64).
		ASSET_URL=$(printf '%s' "$META" \
			| grep -o '"browser_download_url": *"https://[^"]*mihomo-linux-'"$MIHOMO_ARCH"'-v[0-9][^"]*\.gz"' \
			| grep -i compatible | head -n1 | sed 's/.*"\(https[^"]*\)"/\1/')
		[ -n "$ASSET_URL" ] || ASSET_URL=$(printf '%s' "$META" \
			| grep -o '"browser_download_url": *"https://[^"]*mihomo-linux-'"$MIHOMO_ARCH"'-v[0-9][^"]*\.gz"' \
			| head -n1 | sed 's/.*"\(https[^"]*\)"/\1/')
		[ -n "$ASSET_URL" ] || fail "no mihomo-linux-$MIHOMO_ARCH asset in latest release"
		MODE=cli
		;;
	sing-box)
		DEST=/usr/bin/sing-box
		INIT=/etc/init.d/flclash-core
		API='https://api.github.com/repos/SagerNet/sing-box/releases/latest'
		META=$(curl -sS -m 30 -H 'Accept: application/vnd.github+json' "$API") || fail "cannot reach GitHub API"
		ASSET_URL=$(printf '%s' "$META" \
			| grep -o '"browser_download_url": *"https://[^"]*sing-box-[0-9][^"]*linux-'"$SB_ARCH"'-'"$SB_LIB"'\.tar\.gz"' \
			| head -n1 | sed 's/.*"\(https[^"]*\)"/\1/')
		[ -n "$ASSET_URL" ] || fail "no sing-box linux-$SB_ARCH asset in latest release"
		MODE=singbox
		;;
	*)
		fail "unknown core: $CORE (expected mihomo|sing-box)"
		;;
esac

echo "[update] asset: $ASSET_URL"
echo "[update] downloading…"
curl -sSL -m 300 -o /tmp/flclash-core-new.bin "$ASSET_URL" || fail "download failed"
[ -s /tmp/flclash-core-new.bin ] || fail "downloaded file is empty"

TMP=/tmp/flclash-core-new
mkdir -p /tmp/flclash-core-unpack
rm -rf /tmp/flclash-core-unpack/*
if [ "$CORE" = "mihomo" ]; then
	gunzip -c /tmp/flclash-core-new.bin > "$TMP" || fail "gunzip failed"
else
	tar -xzf /tmp/flclash-core-new.bin -C /tmp/flclash-core-unpack || fail "untar failed"
	FOUND=$(find /tmp/flclash-core-unpack -type f -name 'sing-box' | head -n1)
	[ -n "$FOUND" ] || fail "sing-box binary not found in archive"
	cp "$FOUND" "$TMP"
fi
chmod 755 "$TMP"

NEWVER=$("$TMP" -v 2>/dev/null | head -n1)
[ -n "$NEWVER" ] || NEWVER=$("$TMP" version 2>/dev/null | head -n1)
[ -n "$NEWVER" ] || fail "new binary does not execute"
echo "[update] new core: $NEWVER"

echo "[update] installing…"
[ -x "$INIT" ] && "$INIT" stop >/dev/null 2>&1
sleep 3
[ -f "$DEST" ] && cp -a "$DEST" "${DEST}.old-$(date +%s)"
mv "$TMP" "$DEST"
[ -x "$INIT" ] && "$INIT" start >/dev/null 2>&1
sleep 3

# Keep UCI in sync: online mihomo is the standalone CLI (core_mode=cli); the
# prebuilt FlClash IPC core uses core_mode=ipc. sing-box selects its own core.
uci -q set flclash.main.core_type="$CORE" 2>/dev/null
if [ "$CORE" = "mihomo" ]; then
	uci -q set flclash.main.core_mode="cli" 2>/dev/null
fi
uci -q commit flclash 2>/dev/null

FINAL=$("$DEST" -v 2>/dev/null | head -n1)
[ -n "$FINAL" ] || FINAL=$("$DEST" version 2>/dev/null | head -n1)
echo "[update] installed: $FINAL"
echo "[update] done"
set_state done
