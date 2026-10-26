# EVM-vx

A research-stage project to execute Ethereum transactions in parallel in [Vx](https://github.com/vx-lang/Vx), while preserving the results of sequential execution.

**Current status: specification and feasibility research only.** This repository does not yet contain an EVM implementation, a runnable node, or a production library. The goal of at least 20× improvement is an ambition, not a measured result.

## Start here

| Document | Read it for |
|---|---|
| [Draft specification](docs/SPEC.md) | Execution semantics, host boundaries, candidate parallel state machine, safety invariants, and acceptance criteria. |
| [Vx feasibility findings](docs/research/feasibility.md) | Cited compiler/runtime research, five native experiments, arithmetic, workers, ABI, memory, and two local compiler fixes. |
| [Matched performance findings](docs/research/performance.md) | Vx/C arithmetic and toy dispatch measurements, correctness checks, methodology, and limitations. |

There is nothing to build or run from this checkout yet. The reports describe experiments executed outside this repository; raw harnesses, logs, and compiler builds remain in the author's local research archive. This initial publication contains the spec and findings, not a self-contained benchmark package.

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
