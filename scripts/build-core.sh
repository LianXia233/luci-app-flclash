#!/usr/bin/env bash
# Build the FlClash native-IPC core for OpenWrt master / SNAPSHOT.
# Produces CGO_ENABLED=0 static binaries, one per target architecture.
#
# IMPORTANT: FlClash's core is a gomobile/IPC library, NOT a standalone CLI.
# We build ./cmd/core (core.StartServer), which opens listeners/TUN only when a
# host drives the `listener.start` IPC method. The companion Rust binary
# flclash-core-bridge is that host. We deliberately do NOT patch the upstream
# executor.go (older builds commented out listener creation there): the IPC
# path drives listeners from core/lifecycle.go, so the source stays pristine.
#
# Usage: ./build-core.sh [output-dir] [go-binary]
set -eu

OUT="${1:-./luci-app-flclash/files/usr/bin}"
GO="${2:-go}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${ROOT}/src"
FLCLASH="${SRC}/FlClash"
CORE_ROOT="${FLCLASH}/core"          # go.mod: `module core`, replaces mihomo => ./Clash.Meta

if [ ! -d "$CORE_ROOT/cmd/core" ]; then
  echo "error: IPC core source not found at $CORE_ROOT/cmd/core" >&2
  echo "run: git clone --depth 1 https://github.com/chen08209/FlClash $FLCLASH" >&2
  echo "     cd $FLCLASH && git -c url.https://github.com/.insteadOf=git@github.com: submodule update --init --depth 1 core/Clash.Meta" >&2
  exit 1
fi

mkdir -p "$OUT"

echo "== Go toolchain =="
"$GO" version

build_one() {
  name="$1"; shift
  echo "== building IPC core: $name =="
  (
    cd "$CORE_ROOT"
    env CGO_ENABLED=0 GOOS=linux "$@" "$GO" build \
      -tags with_gvisor -ldflags '-w -s' \
      -o "$OUT/mihomo-$name" ./cmd/core
  )
}

# GOAMD64=v1 keeps x86_64 universally compatible (no AVX2 requirement).
build_one arm64 GOARCH=arm64 GOARM64=v8.0
build_one armv7 GOARCH=arm GOARM=7
build_one x86_64 GOARCH=amd64 GOAMD64=v1
build_one mipsle GOARCH=mipsle GOMIPS=softfloat

echo "== artifacts =="
ls -lh "$OUT"/mihomo-*
