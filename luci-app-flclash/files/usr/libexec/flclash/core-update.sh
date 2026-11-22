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

# Only one updater at a time: two concurrent runs would race on the same
# /tmp paths and on the binary rename. mkdir is atomic on every OpenWrt FS.
LOCK=/tmp/flclash-core-update.lock
if ! mkdir "$LOCK" 2>/dev/null; then
	oldpid=$(cat "$LOCK/pid" 2>/dev/null)
	if [ -n "$oldpid" ] && kill -0 "$oldpid" 2>/dev/null; then
		# Do NOT touch the state file: the running update owns it.
		echo "[update] another update is already running (pid $oldpid)"
		exit 3
	fi
	rm -rf "$LOCK"
	mkdir "$LOCK" 2>/dev/null || {
		echo "[update] cannot acquire update lock"
		exit 3
	}
fi
echo $$ > "$LOCK/pid"
trap 'rm -rf "$LOCK"' EXIT INT TERM

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

# Optional download mirror/prefix (e.g. https://ghproxy.example/). The GitHub
# API call above is still direct; only the asset (and its .sha256) is proxied.
MIRROR=$(uci -q get flclash.update.mirror 2>/dev/null)
case "$MIRROR" in
	http://*|https://*) ASSET_URL="${MIRROR%/}/$ASSET_URL" ;;
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

# --- integrity (BEFORE any execution) --------------------------------------
# GitHub releases usually publish "<asset>.sha256". Verify when present; when
# absent, say so explicitly instead of silently trusting the transport. The
# previous code executed whatever arrived and replaced /usr/bin with it; the
# version probe below now only runs on a verified (or explicitly unverified)
# artifact, never before the check.
if command -v sha256sum >/dev/null 2>&1; then
	if curl -fsSL -m 30 -o /tmp/flclash-core.sha256 "${ASSET_URL}.sha256" 2>/dev/null \
		&& [ -s /tmp/flclash-core.sha256 ]; then
		WANT=$(awk '{print $1}' /tmp/flclash-core.sha256 | head -n1)
		GOT=$(sha256sum /tmp/flclash-core-new.bin | awk '{print $1}')
		[ -n "$WANT" ] && [ "$WANT" = "$GOT" ] || fail "sha256 mismatch (expected $WANT, got $GOT)"
		echo "[update] sha256 verified"
	else
		echo "[update] WARNING: no .sha256 asset found; integrity relies on HTTPS"
	fi
else
	echo "[update] WARNING: sha256sum unavailable; integrity relies on HTTPS"
fi

NEWVER=$("$TMP" -v 2>/dev/null | head -n1)
[ -n "$NEWVER" ] || NEWVER=$("$TMP" version 2>/dev/null | head -n1)
[ -n "$NEWVER" ] || fail "new binary does not execute"
echo "[update] new core: $NEWVER"

# --- install with rollback -------------------------------------------------
# core_type must be persisted BEFORE the restart, otherwise `init.d start`
# still launches the previous core and the freshly downloaded one is ignored.
OLD_CORE=$(uci -q get flclash.main.core_type 2>/dev/null)
OLD_MODE=$(uci -q get flclash.main.core_mode 2>/dev/null)
uci -q set flclash.main.core_type="$CORE" 2>/dev/null
if [ "$CORE" = "mihomo" ]; then
	uci -q set flclash.main.core_mode="cli" 2>/dev/null
fi
uci -q commit flclash 2>/dev/null

core_ready() {
	local port
	port=$(uci -q get flclash.main.external_controller 2>/dev/null)
	port=${port##*:}
	[ -n "$port" ] || port=9097
	netstat -lnt 2>/dev/null | grep -q ":$port "
}

wait_core_ready() {
	local i=0
	while [ $i -lt 10 ]; do
		core_ready && return 0
		sleep 1
		i=$((i+1))
	done
	return 1
}

echo "[update] installing…"
# Refuse to stop a working core unless the target filesystem can hold the new
# binary plus a rollback copy.
NEED_BYTES=$(wc -c < "$TMP" 2>/dev/null || echo 0)
AVAIL_KB=$(df -k "$(dirname "$DEST")" 2>/dev/null | awk 'NR==2 {print $4}')
if [ -n "$AVAIL_KB" ] && [ "$AVAIL_KB" -lt $((NEED_BYTES / 1024 * 2 + 4096)) ] 2>/dev/null; then
	fail "not enough space on $(dirname "$DEST") for $((NEED_BYTES / 1024)) KiB core + rollback"
fi
[ -x "$INIT" ] && "$INIT" stop >/dev/null 2>&1
sleep 3
BACKUP=""
[ -f "$DEST" ] && {
	BACKUP="${DEST}.old-$(date +%s)"
	cp -a "$DEST" "$BACKUP"
}
# /tmp is tmpfs and $DEST is overlay: mv across filesystems degrades to
# copy+unlink and a power cut leaves a truncated core. Copy to a sibling
# (same filesystem), sync, then rename atomically.
NEWDEST="${DEST}.new-$$"
cp "$TMP" "$NEWDEST" || fail "cannot stage $NEWDEST"
chmod 755 "$NEWDEST"
sync
mv "$NEWDEST" "$DEST" || { rm -f "$NEWDEST"; fail "cannot replace $DEST"; }
[ -x "$INIT" ] && "$INIT" start >/dev/null 2>&1

if ! wait_core_ready; then
	echo "[update] new core did not become reachable; rolling back"
	if [ -n "$BACKUP" ]; then
		"$INIT" stop >/dev/null 2>&1
		sleep 1
		ROLLBACK="${DEST}.rollback-$$"
		cp -a "$BACKUP" "$ROLLBACK" && sync && mv "$ROLLBACK" "$DEST"
		[ -n "$OLD_CORE" ] && uci -q set flclash.main.core_type="$OLD_CORE" 2>/dev/null
		[ -n "$OLD_MODE" ] && uci -q set flclash.main.core_mode="$OLD_MODE" 2>/dev/null
		uci -q commit flclash 2>/dev/null
		"$INIT" start >/dev/null 2>&1
	fi
	fail "new core failed to start"
fi

# Keep at most the just-replaced binary as a manual rollback; old timestamped
# copies used to accumulate ~66MB each on flash.
for old in "$DEST".old-*; do
	[ -e "$old" ] || continue
	[ "$old" = "$BACKUP" ] || rm -f "$old"
done

FINAL=$("$DEST" -v 2>/dev/null | head -n1)
[ -n "$FINAL" ] || FINAL=$("$DEST" version 2>/dev/null | head -n1)
echo "[update] installed: $FINAL"
# sing-box reads its own .srs rule-sets; refresh the CN pair in the background
# so a slow CDN does not hold the "done" state hostage. The core starts fine
# without them (GEOSITE/GEOIP rules are skipped until they exist).
if [ "$CORE" = "sing-box" ] && [ -x /usr/libexec/flclash/geo-update.sh ]; then
	echo "[update] refreshing sing-box geo rule-sets in background"
	( /usr/libexec/flclash/geo-update.sh sing-box >/dev/null 2>&1 ) &
fi
echo "[update] done"
set_state done
