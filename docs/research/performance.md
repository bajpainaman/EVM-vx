# EVM-vx matched native probes: combined findings

Date: 2026-10-26. Research artifacts, not a production EVM or 20× claim.

**Publication note:** this document summarizes executed local experiments. Raw sources, samples, logs, compiler patches and binaries remain in the author's research archive and are not included in this documentation-only repository. Public source and compiler issue citations are in the [feasibility report](feasibility.md). This checkout alone is not a reproducible benchmark package.

## Decision summary

Native Vx can execute the tested full-width limb algorithms and bounded toy dispatch correctly. The switch-loop implementation is a reasonable baseline for a first EVM correctness experiment, not a final execution-strategy selection. These probes do not compare interpreters against JITs, execute actual EVM contracts, or measure a parallel transaction engine.

- **Switch dispatch:** Vx matched C closely for these tiny-handler streams, with C/Vx median-time ratios about 0.976–1.022.
- **Typed-local indirect calls:** approximately 2.8–3.4× switch costs in these streams, in both languages. Do not select function-pointer threading merely because it compiles.
- **Adjacent-pair fusion:** about 2.8× improvement over switch on pair-friendly streams, but slower than switch on branch-heavy streams. This is a shared algorithmic opportunity, not a Vx language speedup. It recognizes pairs at runtime; it is not predecoding or JIT compilation.
- **Arithmetic:** no uniform advantage. O3 low-256 multiplication favored this Vx code shape, largely because Vx removed unused high-product work while C retained a full-product helper. This is not a comparison against best-available bigint kernels.

Keep interpreter/JIT, scheduler, and production fusion decisions open until gas-correct representative EVM workloads and end-to-end costs exist. Proposed contracts and open decisions are in the [draft specification](../SPEC.md).

## Provenance and matched work

