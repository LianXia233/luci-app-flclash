#!/usr/bin/env bash
# Build FlClash mihomo core for OpenWrt master / SNAPSHOT.
# Produces CGO_ENABLED=0 static binaries, one per target architecture.
# Usage: ./build-core.sh [output-dir] [go-binary]
set -eu

OUT="${1:-./flclash-core/files/usr/bin}"
GO="${2:-go}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${ROOT}/src"
CORE="${SRC}/FlClash/core/Clash.Meta"

if [ ! -d "$CORE" ]; then
  echo "error: source not found at $CORE" >&2
  echo "run: git clone --depth 1 https://github.com/chen08209/FlClash $SRC/FlClash" >&2
  echo "     cd $SRC/FlClash && git submodule update --init --depth 1 core/Clash.Meta" >&2
  exit 1
fi

mkdir -p "$OUT"

echo "== Go toolchain =="
"$GO" version

build_one() {
  name="$1"; shift
  echo "== building $name =="
  (
    cd "$CORE"
    env CGO_ENABLED=0 GOOS=linux "$@" "$GO" build \
      -tags with_gvisor -ldflags '-w -s' \
      -o "$OUT/mihomo-$name" .
  )
}

build_one arm64 GOARCH=arm64 GOARM64=v8.0
build_one armv7 GOARCH=arm GOARM=7
build_one x86_64 GOARCH=amd64 GOAMD64=v1
build_one mipsle GOARCH=mipsle GOMIPS=softfloat

echo "== artifacts =="
ls -lh "$OUT"/mihomo-*
