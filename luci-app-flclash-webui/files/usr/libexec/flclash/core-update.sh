#!/bin/sh
# /usr/libexec/flclash/core-update.sh — fetch the latest static mihomo arm64
# build from upstream and install it. Run detached by rpcd (flclash.core_update);
# progress goes to /tmp/flclash-update.log, final state to /tmp/flclash-update.state
# (running → done | failed). LuCI polls core_update_status.

STATE=/tmp/flclash-update.state
set_state() { echo "$1" > "$STATE"; }

fail() {
	echo "[update] FAILED: $1"
	set_state failed
	exit 1
}

echo "[update] $(date) start"
API='https://api.github.com/repos/MetaCubeX/mihomo/releases/latest'
META=$(curl -sS -m 30 -H 'Accept: application/vnd.github+json' "$API") || fail "cannot reach GitHub API"
ASSET_URL=$(printf '%s' "$META" | grep -o '"browser_download_url": *"https://[^"]*mihomo-linux-arm64-v[0-9][^"]*\.gz"' | head -n1 | sed 's/.*"\(https[^"]*\)"/\1/')
[ -n "$ASSET_URL" ] || fail "no mihomo-linux-arm64 asset in latest release"
echo "[update] asset: $ASSET_URL"

echo "[update] downloading…"
curl -sSL -m 300 -o /tmp/mihomo-new.gz "$ASSET_URL" || fail "download failed"
[ -s /tmp/mihomo-new.gz ] || fail "downloaded file is empty"

gunzip -f /tmp/mihomo-new.gz || fail "gunzip failed"
chmod 755 /tmp/mihomo-new
NEWVER=$(/tmp/mihomo-new -v 2>/dev/null | head -n1)
[ -n "$NEWVER" ] || fail "new binary does not execute"
echo "[update] new core: $NEWVER"

echo "[update] installing…"
/etc/init.d/flclash-core stop
# Give the old process time to release the proxy ports before the new
# binary binds them (v1.19+ honours the YAML ports and would hit EADDRINUSE).
sleep 3
cp -a /usr/bin/mihomo /tmp/mihomo-old-$(date +%s)
mv /tmp/mihomo-new /usr/bin/mihomo
/etc/init.d/flclash-core start
sleep 3

FINAL=$(/usr/bin/mihomo -v 2>/dev/null | head -n1)
echo "[update] installed: $FINAL"
echo "[update] done"
set_state done
