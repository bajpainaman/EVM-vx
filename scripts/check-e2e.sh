#!/usr/bin/env bash
# Build the existing native engine and compare Cancun transactions with revm.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export VXC="${VXC:-/data/fast/vx-toolchain/vxc}"
LLVM="${LLVM_BIN:-/usr/lib/llvm-22/bin}"
for tool in "$VXC" "$LLVM/mlir-translate" "$LLVM/opt" "$LLVM/llc" cargo rustc cc ar sha256sum flock; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing tool: $tool. Set VXC to the fixed Vx 0.0.3 wrapper and LLVM_BIN to LLVM 22's bin directory; Rust/C tools must be on PATH." >&2
    exit 2
  fi
done
mkdir -p "$ROOT/build"
exec 9>"$ROOT/build/e2e.lock"
flock 9
export VX_NO_AST_FALLBACK=1
export ETO_BUILD_JOBS="${ETO_BUILD_JOBS:-4}"
export CARGO_BUILD_JOBS="$ETO_BUILD_JOBS"
if (( ETO_BUILD_JOBS < 1 || ETO_BUILD_JOBS > 4 )); then
  echo "Use 1–4 build jobs on the shared runner." >&2; exit 2
fi
echo "Vx wrapper (not necessarily the compiler binary):"
sha256sum "$VXC"
version="$("$VXC" --version)"
echo "$version"
if [[ "$version" == *"0.0.2"* ]]; then
  echo "Stock Vx 0.0.2 is not accepted. Select the fixed compiler with VXC; see docs/SPEC.md for its tested digest." >&2
  exit 2
fi
if [[ -n "${VXC_SHA256:-}" && -z "${VXC_BINARY:-}" ]]; then
  echo "Set VXC_BINARY when specifying VXC_SHA256; the wrapper is not the compiler binary." >&2
  exit 2
fi
if [[ -n "${VXC_BINARY:-}" ]]; then
  echo "Compiler binary provenance:"
  sha256sum "$VXC_BINARY"
  if [[ -n "${VXC_SHA256:-}" ]]; then
    echo "$VXC_SHA256  $VXC_BINARY" | sha256sum --check
  fi
fi
"$LLVM/llc" --version | sed -n '1,3p'
rustc --version
for opt in 0 2; do
  echo "=== native Vx O$opt; AST fallback disabled ==="
  "$ROOT/scripts/build-lib.sh" "$opt"
  cargo test --locked --release --manifest-path "$ROOT/harness/Cargo.toml" -j "$ETO_BUILD_JOBS" e2e:: -- --nocapture --test-threads=1
  cargo build --locked --release --manifest-path "$ROOT/harness/Cargo.toml" -j "$ETO_BUILD_JOBS"
  BIN="${CARGO_TARGET_DIR:-$ROOT/harness/target}/release/evmvx-test"
  for seed in 1 8675309; do
    "$BIN" pfuzz --blocks 200 --txs 32 --threads 4 --seed "$seed"
  done
  # Optional runtime-supplied EEST/legacy state-test JSON: full observations
  # against revm in addition to the fixtures' independent state/log roots.
  if [[ -n "${EVMVX_FIXTURES:-}" ]]; then
    if [[ ! -e "$EVMVX_FIXTURES" ]]; then echo "Fixture path missing: $EVMVX_FIXTURES" >&2; exit 2; fi
    EVMVX_DIFF=1 "$BIN" statetest "$EVMVX_FIXTURES"
    EVMVX_DB=1 EVMVX_DIFF=1 "$BIN" statetest "$EVMVX_FIXTURES"
  fi
done
echo "O0/O2 Cancun end-to-end checks passed (revm 43.0.3, not evm2)."
