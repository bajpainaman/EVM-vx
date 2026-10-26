# EVM-vx: execution and parallelism contract

Status: **draft 0.1, for review; not an implemented or mechanically verified specification**.
Date: 2026-10-26. Project: [EVM-vx](https://github.com/bajpainaman/EVM-vx).

## 1. Direction

Build a small native CPU EVM in Vx whose observable execution matches a pinned Ethereum fork. Parallelize independent transaction work without changing the input transaction order or Ethereum semantics. Treat performance as a measured property, not a semantic shortcut.

Use **Markdown with explicit state, transitions, invariants, and refinement obligations**, borrowing the useful discipline from MIT's Spec handout. Reuse a fork-pinned executable Ethereum reference rather than inventing another opcode specification. Do not require contributors to learn an additional DSL before implementing the first correct engine.

The proposed parallel design below is an ordered speculative executor. It is a reviewable candidate, not a commitment to MVCC, a particular storage engine, interpreter dispatch, or JIT. The sequential contract applies to any implementation.

### What this draft does not claim

- No complete EVM, consensus client, scheduler model, or formal proof exists in these artifacts.
- No 20× EVM improvement has been demonstrated. Kernel ratios are not end-to-end speedups.
- Passing Vx feasibility probes is not Ethereum conformance.
- This draft is not fork-complete until the semantic profile and conformance matrix are filled in.

## 2. Specification sources and precedence

1. An explicitly selected Ethereum fork and chain configuration define the intended behavior.
2. A pinned executable reference and test corpus operationalize that behavior. Proposed references are EELS and Ethereum execution-spec-tests.
3. This document specifies the engine boundary and parallel refinement obligations. It must not override the chosen fork. Disagreements block release pending resolution, rather than silently changing gas or execution rules.
4. Vx implementation choices, compiler behavior, and benchmarks are subordinate evidence, not semantic authorities.

Candidate snapshots, resolved during this research, **not yet an approved fork profile**:

- [EELS dc1bb6b](https://github.com/ethereum/execution-specs/tree/dc1bb6bd9a38d91a22b73f045e8feee62d166ada).
- [execution-spec-tests 10eaa63](https://github.com/ethereum/execution-spec-tests/tree/10eaa63d5da2f50b63d4359968f36542212f9f50).

The profile must record fork name/activation rules, chain ID/configuration, enabled transaction types and EIPs, opcode table, gas schedule/refund rules, precompile set and versions, code-format rules, and block/system operations. A repository revision alone is not a fork selection. Never combine rules from different forks implicitly.

## 3. Scope and external boundary

The initial target is an embeddable CPU execution library, not networking, consensus, a mempool, a database server, or a GPU EVM. A host supplies immutable execution context and state access; the engine returns an execution result and a state delta. Exact C ABI and allocation ownership remain open.

Define two layers so a message interpreter is not mistaken for transaction or block execution:

- **Message execution:** code, call/create context, gas, stack/memory, nested frames, return/revert/exception behavior.
- **Ordered transaction execution:** envelope validity, nonce/funding rules, intrinsic gas, message execution, transaction settlement, and receipts according to the selected profile.

Block pre/post operations, withdrawals, system calls, commitments, and other fork-specific transitions must have an explicit owner and position in the reference order. Until those are implemented or faithfully supplied by an adapter, the library must not claim full block equivalence.

Inputs are the fork profile F, initial world state S₀, immutable environment E, and ordered transactions T[0..n). State access failures and missing witness data are host failures, not absent accounts or zero storage values.

Environment includes every external fact execution can observe: block fields, historical block hashes, chain configuration, transaction context, and precompile inputs. Workers must not obtain semantic inputs from wall clocks, scheduling, random seeds, or mutable unversioned host callbacks.

## 4. Abstract state and observations

### Persistent reference state

World state maps addresses to account existence, nonce, balance, code identity/content, and storage. Missing, empty, and deleted accounts remain distinct whenever the fork distinguishes them. Representation, hash maps, tries, and database versions are implementation details.

### Transaction-local state

Each transaction owns its call-frame stack, operand stacks, memory, return data, journal, logs, refund accounting, warm-access sets, original storage values, transient storage, and creation/deletion bookkeeping as required by F. Reset and rollback boundaries follow F; these fields must not leak between transactions or retries.

Each frame carries its code identity, program counter, stack height, remaining gas, execution/storage address, caller/value/calldata, static-call flag, and return continuation. Delegated execution must not conflate code address with storage address.

### Observable result

The comparison boundary includes final persistent state, ordered transaction results, ordered logs, return/revert bytes where exposed, gas use, refunds/settlement, receipt fields and cumulative accounting, and fork-required outputs. Compare canonical state contents or commitments, not internal map order or pointer identity.

Diagnostics, worker assignment, retry counts, allocation addresses, and speculative traces are not consensus outputs. If tracing is exposed, committed traces must be identified separately from discarded attempt traces.

### Result categories

| Category | Meaning and publication rule |
|---|---|
| Success | Reference-defined completion and effects. |
| EVM Revert | Valid execution outcome; reference rollback, gas, and return-data rules apply. Not equivalent to discarding all transaction effects. |
| EVM Exception | Reference-defined exceptional halt, such as out-of-gas or invalid execution. Not a host crash. |
| Invalid Transaction | Envelope/state-dependent validity failure. In block-validation mode reject as the reference does; do not skip it or turn it into a failed receipt. |
| Host Failure | Allocation failure, unavailable state, worker failure, timeout, or compiler/runtime fault. Never reinterpret as REVERT, out-of-gas, or a zero read. |

A speculative invalidity/exception is only an attempt result until its dependencies validate against the correct ordered prefix. An earlier transaction may change the condition that caused it.

## 5. Sequential reference contract

Let RefTx(F, E, Sᵢ, Tᵢ) produce the reference next state Sᵢ₊₁ and result Rᵢ, or the profile-defined transaction validity error. Ordered execution applies RefTx in input order, with fork-defined block transitions at their required positions.

For an accepted batch, EVM-vx must produce the same canonical observations as that sequential execution. One-worker and many-worker modes have the same contract. A request may return Host Failure instead of completion under explicitly documented host limits, but it must not publish a successful result with different semantics.

### Integer contract

- Word values are unsigned 256-bit bitvectors; signed opcodes interpret the same bits as two's-complement values.
- ADD/SUB/MUL return the low 256 bits. ADDMOD and MULMOD reduce the full mathematical intermediate, not its wrapped low word.
- DIV/MOD/SDIV/SMOD return zero for zero divisor. SDIV truncates toward zero; signed minimum divided by −1 wraps to signed minimum. SMOD follows the dividend's sign.
- Shift operands are full words. SHL/SHR counts ≥256 produce zero; SAR produces sign fill. No host invalid shift or signed-overflow operation may be reachable.
- Byte order, memory encoding, sign extension, exponentiation, comparisons, and remaining opcodes must match the pinned reference. Host limb order is not the EVM byte order.

These are acceptance constraints, not evidence that all opcodes are implemented. Existing limb experiments cover only their enumerated operations.

### Execution checks

Stack under/overflow, gas charges, dynamic gas, memory expansion and zero initialization, jump destinations, truncated PUSH behavior, call-depth rules, return-data bounds, static restrictions, CREATE/CREATE2, precompiles, and rollback must follow F, including failure ordering. Optimized handlers or fused instructions must preserve intermediate checks and failure points, not just successful final values.

Host pointer/length calculations must be checked before truncating U256 offsets into machine indices. A huge EVM offset is not permission for an overflowing allocation or an out-of-bounds native access. Memory/gas arithmetic must not wrap accidentally. Host memory limits are distinct from EVM out-of-gas.

## 6. Candidate parallel machine

Abstract scheduler state consists of:

- C: immutable, published world-state snapshot for the committed transaction prefix.
- k: index of the next transaction permitted to commit; initially zero.
- A: bounded set of attempt records, each tagged with transaction index, generation, and base snapshot.
- R: committed ordered results; initially empty.

An attempt owns a private write overlay, result/journal data, and a complete dependency record. Other workers cannot read its uncommitted writes. The first candidate uses committed snapshots only; reading speculative predecessor versions would require a different dependency/invalidation design.

The following are abstract actions, not a requirement to implement each as one hardware instruction:

| Action | Preconditions | State change |
|---|---|---|
| Claim(i) | i ≥ k; no competing live generation; capacity available | Create an attempt against a retained consistent snapshot of C. |
| Execute(i,g) | Generation g owns its buffers | Evaluate reference-equivalent transaction behavior; record dependencies and private effects. No shared semantic writes. |
| Complete(i,g) | Attempt finished | Publish a fully initialized immutable attempt record with synchronization. |
| Validate(k,g) | Completed current generation for the next index | Check all dependencies against current C. Validation and publication must not race another semantic writer. |
| Commit(k,g) | Validation succeeds | Atomically publish reference-equivalent next state and result, then advance k. |
| Retry(k,g) | Validation fails | Discard attempt-local semantic effects; execute a fresh generation against a current snapshot. |
| Serial fallback(k) | Retry/resource policy requests it | Execute next transaction against C without stale speculation; publish only a complete reference-equivalent result. |
| Fail host | Unrecoverable host error | Stop successful publication, join/cancel safely, and return a distinct host error. |

Atomic publication means readers never observe half an account update or a state/result prefix mismatch. It may use a coordinator and immutable root, a lock, or an equivalent mechanism. Persistence and crash recovery are separate from in-process publication.

### Dependency completeness

Track every state observation that could change execution, validity, gas, or settlement: account existence and fields, code identity, storage, original storage values, and implicit reads such as transfers, fees, nonce checks, creation collisions, and deletion rules. Include negative reads and host-derived state results. A storage-slot version alone is insufficient if account lifecycle can invalidate it.

Reads of the attempt's own writes use its overlay. External reads use one consistent base snapshot, not a mixture of separately sampled mutable versions. Version tokens must change on every relevant update, avoid ABA/reuse while retained, and survive snapshot retention correctly. Whole-account or global conservative validation is permitted as a correctness-first implementation, though it may serialize execution.

Do not publish worker-computed cumulative receipts or other prefix aggregates without validating/recomputing them from committed prefix data. Block gas/validity checks are reference operations, not scheduling heuristics.

## 7. Safety invariants and refinement obligations

**S1 — Prefix equivalence.** C and R equal the sequential reference after exactly k transactions and the corresponding boundary transitions.

**S2 — Isolation.** An attempt cannot mutate C or another attempt's semantic state before Commit. Failed attempts do not leak logs, gas/refund counters, transient state, or output buffers.

**S3 — Complete validation.** A successful validation establishes that the attempt's observations and state-dependent validity match execution against the current prefix. Conservative rejection is safe; missed conflicts are not.

**S4 — Ordered, exact-once publication.** Only index k can commit, and at most one generation publishes for that index. Attempts may execute more than once; transactions commit once.

**S5 — Deterministic observations.** Changing worker count, claim order, delays, or retries cannot change accepted state/results. Internal scheduling may remain nondeterministic.

**S6 — Memory and ownership safety.** Snapshot/overlay/output lifetimes outlive all their readers. Worker completion uses a defined synchronization edge; job-claim atomics alone do not publish results. No data race, use-after-free, or partial result is allowed.

**S7 — Failure separation.** Host failures never fabricate a reference result. Candidate batch API: all-or-error publication to the caller, with private intermediate prefixes. A streaming/prefix API requires an explicit different contract.

Refinement argument to discharge: Claim/Execute/Complete/Retry preserve the abstract committed state (stuttering steps); a valid Commit corresponds to exactly one RefTx step. Initialization and block-boundary actions correspond to their reference transitions. Induction over k then establishes batch equivalence. This is an obligation, **not a completed proof**.

## 8. Progress and resource bounds

Safety does not guarantee speed or completion. Under finite valid input, available host resources, terminating host services, fair worker/coordinator scheduling, and finite gas, the design should make progress.

Use bounded workers, bounded in-flight attempts, bounded retained snapshots, and a configured retry limit followed by serial fallback. Exact numeric limits remain to be measured. Admit work only when buffers/snapshot budget allow; prioritize the next-to-commit transaction so later work cannot exhaust resources and starve it.

No barrier may require every requested worker to start successfully. Thread-creation failure must yield safe reduced concurrency or Host Failure. Cancellation must join workers before freeing shared memory. Long native kernels and host callbacks need explicit cancellation/timeout behavior; wall-clock deadlines are host policy, not consensus gas.

Do not infer resident memory from requested thread-stack reservations. Report peak RSS, virtual reservations, per-worker buffers, retained state, and cache costs separately.

## 9. Acceptance and counterexamples

| Layer | Required evidence before claiming support |
|---|---|
| Compiler substrate | Pin plus patch digest; native no-AST-fallback regression runs at O0/O2/O3; guard/struct-pointer controls retained. |
| Word arithmetic | Independent bigint expectations; full-width inputs and outputs; carry chains, zero division, signed edges, high shift counts, full modular intermediates; mutation detection. |
| Sequential EVM | Fork-selected state/VM fixtures plus differential results against pinned reference; gas, state, logs, failure bytes/status checked, not only return value. Enumerate exclusions. |
| Parallel refinement | Same fixtures at 1/2/4 workers, deterministic perturbed schedules, directed conflict histories, retries and serial fallback; complete output/state equality. |
| Memory/ABI | Real caller-owned-buffer tests, lengths/ownership/layout/alignment, lifecycle/failure checks, runtime dependency inventory. |
| Performance | Equivalent correct work, pinned baseline, declared machine/core budget, costs and raw samples; no inference from codegen alone. |

Directed scheduler histories must include read-after-write, write-after-read, blind write/write ordering, disjoint storage, missing-account creation, balance/nonce conflicts, code replacement/lifecycle, reverted nested writes, warm/cold gas dependencies, original-storage/refund effects, transaction-local transient isolation, stale invalid transactions, and stale exceptional outcomes. Force a slow predecessor and completed later attempts. Inject missing dependency/version checks and duplicate commits to show validation catches plausible bugs.

A small bounded scheduler model in TLA+/Quint or equivalent is a proposed follow-up for S1–S5, with a finite key/value domain, conflict cycles, retries, generation replacement and failed workers. It will not prove native memory safety or all EVM semantics. Select the tool only after the state machine is agreed.

## 10. Performance decision contract

The goal ≥20× needs a named baseline/version, corpus, execution scope, hardware/core count, cache/state conditions, and aggregation rule before it is testable. Report single-thread engine efficiency separately from multicore throughput; using more cores is not a language speedup.

Required future workloads include real contract computation, storage-heavy execution, independent transactions, highly contended transactions, calls/creation/precompiles, and adversarial conflict chains. Report serial fraction, retry/validation costs, latency distribution, throughput, memory, preprocessing/JIT compilation, state I/O, and commit/commitment costs. Declare exclusions rather than hiding them outside a timer.

Current matched kernels suggest a switch loop is a sensible **baseline experiment**: Vx and C switch times are close, tiny-handler indirect dispatch costs more, and adjacent-pair fusion helps only favorable streams. Arithmetic results are workload/code-shape dependent and noisy. These results do not decide interpreter versus JIT, production fusion policy, or the scheduler design. See [combined performance findings](research/performance.md).

## 11. Native packaging and implementation constraints

Target a pointer/scalar library ABI first; do not expose compiler-private aggregate-return layouts. Separate code generation/compiler dependencies from runtime payload dependencies. Record actual linked symbols and dynamic libraries for the chosen build path.

A U256 may be four u64 limbs; native u256, automatic 32-byte alignment, an existing U256-keyed map, or compiler-enforced tail calls must not be assumed. Public EVM encoding is fixed independently of limb representation. Alignment, allocation growth/zeroing, OOM behavior, and host ownership require tested contracts.

The research compiler is upstream dc6a460 plus local uncommitted fixes, not an upstream release with those fixes. Benchmarks use flat LLVM emission and explicit LLVM translation/optimization/object generation, not the compiler's internal object pipeline. Preserve this distinction in build provenance. See [consolidated source and compiler findings](research/feasibility.md).

## 12. Open decisions before implementation acceptance

1. Select the Ethereum fork/chain profile and initial conformance scope. Recommendation: one explicit fork, not every historical fork at once.
2. Define the host boundary: message execution only, ordered transactions, or full block integration; state adapter, publication/failure ownership, precompile provider, and ABI.
3. Pin the end-to-end performance baseline/corpus and core budget. Keep 20× as a target until that contract exists.
4. Select the first correctness implementation and scheduler experiment after representative workloads; switch-loop baseline is a recommendation, not a measured universal winner.
5. Agree numeric resource budgets, retry policy, and how compiler fixes are maintained/upstreamed.

The draft can be refined while these remain open; no fork-specific implementation should claim acceptance with them unresolved.

## Appendix A. Why not adopt the entire Spec/ISA framework now?

[MIT 6.826, Handout 3: Introduction to Spec](https://web.mit.edu/6.826/archive/S06-www/notes/HO3.pdf) supplies useful ways to describe state changes, abstraction and concurrency. Relevant printed handout pages: introductory execution/state material pp. 2–7, atomicity/concurrency p. 31, refinement examples pp. 33–38, design summary p. 40. The PDF uses two-page spreads; these are printed page labels, not PDF page indices. The original handout is linked rather than redistributed here.

Borrow that discipline, not an obligation to build a Spec toolchain. Markdown is the review surface; executable differential tests are the first operational oracle. The key distinction is deterministic external observations versus permitted nondeterministic internal scheduling.

Framework families considered during research include ISA-oriented languages (Sail/ASL), executable semantics (K/KEVM, SpecTec), proof assistants (Lean/Rocq), readable references (EELS), and concurrency models (TLA+/Quint). Third-party coverage percentages and current framework-status claims were not independently validated here and are not used as evidence.

For this project, reuse Ethereum semantics rather than specify a new ISA. A scheduler model answers a different question than an opcode model. K/KEVM or Lean may later support targeted proofs if the maintenance cost is justified; no framework adoption or proof claim is made in this draft.

Next: review the [open decisions](#12-open-decisions-before-implementation-acceptance) and the [research evidence](research/feasibility.md) before choosing an implementation milestone.
