#!/usr/bin/env bash
# Builds build/libevmvx.a from the Vx sources listed in src/evm/MANIFEST.
#
# The files are concatenated into one module (build/evmvx.vx) because the
# pinned compiler cannot index a pointer to a struct whose field type comes
# from another module. `import evm::...` lines are dropped; other imports are
# kept once, at the top.
#
# Usage: scripts/build-lib.sh [opt-level 0|1|2|3]   (default 2)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPT="${1:-2}"
OUT="$ROOT/build"
mkdir -p "$OUT"
SRC="$OUT/evmvx.vx"
{
  grep -hE '^import ' $(sed "s|^|$ROOT/src/evm/|" "$ROOT/src/evm/MANIFEST") \
    | grep -v '^import evm::' | sort -u
  while read -r f; do
    [ -z "$f" ] && continue
    echo "// ---- src/evm/$f"
    grep -vE '^import ' "$ROOT/src/evm/$f"
  done < "$ROOT/src/evm/MANIFEST"
} > "$SRC"
"$ROOT/scripts/vxbuild" "$SRC" "$OUT/evmvx-O$OPT.o" "$OPT"
# CPU-only atomics for the Vx scheduler; a device build replaces this shim.
cc -O2 -fPIC -c "$ROOT/src/native/atomic_shim.c" -o "$OUT/atomic_shim.o"
cc -O2 -fPIC -c "$ROOT/src/native/alloc_shim.c" -o "$OUT/alloc_shim.o"
rm -f "$OUT/libevmvx.a"
ar rcs "$OUT/libevmvx.a" "$OUT/evmvx-O$OPT.o" "$OUT/atomic_shim.o" "$OUT/alloc_shim.o"
echo "built $OUT/libevmvx.a (O$OPT)"
