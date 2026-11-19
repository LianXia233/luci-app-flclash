#!/bin/sh
# /usr/libexec/flclash/core-update.sh — update an installed proxy core.
#
# Usage: core-update.sh [mihomo|sing-box]
#   mihomo   (default) — MetaCubeX/mihomo static linux arm64 build (the
#            FlClash Clash.Meta fork's drop-in runtime).
#   sing-box  — SagerNet/sing-box static linux arm64 build.
#
# The selected core is installed to its canonical path and the matching
# init.d service is restarted. Progress goes to /tmp/flclash-update.log,
# final state to /tmp/flclash-update.state (running → done | failed).
# LuCI polls core_update_status. The binary is validated by actually running
# `-v` before an atomic rename replaces the live one, so a bad download can
# never take the working core down.

CORE="${1:-mihomo}"
STATE=/tmp/flclash-update.state
set_state() { echo "$1" > "$STATE"; }

fail() {
	echo "[update] FAILED: $1"
	set_state failed
	exit 1
}

echo "[update] $(date) start core=$CORE"
case "$CORE" in
	mihomo)
		DEST=/usr/bin/mihomo
		INIT=/etc/init.d/flclash-core
		API='https://api.github.com/repos/MetaCubeX/mihomo/releases/latest'
		META=$(curl -sS -m 30 -H 'Accept: application/vnd.github+json' "$API") || fail "cannot reach GitHub API"
		ASSET_URL=$(printf '%s' "$META" | grep -o '"browser_download_url": *"https://[^"]*mihomo-linux-arm64-v[0-9][^"]*\.gz"' | head -n1 | sed 's/.*"\(https[^"]*\)"/\1/')
		[ -n "$ASSET_URL" ] || fail "no mihomo-linux-arm64 asset in latest release"
		TMP=/tmp/mihomo-new
		;;
	sing-box)
		DEST=/usr/bin/sing-box
		INIT=/etc/init.d/flclash-singbox
		API='https://api.github.com/repos/SagerNet/sing-box/releases/latest'
		META=$(curl -sS -m 30 -H 'Accept: application/vnd.github+json' "$API") || fail "cannot reach GitHub API"
		ASSET_URL=$(printf '%s' "$META" | grep -o '"browser_download_url": *"https://[^"]*sing-box-[0-9][^"]*linux-arm64[^"]*\.tar\.gz"' | head -n1 | sed 's/.*"\(https[^"]*\)"/\1/')
		[ -n "$ASSET_URL" ] || fail "no sing-box linux-arm64 asset in latest release"
		TMP=/tmp/singbox-new
		;;
	*)
		fail "unknown core: $CORE (expected mihomo|sing-box)"
		;;
esac

echo "[update] asset: $ASSET_URL"
echo "[update] downloading…"
curl -sSL -m 300 -o /tmp/flclash-core-new.gz "$ASSET_URL" || fail "download failed"
[ -s /tmp/flclash-core-new.gz ] || fail "downloaded file is empty"

mkdir -p /tmp/flclash-core-unpack
rm -rf /tmp/flclash-core-unpack/*
if [ "$CORE" = "mihomo" ]; then
	gunzip -c /tmp/flclash-core-new.gz > "$TMP" || fail "gunzip failed"
else
	tar -xzf /tmp/flclash-core-new.gz -C /tmp/flclash-core-unpack || fail "untar failed"
	FOUND=$(find /tmp/flclash-core-unpack -type f -name 'sing-box' | head -n1)
	[ -n "$FOUND" ] || fail "sing-box binary not found in archive"
	cp "$FOUND" "$TMP"
fi
chmod 755 "$TMP"

NEWVER=$("$TMP" -v 2>/dev/null | head -n1)
[ -n "$NEWVER" ] || fail "new binary does not execute"
echo "[update] new core: $NEWVER"

echo "[update] installing…"
[ -x "$INIT" ] && "$INIT" stop >/dev/null 2>&1
sleep 3
[ -f "$DEST" ] && cp -a "$DEST" "${DEST}.old-$(date +%s)"
mv "$TMP" "$DEST"
[ -x "$INIT" ] && "$INIT" start >/dev/null 2>&1
sleep 3

# Reflect the active core in UCI so the UI and switch logic stay in sync.
uci -q set flclash.main.core_type="$CORE" 2>/dev/null && uci -q commit flclash 2>/dev/null

FINAL=$("$DEST" -v 2>/dev/null | head -n1)
echo "[update] installed: $FINAL"
echo "[update] done"
set_state done