- Hardware: AMD Ryzen 9 5900X, 24 usable logical CPUs, Linux. Shared busy host; no affinity, frequency, or priority changes.
- Upstream Vx: [dc6a460](https://github.com/vx-lang/Vx/commit/dc6a460873e191e394f5afaa827c1997e43633ac) **plus local uncommitted compiler fixes** described in the [feasibility report](feasibility.md#8-compiler-defects-fixes-and-upstream-issues).
- Compiler SHA256: `bcdd182c2547da191e53c9df94eacc655988338c59b54ab674d05888ee4a281b`; wrapper reports vxc 0.0.3. Hash independently rechecked by coordinator.
- LLVM/Clang 22.1.8, O2/O3, native CPU targeting (znver3), no LTO or C restrict advantage.
- **Build-path qualification:** Vx flat `--emit-llvm` output was translated, optimized, and lowered with explicit LLVM tools. These are real executed native objects, but not the compiler's internal `-c` optimization pipeline. `VX_NO_AST_FALLBACK=1` was required; accepted builds recorded flat diagnostics.
- Independent Python bigint/interpreter expectations checked both C and Vx, complete outputs rather than checksum-only validation. Native clocks exclude Python/FFI transitions, process startup, compilation, allocation, printing, and oracle I/O. Reset/call/output-store overhead is included where reported.
- Three warmups and 15 randomized paired measurements per comparison; timing/calibration/warmup windows held the shared flock. Raw samples, commands, full outputs, IR, executed-object disassembly, and initial runs are retained in probe directories.

## Correctness and coordinator verification

| Evidence | Result |
|---|---|
| Arithmetic full | ADD, low-256 MUL, UDIV, full-product MULMOD; 989 cases per operation × C/Vx × O2/O3; all passed. Zero divisor/modulus and full-width boundary/random inputs included. |
| Arithmetic negative controls | All 16 high-limb output mutations detected. |
| Arithmetic timed output validation | All 330 complete output blocks matched the independent oracle. |
| Dispatch full | 120 complete state/status/counter comparisons passed; deliberately wrong native output rejected. |
| Dispatch timed output validation | 864,420 stored state/status results checked; exact logical instruction denominators retained. |
| Coordinator arithmetic smoke | Exit 0; 285 cases per operation × C/Vx × O2/O3 = 4,560 outputs; all 16 negative controls detected. Reused original built-library digests, no timing. |
| Coordinator dispatch smoke | Exit 0; freshly compiled small kernels; 120 comparisons and mutation control passed, no timing. |

The local archive retains separate arithmetic/dispatch smoke logs, exit statuses and machine-readable checks. Full timed results remain separate and were not overwritten by smoke.

The archived combined smoke script was then executed successfully, exit 0, including its compiler hash check. This repeated correctness only; no timed run was repeated by the coordinator.

## Arithmetic measurements

Ratio = C median time / Vx median time; above one favors Vx. These are source-algorithm comparisons, not equal machine-instruction-count comparisons.

| Operation | O2 C/Vx | O3 C/Vx |
|---|---:|---:|
| ADD | 0.809 | 0.848 |
| low-256 MUL | 1.252 | 2.510 |
| UDIV | 0.975 | 0.971 |
| full-product MULMOD | 1.135 | 0.987 |

At O3, C retained a full-product helper with 16 multiply instructions; Vx inlined and pruned to 10. The 2.510 ratio is therefore evidence of a code-shape/optimization difference worth addressing in a future matched low-product baseline, not proof of inherently faster multiplication.

One-minute load rose approximately 29→35 during the final arithmetic run on 24 logical CPUs. Several timing IQRs were 20–55% of the median; most paired ranges crossed one. Batch duration ranged 13.906–98.706ms, so the calibration goal of ≥20ms was not maintained for all samples. Two full runs used 17.095s native work including warmups/calibration; both are preserved. No strong general language ranking follows.

Exact ns/op, per-implementation IQR, paired-ratio spread, algorithms, commands, JSON and execution history are retained in the local arithmetic archive. IQR means interquartile range, a measure of sample spread, not a confidence interval.

## Dispatch measurements

540 final measured samples, minimum sample 49.382ms, 33.140s measured native work (42.556s including warmups/calibration). Final inputs execute 16,384 logical toy instructions per repetition with full output stores. The earlier shorter run remains in `dispatch/initial-run/`.

| Stream | O2 switch C/Vx | O3 switch C/Vx | Vx switch/fused O2 | Vx switch/fused O3 |
|---|---:|---:|---:|---:|
| Mixed | 0.998 | 0.997 | 0.976 | 0.952 |
| Pair-friendly | 0.999 | 0.996 | 2.824 | 2.848 |
| Branch-heavy | 1.022 | 0.976 | 0.634 | 0.676 |

Switch/fused above one favors fusion. Indirect calls remained actual indirect calls; their table used caller-supplied native handler addresses, not native Vx function-array literal support. Valid fixture-domain arithmetic is much narrower than full EVM words. No gas, EVM memory expansion, state I/O, or real contract behavior is represented.

Host load approximately 15.6–20.9. Mixed fused O2 C had 35.8% IQR/median, making its large isolated C/Vx ratio unreliable for ranking languages. Cross-strategy ratios come from separate randomized blocks and can reflect host drift. Cache-hot repeated streams also favor prediction unlike a diverse contract corpus.

All 18 cells, paired spread, native code sizes and raw samples are retained in the local dispatch archive.

## Reproduction limits

The preserved local smoke checks the compiler hash, runs arithmetic correctness using retained libraries, rebuilds small dispatch kernels, and checks negative controls. Separate full commands regenerate measurements under a shared timing lock. The required compiler build, patches, harnesses and inputs are not bundled here, so there is no fresh-machine reproduction command in this snapshot. Do not substitute an arbitrary installed Vx version and treat it as the measured toolchain.

## Next evidence needed, in priority order

1. Select fork and baseline/corpus/cost boundary. A 20× target without these is not testable.
2. Implement the smallest gas-correct sequential EVM slice; compare complete observations against the pinned reference before timing.
3. Measure switch baseline on representative bytecode, then compare fusion/predecode and any JIT candidate including preparation costs, memory, and cold/hot execution.
4. Compare equivalent optimized low-product algorithms and realistic arithmetic mixes under quieter host conditions; retain raw spreads rather than cherry-picking the best ratio.
5. Measure ordered transaction parallelism with independent and conflicting state access, counting validation, retries, serial fallback, state I/O and commitment costs. Existing native worker overlap is feasibility evidence, not scheduler throughput.

The measurement pass produced research artifacts only, not a product EVM. This repository publishes the documentation, not the compiler build or experiment binaries.

Next: review the [draft specification's open decisions](../SPEC.md#12-open-decisions-before-implementation-acceptance) before selecting the implementation strategy.
