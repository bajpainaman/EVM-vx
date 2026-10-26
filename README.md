# EVM-vx

A research-stage project to execute Ethereum transactions in parallel in [Vx](https://github.com/vx-lang/Vx), while preserving the results of sequential execution.

**Current status: a working sequential EVM, checked against the official Ethereum state tests.** `src/evm/` holds an EVM written in Vx: every opcode for Frontier through Osaka, per-fork gas, a journaled state with revert, CALL/CREATE, the transaction handler, EIP-7702 authorizations and most precompiles. `harness/` runs the EEST state tests and blockchain tests through it and checks state and logs roots the way revm's `revme statetest` and `revme btest` do.

Three choices let the core move to a GPU or other device without a rewrite:

- **No native recursion.** CALL and CREATE suspend the calling frame, and `run_frame` runs every frame on an explicit stack. The native stack depth stays the same at any call depth.
- **No host calls during execution.** With a `StateDb` attached (`evmvx_transact_db`), the engine reads accounts, storage and block hashes from Vx memory and never calls back into the host.
- **One allocator.** Every allocation goes through `vx_malloc`, `vx_calloc`, `vx_realloc` and `vx_free` in `src/evm/libc.vx`, so a device build can swap in an arena by changing one file.

The pairing-based precompiles (bn254 at 0x06–0x08, KZG at 0x0a, BLS12-381 at 0x0b–0x11) come from a stateless crypto provider, `evmvx_crypto_precompile`. That matches revm, which takes them from its `crypto()` backends. The harness backs the provider with revm's own precompiles. Every other precompile is written in Vx.

Not yet done: the parallel executor, and native Vx versions of the pairing precompiles. See [What's next](#whats-next). No performance claim is made; the 20× goal is still an ambition.

## Test results (2026-10-26)

Every case passes against the official Ethereum fixtures, with the roots checked the way `revme` checks them:

| Suite | Passed | Failed |
|---|---|---|
| EEST develop state tests, Frontier to Osaka | 63,314 | 0 |
| EEST develop blockchain tests | 61,391 | 0 |
| Legacy Cancun state tests | 108,134 | 0 |
| Legacy Constantinople state tests | 20,690 | 0 |

The skip list is revm's own (`skip_test` in `harness/src/main.rs`). Amsterdam (devnet) fixtures are not run yet.

## What's next

In order:

1. **Three open decisions.** Each takes a few minutes.
   - Merge `vx-evm` into `main` through a pull request.
   - Report the Vx byte-index miscompile upstream. A read such as `buf[135]` through a `*mut u8` reads `buf[-121]`. The workaround is in [docs/VX_NOTES.md](docs/VX_NOTES.md).
   - Run the Amsterdam devnet fixtures. Amsterdam is not on mainnet yet.
2. **M4: the parallel executor.** The parallel executor is the project's primary goal: Block-STM-style execution with multi-version reads, parallel validation and fee-recipient deltas, plus a benchmark against revm. It builds on the explicit frame stack and the `StateDb`. Expect several days of work.
3. **Native Vx pairing precompiles.** bn254, KZG and BLS12-381 still run revm's code through `evmvx_crypto_precompile`, which is a host call. They must move into Vx before any GPU work.
4. **M5: the GPU step.** It depends on items 2 and 3. It also needs a test kernel to show whether the Vx GPU backend can compile the interpreter loop. Its GPU model today is `across` loops in `spawn on` regions.

## Run the tests

The tests need the fixed Vx compiler (upstream dc6a460 plus the local fixes in [section 8 of the feasibility report](docs/research/feasibility.md#8-compiler-defects-fixes-and-upstream-issues)), LLVM/Clang 22, Rust and Python 3. Set `VXC` to the compiler wrapper if it is not at the default path in `scripts/vxc`. Compiler quirks the code works around are in [docs/VX_NOTES.md](docs/VX_NOTES.md).

```sh
scripts/build-lib.sh 2                           # builds build/libevmvx.a
python3 scripts/fetch-fixtures.py                # EEST v5.4.0, devnet and legacy fixtures
cd harness && cargo build --release
target/release/evmvx-test statetest <fixtures>/main/develop/fixtures/state_tests/cancun
EVMVX_DIFF=1 target/release/evmvx-test statetest <dir>   # on failure, diff against revm
target/release/evmvx-test btest <fixtures>/main/develop/fixtures/blockchain_tests/cancun
EVMVX_DB=1 target/release/evmvx-test btest <dir>         # run on a Vx StateDb, no host callbacks

scripts/build-testlib.sh 2                       # unit-test library with stub host callbacks
python3 tests/u256/run.py                        # 25 word opcodes vs. a Python-int oracle
python3 tests/keccak/run.py                      # Keccak-256 vs. pycryptodome
python3 tests/precompiles/test_modexp.py         # also test_blake2f.py
```

## Start here

| Document | Read it for |
|---|---|
| [Draft specification](docs/SPEC.md) | Execution semantics, host boundaries, candidate parallel state machine, safety invariants, and acceptance criteria. |
| [Vx feasibility findings](docs/research/feasibility.md) | Cited compiler/runtime research, five native experiments, arithmetic, workers, ABI, memory, and two local compiler fixes. |
| [Matched performance findings](docs/research/performance.md) | Vx/C arithmetic and toy dispatch measurements, correctness checks, methodology, and limitations. |

The reports describe experiments executed outside this repository; raw harnesses, logs, and compiler builds remain in the author's local research archive. This initial publication contains the spec and findings, not a self-contained benchmark package.

## Project direction

The proposed design has two layers:

1. A correct sequential EVM core, checked against an executable reference for one explicitly selected Ethereum fork.
2. Bounded CPU workers that speculate on transactions using private state changes, validate dependencies, and publish results in input order. Conflicts trigger retries or sequential fallback.

Parallel execution must preserve state, gas accounting, transaction results, logs, and rollback behavior. A host resource failure must never masquerade as an EVM revert or out-of-gas result.

The spec borrows MIT Spec's state/transition/refinement discipline in readable Markdown. A pinned Ethereum reference and conformance tests should define fork-specific behavior. No formal proof, final scheduler, or interpreter/JIT choice is claimed.

## What the research established

- **Word arithmetic:** exact 256-bit limb algorithms ran natively in Vx, using 128-bit intermediates. This does not require a primitive `u256` type.
- **Boundary semantics:** guarded division and shifts matched independent Python expectations, including zero divisors, signed boundaries, and full-width shift counts.
- **CPU concurrency:** scoped tasks and native atomics supported bounded workers, exact-once independent jobs, and overlapping useful-compute intervals. These are not yet a transaction scheduler or a speedup measurement.
- **Embedding:** small callable native libraries worked with pointer/scalar interfaces. Scalar payloads did not require the Vx runtime; tested allocation/task payloads did.
- **Compiler issues:** local fixes for i128 overflow guards and struct-pointer lowering passed regression probes. They are not assumed to be merged upstream. See [Vx #2278](https://github.com/vx-lang/Vx/issues/2278) and [Vx #2154](https://github.com/vx-lang/Vx/issues/2154).

The source investigation is pinned to [Vx dc6a460](https://github.com/vx-lang/Vx/commit/dc6a460873e191e394f5afaa827c1997e43633ac). Fixed-compiler results and benchmarks used that source **plus local fixes**, not an arbitrary Vx installation.

## What the timings do—and do not—say

Matched native toy kernels found switch dispatch close to C, indirect calls more expensive for tiny handlers, and adjacent-pair fusion helpful only on favorable streams. Arithmetic results varied with code shape and optimization. Shared-host contention limits close rankings.

These measurements do **not** establish Ethereum compatibility, performance against a production EVM, or a 20× gain. They also do not settle interpreter versus JIT. See the [performance report](docs/research/performance.md) before drawing architecture conclusions.

## Next milestones

1. Pin the target Ethereum fork, host/state interface, benchmark baseline, corpus, and core budget.
2. Implement the smallest sequential correctness milestone and differential-test complete observable results, including gas and state.
3. Measure representative bytecode before selecting dispatch, predecode, or JIT strategies.
4. Add conflict validation, deterministic ordered publication, bounded retries, and serial fallback; test against the sequential core.
5. Publish a portable experiment harness and the prerequisites needed to reproduce the research.

For review, start with the [spec's open decisions](docs/SPEC.md#12-open-decisions-before-implementation-acceptance). For compiler work, start with the [documented defects and regression evidence](docs/research/feasibility.md#8-compiler-defects-fixes-and-upstream-issues).
