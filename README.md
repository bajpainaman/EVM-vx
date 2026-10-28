# EVM-vx

Execute ordered Ethereum transactions in parallel in [Vx](https://github.com/vx-lang/Vx), with sequential execution as the correctness reference.

**Research-stage CPU implementation:** a sequential EVM, an optimistic parallel transaction executor, and a Rust harness for Ethereum fixtures, differential testing, and benchmarks. The interpreter, transaction logic, state database, and parallel scheduler live in Vx. The host supplies threads and cryptographic backends for selected precompiles.

## What's built

| Component | Implementation |
|---|---|
| Sequential EVM | Fork-specific opcode and gas tables through Osaka, 256-bit arithmetic, journaled state and rollback, nested CALL/CREATE, transaction validation and settlement, and EIP-7702 authorizations. See [the transaction API](src/evm/api.vx) and [interpreter](src/evm/interp.vx). |
| Parallel executor | YOLO ownership: workers read an immutable base and own their attempts. After joining, one finalizer validates and repairs in transaction order, then publishes the complete block. No engine mutexes or spinlocks. See [block.vx](src/evm/block.vx) and [mvmem.vx](src/evm/mvmem.vx). |
| Vx state database | Accounts, storage, bytecode, and block hashes live in [StateDb](src/evm/db.vx). This path replaces host callbacks for state reads. |
| Verification harness | Official Ethereum state and blockchain fixtures, a conflict fuzzer, and benchmarks against revm. The dependency versions are pinned in [harness/Cargo.toml](harness/Cargo.toml) and [Cargo.lock](harness/Cargo.lock). |

The parallel executor uses **YOLO: You Only Lock Once**, implemented here through ownership with no engine lock acquisition. Attempts read the pre-block state and record dynamic dependencies privately. The finalizer keeps one final value per changed location; unchanged touched accounts do not create false dependencies. Touch, deletion, creation, and storage-generation effects retain their EVM semantics. Each stale transaction is repaired once against its exact prefix. Validation, repair, and publication are serial; independent first attempts execute in parallel. Fee-recipient rewards accumulate as deltas, and repairs receive exact prior fees even when a new branch discovers a fee-recipient read.

The raw block API requires exclusive host ownership of StateDb and stable environment buffers until every worker has joined. Call `finish` after joining; it is idempotent and publishes nothing for cancelled, incomplete, or invalid batches. Cancel before finalization and join before freeing. Cancellation stops new claims; an active transaction finishes its bounded EVM execution. Private logs grow dynamically; allocation failure retains the existing process-failure behavior. See [the ownership contract and evidence](docs/YOLO.md).

Block preparation, system calls, withdrawals, rewards, and receipt construction are handled by the [Rust blockchain-test adapter](harness/src/bctest.rs). The Vx block API executes the ordered transaction batch.

## Build and run a conflict test

The first check runs generated blocks through **revm sequential, Vx sequential, and Vx parallel**. It compares outcomes, gas/refunds, output, created addresses, logs, and final account/code/storage snapshots. Cases include shared senders and storage, transfers, reverts, nested calls, contract creation, and fee-recipient balance reads.

Prerequisites:

- The fixed Vx compiler: upstream `dc6a460` plus the local i128-guard and struct-pointer fixes described in the [feasibility report](docs/research/feasibility.md#8-compiler-defects-fixes-and-upstream-issues). The compiler and patches are not bundled here; stock `vxc 0.0.2` is not equivalent.
- LLVM 22 with `mlir-translate`, `opt`, and `llc`; a C compiler and archiver; Rust and Cargo.

The scripts default to `/data/fast/vx-toolchain/vxc` for the fixed compiler and `/usr/lib/llvm-22/bin` for LLVM. Set `VXC` to your fixed compiler executable and `LLVM_BIN` to your LLVM 22 binary directory if installed elsewhere. See [Vx build notes](docs/VX_NOTES.md) for compiler workarounds.

Run all commands below from the repository root:

```sh
scripts/build-lib.sh 2
cargo build --release --locked --manifest-path harness/Cargo.toml
harness/target/release/evmvx-test pfuzz --blocks 200 --txs 64 --threads 8 --seed 1
```

The build produces `build/libevmvx.a` and links it into `harness/target/release/evmvx-test`. A successful fuzzer run prints `revm == vx-seq == vx-par` and reports transaction, re-execution, and fee-recipient rerun counts. Base-snapshot speculation makes retry decisions independent of worker scheduling. Run directed ownership and lifecycle checks with `cargo test --release --locked --manifest-path harness/Cargo.toml`.

## Run Ethereum fixtures

Fixture fetching needs Python 3.12 or newer. The fetch script downloads EEST v5.4.0 develop/stable archives, a pinned Glamsterdam devnet archive, and the legacy tests into the directory you supply:

```sh
python3 scripts/fetch-fixtures.py build/fixtures
harness/target/release/evmvx-test statetest build/fixtures/main/develop/fixtures/state_tests/cancun
harness/target/release/evmvx-test btest build/fixtures/main/develop/fixtures/blockchain_tests/cancun
```

Exercise the Vx state database and the parallel block path separately:

```sh
EVMVX_DB=1 harness/target/release/evmvx-test statetest build/fixtures/main/develop/fixtures/state_tests/cancun
EVMVX_BLOCK=8 harness/target/release/evmvx-test btest build/fixtures/main/develop/fixtures/blockchain_tests/cancun
```

`EVMVX_BLOCK` selects the worker count and attaches a StateDb. Eligible blocks use the parallel executor; invalid-block cases and aborted attempts use the sequential path. The runner prints parallel-block and fallback counts. `EVMVX_DIFF=1` adds a revm comparison when a state test's final state root differs.

Missing paths and zero-case runs fail. State tests check expected invalidity, state roots, and logs hashes. Blockchain tests check gas and receipt roots where applicable, plus fixture post-state. Skip lists and fork exclusions are in [main.rs](harness/src/main.rs) and [bctest.rs](harness/src/bctest.rs).

### Reported fixture results — 2026-10-26

These sequential-run totals were recorded before the parallel executor was added. They are retained as historical results; validation of the current parallel path requires its own run and fallback counts.

| Suite | Passed | Failed |
|---|---:|---:|
| EEST develop state tests, Frontier to Osaka | 63,314 | 0 |
| EEST develop blockchain tests | 61,391 | 0 |
| Legacy Cancun state tests | 108,134 | 0 |
| Legacy Constantinople state tests | 20,690 | 0 |

The totals reflect the runners' skip policies. Amsterdam/devnet results are not recorded here.

### Arithmetic and precompile checks

The shared test library exposes Keccak and precompile checks. The Keccak oracle requires `pycryptodome`; the commands below install it in a local virtual environment.

```sh
scripts/build-testlib.sh 2
python3 -m venv build/test-venv
build/test-venv/bin/python -m pip install pycryptodome
build/test-venv/bin/python tests/keccak/run.py
build/test-venv/bin/python tests/precompiles/test_modexp.py
build/test-venv/bin/python tests/precompiles/test_blake2f.py
```

The [U256 differential runner](tests/u256/run.py) independently builds a Vx driver and checks 25 word operations against Python integers:

```sh
python3 tests/u256/run.py --smoke -O 2
```

That driver also needs the fixed compiler's `libvx_std_core` runtime library. Set `CLANG` to your Clang executable and `VX_RUNTIME_LIB_DIR` to the directory containing that library if the defaults in the runner do not match your installation. Omit `--smoke` for the larger corpus; omit `-O 2` to check optimization levels 0, 2, and 3.

## Measure performance

Two benchmark modes are implemented:

```sh
harness/target/release/evmvx-test bench build/fixtures/main/develop/fixtures/state_tests/cancun --reps 3
harness/target/release/evmvx-test pexec --txs 2000 --contracts 2000 --threads 8 --reps 3
```

- [`bench`](harness/src/bench.rs) times sequential execution and commit for parsed state-test cases in Vx and revm. It reports `vx/revm` elapsed time: values above 1 mean Vx took longer. Run conformance checks separately; this mode collects timings.
- [`pexec`](harness/src/pexec.rs) runs storage, arithmetic, Keccak, transfer, CALL, or revert workloads (`--workload NAME`). It verifies all three engines, including state and receipt roots, outside the execution timers. Reduce `--contracts` to increase contention. `--json` preserves timing samples and exact retry keys, writers, and reasons.

The state backends differ: sequential Vx uses Rust host maps, revm uses its own state adapters, and parallel Vx uses StateDb. Interpret results with the workload, worker count, backend, compiler, and machine specified. The earlier [Vx/C kernel measurements](docs/research/performance.md) cover arithmetic and toy dispatch; the 20× target remains a research goal.

## Precompiles and the GPU path

Vx implements ecrecover, SHA-256, RIPEMD-160, identity, MODEXP, BLAKE2F, and P-256 verification. bn254 (`0x06`–`0x08`), KZG (`0x0a`), and BLS12-381 (`0x0b`–`0x11`) use the stateless `evmvx_crypto_precompile` callback, backed by revm's precompiles in the harness. These crypto callbacks remain active with StateDb attached.

The interpreter uses an explicit frame stack for CALL/CREATE, and allocations go through four functions in [libc.vx](src/evm/libc.vx). [Native Vx atomics](src/evm/atomic.vx) claim work and publish completion. The [per-thread allocator](src/native/alloc_shim.c) transfers idle heaps through atomic ownership claims over a permanent registry; it has no mutex or spinlock. CPU allocation and external crypto still need device implementations, plus evidence that the Vx GPU backend can compile and run the interpreter.

## Read the code and design

| Path | Start here for |
|---|---|
| [src/evm/abi.vx](src/evm/abi.vx), [api.vx](src/evm/api.vx), [Rust FFI](harness/src/ffi.rs) | Execution inputs, result ownership, host callbacks, and C ABI mirrors. |
| [interp.vx](src/evm/interp.vx), [state.vx](src/evm/state.vx), [call.vx](src/evm/call.vx) | Opcode execution, journaled state, and nested calls/creation. |
| [block.vx](src/evm/block.vx), [mvmem.vx](src/evm/mvmem.vx), [db.vx](src/evm/db.vx) | Private attempts, final prefix state, and whole-block publication. |
| [harness/src/pfuzz.rs](harness/src/pfuzz.rs) | Three-way conflict testing against revm. |
| [docs/VX_NOTES.md](docs/VX_NOTES.md) | Build constraints and compiler workarounds. |
| [docs/SPEC.md](docs/SPEC.md) | The original draft execution contract, invariants, and acceptance obligations. Some proposed mechanisms differ from the current code; use the source for implemented behavior. |
| [Feasibility](docs/research/feasibility.md) and [performance](docs/research/performance.md) reports | Original compiler, embedding, concurrency, and kernel experiments. Their raw artifacts remain in the author's local research archive. |

Next work: publish reproducible parallel conformance and conflict-test results, measure representative blocks across contention levels and worker counts, package the fixed toolchain, and complete the device-port prerequisites. Start with [the conflict fuzzer](harness/src/pfuzz.rs), [the block executor](src/evm/block.vx), or [the Vx build notes](docs/VX_NOTES.md).
