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
# hash_hw.vx in MANIFEST is the hash permutations: on the CPU's SHA3/SHA2 instructions
# when the host has them (the Vx object targets the host CPU), else the scalar Vx ones.
HASH="${EVMVX_HASH:-}"
if [ -z "$HASH" ]; then
  if cc -mcpu=native -dM -E - </dev/null 2>/dev/null | grep -q '__ARM_FEATURE_SHA3 1' \
     && cc -mcpu=native -dM -E - </dev/null 2>/dev/null | grep -q '__ARM_FEATURE_SHA2 1'; then
    HASH=arm
  else
    HASH=portable
  fi
fi
case "$HASH" in arm|portable) ;; *) echo "EVMVX_HASH must be arm or portable, not '$HASH'" >&2; exit 1 ;; esac
src_of() { if [ "$1" = hash_hw.vx ]; then echo "$ROOT/src/evm/hash_hw_$HASH.vx"; else echo "$ROOT/src/evm/$1"; fi; }
{
  grep -hE '^import ' $(while read -r f; do [ -n "$f" ] && src_of "$f"; done < "$ROOT/src/evm/MANIFEST") \
    | grep -v '^import evm::' | sort -u
  while read -r f; do
    [ -z "$f" ] && continue
    echo "// ---- src/evm/$(basename "$(src_of "$f")")"
    grep -vE '^import ' "$(src_of "$f")"
  done < "$ROOT/src/evm/MANIFEST"
} > "$SRC"
"$ROOT/scripts/vxbuild" "$SRC" "$OUT/evmvx-O$OPT.o" "$OPT"
# Scheduling atomics are emitted by Vx; allocation remains a CPU runtime ABI.
cc -O2 -fPIC -c "$ROOT/src/native/alloc_shim.c" -o "$OUT/alloc_shim.o"
rm -f "$OUT/libevmvx.a"
ar rcs "$OUT/libevmvx.a" "$OUT/evmvx-O$OPT.o" "$OUT/alloc_shim.o"
echo "built $OUT/libevmvx.a (O$OPT, hash $HASH)"
