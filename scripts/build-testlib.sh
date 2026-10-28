#!/usr/bin/env bash
# Builds build/libevmvx.so for the Python unit tests, with stub host callbacks.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/build-lib.sh" "${1:-2}" >/dev/null
gcc -shared -fPIC "$ROOT/build/evmvx-O${1:-2}.o" "$ROOT/build/atomic_shim.o" "$ROOT/build/alloc_shim.o" \
  "$ROOT/tests/support/stub_host.c" -o "$ROOT/build/libevmvx.so"
echo "built $ROOT/build/libevmvx.so"
