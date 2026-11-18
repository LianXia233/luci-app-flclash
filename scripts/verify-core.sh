#!/usr/bin/env bash
# Verify prebuilt cores are compatible with OpenWrt master / SNAPSHOT.
# Usage: ./verify-core.sh [binary-dir]
set -eu

DIR="${1:-./flclash-core/files/usr/bin}"
FAIL=0

check() {
  label="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "  PASS  $label"
  else
    echo "  FAIL  $label"
    FAIL=1
  fi
}

echo "== OpenWrt master / SNAPSHOT compatibility =="

for f in "$DIR"/mihomo-*; do
  [ -e "$f" ] || continue
  echo "-- $(basename "$f") --"
  file "$f"

  check "ELF executable" sh -c "file '$f' | grep -q 'ELF'"
  check "statically linked" sh -c "file '$f' | grep -q 'statically linked' || file '$f' | grep -qv 'dynamically linked'"
  check "stripped (no symbols)" sh -c "file '$f' | grep -q 'stripped'"

  case "$(basename "$f")" in
    *arm64) check "ARM aarch64" sh -c "file '$f' | grep -q 'ARM aarch64'" ;;
    *armv7) check "ARM EABI5" sh -c "file '$f' | grep -q 'ARM, EABI5'" ;;
    *x86_64) check "x86-64" sh -c "file '$f' | grep -q 'x86-64'" ;;
    *mipsle) check "MIPS little-endian" sh -c "file '$f' | grep -qi 'MIPS.*LSB'" ;;
  esac

  if command -v readelf >/dev/null 2>&1; then
    check "no INTERP segment" sh -c "! readelf -l '$f' 2>/dev/null | grep -q INTERP"
    check "no NEEDED libs" sh -c "! readelf -d '$f' 2>/dev/null | grep -q NEEDED"
  fi

  if command -v go >/dev/null 2>&1; then
    echo -n "  INFO  "
    go version -m "$f" 2>/dev/null | grep -E 'go1\.|tags=' | head -4 | tr '\n' ' '
    echo
  fi
done

echo
if [ "$FAIL" -eq 0 ]; then
  echo "RESULT: all checks passed"
else
  echo "RESULT: some checks failed"
  exit 1
fi
