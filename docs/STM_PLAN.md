# Optimistic STM execution plan

Implementation and measured results: [YOLO ownership contract](YOLO.md).
The sections below retain the design requirements, not a claim that the wider
CPU roadmap or 20x target is complete.

Base: `74ba103`. This worktree isolates the STM change from the arithmetic
experiments in the original checkout. Engine and scheduler changes stay in Vx.

## User direction

Use the supplied YOLO-STM structure: optimistic parallel execution, an ordered
conflict scan, and targeted dependency-ordered repair. Remove avoidable shared
mutation instead of substituting a cheaper lock around the same work.

First prove the CALL workload's suspected false conflicts: record transaction,
account/storage location, expected and observed versions, and why the observed
version was installed. Measure acquisition counts, wait and hold time separately
from release performance. Preserve EIP-161 touch/cleanup/rollback semantics.

## Required invariants

- Final state, outcomes, gas/refunds, created addresses, output, logs, and receipts
  equal sequential execution in input order under the same fork.
- Every attempt has one owner. Its read/write logs and allocation arenas are
  private until publication. Readers never observe partially written records.
- Published attempt data remains alive until all readers have joined. Replaced
  speculative versions are retired, not freed under active readers.
- Touch/lifecycle effects remain journaled even when no account-value change
  needs a speculative version. Creation, deletion, and storage generations must
  not be elided as ordinary unchanged account data.
- No partial block reaches StateDb after failure, cancellation, arena/queue
  exhaustion, or incomplete repair. Join workers before destroying shared data.
- Fee deltas avoid a shared fee-recipient write. An actual fee-recipient read
  must observe the correct ordered prefix, including prior final gas charges.

## EVM adaptation

The supplied Rust implementation assumes SVM's complete declared access sets and
uses DashMap shard locks. EVM access lists are not complete read/write manifests.
Keep dynamic logs worker-owned. A repair can discover new keys or omit old
writes. The implementation uses ordered final-prefix repair, so each rerun is
already final and its new writes affect subsequent validation. A future parallel
mini-graph would need a complete closure or a correct serial fallback when that
graph is insufficient.

Phase separation should allow immutable shared snapshots and published records,
with native Vx atomics only at publication and scheduling boundaries. Do not
replace the existing store with another globally locked container. Avoid an
all-pairs hot-key graph; preserve transaction-order dependencies with bounded
storage and an explicit overflow path.

## Whole-engine ownership

YOLO means **You Only Lock Once**. It applies to the engine's ownership model,
not an agent workflow. The scope includes scheduling, state access, version
storage, allocation, publication, failure, and destruction.

Workers execute against an immutable base and own their attempts and logs.
After the workers join, one owner validates and repairs affected transactions
in input order against the finalized prefix, then publishes the block. This
order is already topological and remains correct when EVM execution discovers
new keys. The initial implementation deliberately keeps repair serial, as the
supplied Rust implementation's topo traversal is serial too. Parallel repair
requires a complete dependency closure and is not implied by removing locks.

Only final prefix writes enter the owner-confined store. Each location needs
its latest value, not a sorted array of speculative incarnations. Unchanged
account values do not create dependencies, while lifecycle effects and storage
writes still do. The host must retain the StateDb, environments, and block until
all workers join; publication and destruction cannot race worker execution.

## Acceptance

- Reproduce the same 2,000-transaction CALL workload and retain before/after
  retry causes, lock statistics, release timings, CPU use, and latency samples.
- Include independent transfers, reverts, shared-call targets, and hot storage.
- Verify directed lifecycle and changing-footprint cases separately from speed.
- Match sequential Vx and pinned revm state roots, receipt roots, gas, and logs.
- Enumerate fixture exclusions and fallbacks; empty test runs are failures.
- Sanitizer coverage requires a detected positive race control on the actual
  Vx/LLVM path. That control and an instrumented engine run now pass as described
  in the results document. Instrumented timings are not release performance.
- No 20x claim without matching, repeated measurements on a declared scope.
