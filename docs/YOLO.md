# YOLO ownership contract

YOLO means **You Only Lock Once**. This implementation uses ownership and phase
separation to acquire no engine mutex or spinlock. It is an optimistic CPU
executor, with parallel speculation and serial ordered repair/publication.
It does not claim parallel repair or a globally lock-free process: thread joins,
libc allocation, the kernel, and external cryptographic providers have their own
implementation details.

## Protected invariants and their owners

| Data | Owner and invariant | Synchronization |
|---|---|---|
| StateDb, environments, opcode tables | Host retains them unchanged while workers run. No other block, transaction, mutation, or destructor may use the same StateDb concurrently. | Host ownership and explicit worker joins |
| Ctx, journal, frames, read log | One worker owns all mutable execution state. | None |
| Attempt/result slot | One atomic claim assigns its writer. Finalizer reads only after join and completion publication. | Relaxed fetch-add claim; release/acquire completion |
| Prefix maps and values | Only finalizer accesses these mutable maps. Each installed writer is final and earlier than the current transaction. | None |
| Fee accumulator | Finalizer maintains exact prior fees, resetting the pending sum when an explicit beneficiary write folds them in. | None |
| Allocator heap | One live thread owns the local lists. Registry links never change after publication and nodes are never reclaimed. | Atomic ownership claim/release; remote-free publication |

There are no per-read locks, location locks, shard locks, write-log locks,
commit locks, retry locks, allocator mutexes, or lazy-initialization spinlocks.
The C atomic shim is removed. `atomic.vx` emits LLVM atomic operations directly.

## Execution and correctness

1. Every transaction executes against the same immutable pre-block StateDb.
   Reads, results, and writes remain private; no speculative write is installed
   in a shared map. Workers exit when no work remains.
2. After joining all workers, the finalizer walks transaction indices. An
   attempt is stale when an earlier final semantic write intersects its dynamic
   read log, or when its beneficiary read needs deferred fees. It reruns that
   transaction once against the exact final prefix.
3. Only final writes enter the prefix store. One value per location suffices;
   overwrite is constant expected-time map lookup, with amortized map growth.
   There are no sorted version arrays, insertion shifts, incarnation scans,
   speculative-write cleanup, or quadratic hot-key dependency graphs.
4. After every final result is valid, results are applied to StateDb in order
   and the remaining beneficiary fees are paid once. `finish` is idempotent.

Induction over the finalizer's index gives the ordering argument: if the prefix
is final, an unchanged read set has the same inputs as sequential execution;
otherwise a rerun observes that exact prefix. New keys or omitted writes in a
rerun therefore require neither another speculative iteration nor an assumed
complete EVM access list. This is an implementation argument, not a formal proof.

Account-value elision happens **after validation**, by comparison with the final
prefix. It must never be moved to speculative installation without recording
the dependency. Touch remains in the journal/result. Deletion and creation
publish lifecycle writes and a storage reset generation. Reads of slots older
than the last reset return zero. Unchanged nonempty callees produce no account
write solely from a zero-value CALL.

Retry diagnostics expose address, storage key, expected/observed writer, and
reason through `evmvx_block_conflict`; expected `u64::MAX` means base state.
Kinds are account=1, storage=2, beneficiary=3. Reasons are account value=1,
storage value=2, lifecycle=4, deferred fees=8. Beneficiary diagnostics describe a
prefix requirement, not a single conflicting writer. `pexec --json` retains
every retry record outside the execution timer.

## Completion, cancellation, and destruction

The host must explicitly join every started worker, including idle workers.
It may start fewer workers than requested; no worker-count barrier exists.
No started workers means a nonempty batch is incomplete and publishes nothing.
The host calls cancellation before finalization, then joins. Cancellation is
observed between transactions; it does not interrupt an active EVM frame.
Finalization, result inspection, and destruction are host-owned operations and
must not race each other or workers.

Cancelled, incomplete, host-failed, and finally invalid batches leave StateDb
unchanged. Reverts and EVM halts remain valid transaction outcomes. Private logs
grow rather than exhausting a shared fixed arena. There is no task queue that
can silently drop work. Allocation exhaustion retains the engine's existing
process-failure behavior; recoverable allocation failure is not implemented.

All attempt results remain alive through finalization; replaced speculative
results can be freed because no other attempt ever read them. Block destruction
releases logs/results/maps after join. Allocator heaps remain in a permanent
registry and are reused across thread lifetimes; this preserves the existing
process-lifetime heap policy while removing the orphan mutex.

## Validation

Built with Vx source `dc6a460` plus the existing local compiler fixes, flat LLVM
emission, LLVM 22, `-O2`, on a Ryzen 9 5900X Linux host. No arithmetic experiments
from the separate CPU worktree are included.

- 13 directed tests cover empty/incomplete/cancelled batches, publication and
  repeated finish, invalid final transactions, private log growth, unchanged
  callees, hot storage, changing write sets, touch rollback, CREATE output,
  refunds, and newly discovered beneficiary reads. They compare against revm
  and sequential Vx; directed block checks include state and receipt roots.
- 1,000 generated blocks / 32,909 transactions, seed 12345, 12 workers: outcomes,
  gas/refunds, output, created addresses, logs and final state match both
  sequential engines. 30,016 total repairs include 4,327 beneficiary repairs.
- Selected stable blockchain fixtures: Frontier touch, Constantinople CREATE2,
  Cancun SELFDESTRUCT. 18 files, 592 cases passed; 610 blocks ran in parallel,
  zero fallback. Existing fixture skip policies remain in force.
- A deliberately racy Vx function was compiled through LLVM's TSan passes;
  TSan detected its concurrent writes. With all Vx functions and the C allocator
  instrumented, 200 generated blocks / 6,287 transactions then passed without a
  reported race. Rust and external crypto dependencies were not instrumented.
  An initial report across Rust's uninstrumented scoped-thread completion was
  resolved by explicitly joining every worker; no suppression was used.

Differential checks also exposed two pre-existing result-field mismatches:
successful CREATE omitted deployed output bytes, and transaction results always
reported zero refunds. Both are corrected and covered by directed comparisons.

## Measurements

The baseline CALL diagnostic run at 2,000 transactions / 12 workers recorded
33,992 exclusive fine-grained acquisitions, 504,731,409 ns aggregate wait,
90,792,307 ns aggregate hold, and two commit-gate acquisitions holding for
281,424,451 ns total. Uprobes substantially perturb this run; these are lock
diagnostics, not release execution times. The new engine has no corresponding
lock operations or commit gate. Release timings are measured separately.

The baseline source chain is: zero-value transfer journals a touch; installation
publishes the unchanged common callee; writer-identity validation rejects later
readers. Baseline retry counts and the new no-false-conflict regression support
that diagnosis. The old binary's exact per-conflict keys were not captured;
do not treat source-derived attribution as a historical dynamic key trace.

Serial repair is the remaining scaling limit on hot dependency chains. Removing
locks does not imply a 20x speedup. The broader CPU roadmap and device port are
not complete.

Matched release runs use the saved `74ba103` engine with the same workload
harness, 2,000 transactions, 12 workers pinned to CPUs 0-11, Cancun rules, and
21 repetitions per row. Contracts=2,000 except hot storage=1. Engine execution,
scheduling, finalization, and observation copying are timed; StateDb setup,
root computation, verification, and destruction are reported separately or
excluded. No instrumented binary contributes to these times.

| Workload | Before median ms | After median ms | Before / after p95 ms | Throughput before / after tx/s | After vs revm | Retry count before / after |
|---|---:|---:|---:|---:|---:|---|
| Transfer | 10.318 | 3.091 | 15.955 / 10.964 | 193,836 / 646,991 | 0.641x | 0 / 0 |
| CALL | 31.604 | 4.025 | 37.085 / 12.010 | 63,283 / 496,837 | 3.557x | 1,500-1,999 / 0 |
| Revert | 10.385 | 2.845 | 17.712 / 10.673 | 192,583 / 702,877 | 0.979x | 0 / 0 |
| Hot storage | 35.362 | 15.760 | 47.913 / 18.914 | 56,559 / 126,900 | 1.056x | 1,742-1,999 / 1,999 |

p95 is the nearest-rank sample statistic from 21 runs, not a production latency
estimate. All after samples passed three-way state/receipt-root and observation
comparisons. Hot-storage retry records identify slot 0 of the shared contract,
the prior final writer, expected base state, and storage-change reason 2.

`/usr/bin/time` process CPU seconds (user+system) before/after were transfer
1.93/2.09, CALL 9.23/4.48, revert 2.72/2.93, and hot storage 9.34/3.84. These
include all three engines, setup, roots, and verification; they do **not** isolate
parallel-engine CPU use. The after harness also computes roots/retry diagnostics
outside its execution timer. Do not interpret the transfer/revert process totals
as isolated executor CPU regressions or the reductions as precise engine ratios.

Raw artifacts are retained on the Linux validation host under
`/data/fast/evmvx-stm-build-20261028/`: `before-*.json`, `after-*.json`,
`before-*.cpu`, `after-*.cpu`, `test-final.log`, `pfuzz.log`, `lifecycle.log`,
and `tsan/{probe.log,run-joined.log}`. Baseline lock diagnostics are in
`/data/fast/evmvx-cpu-control-build-20261028/{locks.bt,locks.log}`.

Reproduce a row with the appropriate baseline or candidate binary:

```sh
taskset -c 0-11 build/evmvx-test pexec --txs 2000 --contracts 2000 \
  --threads 12 --reps 21 --workload calls --json
```

The sanitizer route marks generated LLVM function definitions `sanitize_thread`,
runs LLVM 22 `opt -passes='default<O1>,tsan-module,function(tsan)'`, and links
the instrumented Vx object plus `clang-22 -fsanitize=thread` allocator object.
The Rust harness is linked with Clang's TSan runtime via `cargo rustc --bin
evmvx-test -- -C linker=clang-22 -C link-arg=-fsanitize=thread`; dependencies
remain uninstrumented. Explicit joins make the native worker lifetimes visible
to that runtime. The positive Vx race control is essential to this claim.
