# EVM in Vx: consolidated findings

**Date:** 2026-10-26. **Status:** research and feasibility experiments completed, two compiler fixes implemented locally. No product EVM is implemented.

This document consolidates the source investigation, five executed probes, compiler fixes, final coverage update, packaging evidence, and remaining decisions. Earlier reports remain available as historical evidence. Their original failures must not be confused with the locally fixed results below.

**Publication note:** this is the feasibility snapshot from before the matched timing pass. See [performance findings](performance.md) for the subsequent measurements and the [draft spec](../SPEC.md) for the proposed contract. Raw experiment sources, logs, compiler binaries, and patches remain in the author's local research archive; they are not included in this documentation-only repository snapshot. Reported execution results are not independently reproducible from this checkout alone.

## 1. Bottom line

**Vx supplies enough tested native functionality to justify an EVM prototype. It does not yet supply a production EVM or an established performance advantage.**

- Exact U256 arithmetic works as Vx limb algorithms, using u128 intermediates. A primitive u256 type is not required for the tested algorithms.
- Explicitly guarded full-U256 division and shifts match independent Python expectations at O0/O2/O3.
- Switch dispatch, typed-local indirect calls, and a small fused dispatcher execute correctly. No performance ranking is established.
- Scoped CPU tasks and native atomics support bounded workers with exact-once job processing. Supplementary tests establish overlapping useful-compute intervals.
- Callable native libraries work. Scalar code can deploy without the Vx runtime. Tested allocation and task libraries require a separate runtime.
- Two discovered compiler blockers have local fixes and regression evidence. They are not merged upstream.
- No full EVM, transaction executor, MVCC engine, or U256 state map was built.
- **No result establishes 20×, competitiveness against evm2/revm, or any EVM throughput improvement.**

## 2. User goals versus open decisions

The requested direction is an extremely minimal parallel EVM written in Vx, with a target of at least 20× improvement. `alloy-rs/evm2` was named for comparison. An iPhone-sized deployment was an illustration of minimalism, not a hard requirement.

The following decisions remain open:

1. Which Ethereum fork defines correctness?
2. Which benchmark, baseline revision, hardware, and state-access model define 20×?
3. Does the target concern transaction latency, block throughput, or both?
4. Should execution use an interpreter, translated code, or a hybrid?
5. Which implementation layout should the new [EVM-vx repository](https://github.com/bajpainaman/EVM-vx) use?
6. What embedding API, state ownership model, and deployment targets must be supported?

An execution-only, same-hardware comparison against evm2 was proposed, but not frozen as the benchmark contract. An embeddable core with caller-supplied state was also proposed. These are recommendations, not already approved architecture decisions.

The preferred staged direction is a correct sequential execution core before speculative transaction parallelism. This avoids confusing native thread support with a correct parallel EVM.

## 3. Provenance and evidence standard

### Source and toolchains

| Item | Value |
|---|---|
| Upstream source pin | [dc6a460873e191e394f5afaa827c1997e43633ac](https://github.com/vx-lang/Vx/commit/dc6a460873e191e394f5afaa827c1997e43633ac) |
| Original compiler version | `vxc 0.0.3`, built from the pinned source |
| Original compiler SHA256 | `758ffc4a647093e3c25468afbecac05a0ee339daff197155e0aec4778f7c9aa1` |
| Fixed compiler SHA256 | `bcdd182c2547da191e53c9df94eacc655988338c59b54ab674d05888ee4a281b` |
| LLVM / Clang | 22.1.8 |
| Rust / Python | Rust 1.93.0, Python 3.12.3 |
| CPU | AMD Ryzen 9 5900X, 12 cores / 24 logical CPUs |
| Memory snapshot | About 47 GiB physical memory; available memory and load varied |

The compiler is a development build. Generated payload optimization levels were selected independently. Older installed-v0.0.2 smoke data is non-authoritative and is not counted as acceptance.

The shared compiler build used existing prerequisites and the shared Cargo queue, with at most four requested jobs. No system-wide installation occurred. Static MLIR C API archives satisfied the required link contract. Missing `libMLIR-C.so` did not remain a blocker.

### What an executed pass means

- Programs ran as actual Vx-produced native code with `VX_NO_AST_FALLBACK=1`.
- Compiler diagnostics identified the backend. Explicit legacy workarounds were separately labeled in the original payload experiments.
- Python standard-library big integers supplied independent arithmetic expectations.
- Inputs arrived at runtime. Complete outputs were compared, not only checksums or process exit codes.
- Fixed seeds, directed boundaries, commands, compiler hashes, stdout/stderr, and return codes were preserved.
- Intentional wrong expectations or outputs tested each harness's ability to detect mismatches.
- IR and disassembly support code-generation observations. They do not substitute for execution or establish speed.

Shared-host load was substantial. No matched timed baseline was completed during this feasibility phase; the later [performance pass](performance.md) adds matched toy kernels, not a full EVM baseline. Worker counts were capped at four. No CPU affinity or frequency settings were changed.

## 4. Integer semantics and exact words

### Types and literals

The primitive type definitions include i128 and u128, but no primitive i256/u256 in the inspected enum and type parser. Both 128-bit types lower to signless MLIR i128. See [types.rs:628–656](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/syntax/types.rs#L628-L656), [types.rs:1388–1402](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/syntax/types.rs#L1388-L1402), and [mlir_ty.rs:15–34](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/mlir_ty.rs#L15-L34).

Full-range u128 literal support must not be inferred from that type support. The inspected flat immediate encoding uses i64/u64 values. The legacy literal path also parses i64/u64. Wide constants in the probes were assembled from limbs or shifts. See [flatten.rs:6970–6984](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/hir/flatten.rs#L6970-L6984) and [expr.rs:3984–4000](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/codegen/lower/expr.rs#L3984-L4000).

### Wrapping arithmetic and wide products

Ordinary fixed-width integer addition, subtraction, and multiplication lower to `arith.addi/subi/muli`, without signed/unsigned overflow promises in the ordinary scalar path. The tested limb algorithms rely on modular results. `usize` is different: it has restricted range and checked operations, so it is not a suitable wrapping limb. See [flat arithmetic:34–69](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/codegen/flat/emit/arith.rs#L34-L69) and [usize checks:130–181](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/codegen/flat/emit/arith.rs#L130-L181).

The numeric method macros instantiate signed/unsigned widths through 64 bits, not i128/u128. Plain u128 operators worked, but that does not establish u128 methods such as `wrapping_mul()`. Compile-time evaluation has different range behavior and must not serve as the EVM arithmetic oracle. See [num.vx:642–645](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/num.vx#L642-L645), [1116–1119](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/num.vx#L1116-L1119), and [comptime_interpreter.rs:1219–1261](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/hir/comptime_interpreter.rs#L1219-L1261).

Libm contains a four-u64 `LibmU256` and an exact u64 multiply-high helper. However, `quick_mul_hi` deliberately approximates the high product and is unsuitable for exact EVM arithmetic. See [libm.vx:1097–1102](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/libm.vx#L1097-L1102), [9949–9962](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/libm.vx#L9949-L9962), and [10176–10213](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/libm.vx#L10176-L10213).

### Division and shifts

Signedness determines division, remainder, and right-shift operations. Host division checking inserts zero-divisor and signed-overflow guards. The original i128 overflow guard was defective; section 8 records its fix. See [flat.rs:436–467](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/codegen/flat.rs#L436-L467) and [VxLowering.cpp:6242–6311](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/dialect/VxLowering.cpp#L6242-L6311).

Ordinary scalar shifts do not provide an established EVM oversized-count guarantee. The tested algorithms explicitly guard the full U256 count before narrowing. They avoid limb shifts by 64. EVM DIV/MOD return zero on a zero divisor; SDIV handles MIN/−1 explicitly; SAR uses sign fill for counts at least 256. These are algorithm obligations, not consequences of host instructions.

### Executed arithmetic evidence

Probe 01 implemented four-u64 words with u128 intermediates. Twelve operations passed at O0/O2:

`ADD`, `SUB`, `MUL`, unsigned `LT`, `AND`, `OR`, `XOR`, `NOT`, `UDIV`, `UMOD`, `ADDMOD`, `MULMOD`.

- 2,409 cases per operation per optimization, including 1,000 full-width random triples.
- **57,816 complete outputs**, zero mismatches.
- ADDMOD retains the 257th sum bit. MULMOD reduces the full 512-bit product.
- Truncated-modular-result mutations were detected at both optimization levels.
- Actual native objects contain `adcq` and `mulxq`. No external bigint or heap-allocation helpers were required by that kernel.
- Test I/O and signal setup symbols are harness dependencies, not evidence of arithmetic runtime requirements.

Probe 02 independently used two u128 halves and four-u64 shift lanes. Its guarded semantics passed O0/O2/O3:

| Opcode | Cases per optimization |
|---|---:|
| DIV | 2,396 |
| MOD | 2,396 |
| SDIV | 2,396 |
| SMOD | 2,396 |
| SHL | 2,252 |
| SHR | 2,252 |
| SAR | 2,252 |

That is **49,020 complete matching outputs**. Cases include signed boundaries, asymmetric signs, full-width count limbs, and counts 0/1/63/64/65/127/128/129/191/192/255/256/257. Additional in-range shifts prevent random inputs from mostly testing zero results.

The two word representations demonstrate feasibility. They do not settle the best production representation or division algorithm.

## 5. Dispatch and calling conventions

Named function-pointer parameters and typed-local calls have parser, checker, flattening, and native lowering support. Arbitrary function-pointer call expressions use a separate checker path that rejects them at this pin. See [function type parser:304–319](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/parser/types.rs#L304-L319), [named calls:825–859](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/hir/check/calls.rs#L825-L859), [expression calls:167–207](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/hir/check/calls.rs#L167-L207), and [indirect emission:367–442](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/codegen/flat/emit/call.rs#L367-L442).

Probe 03 established:

| Shape | Executed status |
|---|---|
| Match/switch loop | Pass O0/O2/O3 |
| Table load into a typed local, then call | Pass O0/O2/O3 |
| Small fused add/mix pair | Pass O0/O2/O3 |
| Direct `table[opcode](state)` | Frontend rejection |
| Native function-array literal | Frontend rejection, E3018 |

The typed-local table was supplied by the host. That is a table-construction workaround, not proof of all-Vx function-array support. A raw state pointer and scalar status return work. General indirect aggregate-return support is not established.

Each supported variant/optimization matched ten complete Python-interpreter states. The long stream executed 262,144 operations under a 1 MiB stack limit. This establishes bounded execution for the tested stream, not recursive tail-call safety for arbitrary programs.

| O2 dispatcher symbol | Bytes | Instructions | Indirect calls | Conditional branches |
|---|---:|---:|---:|---:|
| Switch | 194 | 52 | 0 | 7 |
| Typed local | 108 | 41 | 1 | 4 |
| Fused pair | 304 | 87 | 0 | 9 |

Those are static object measurements, not throughput. Switch/fused handlers inline in this toy program. Sidecar LLVM IR was translated and independently optimized; actual executed-object disassembly is authoritative for machine-code claims.

No user-facing `musttail`, `tailcc`, `fastcc`, `ghccc`, or similar controls were found in the searched parser/checker/lowering/test paths. This does not prove impossibility through inline MLIR or unsearched paths. Ordinary indirect calls provide no guaranteed tail ABI.

**Current recommendation:** use a loop dispatcher as the simplest all-Vx correctness starting point. Benchmark before selecting the production execution strategy.

## 6. CPU tasks, workers, and atomics

### Implemented paths

`tasks { spawn { ... } }` has real scoped OS-thread implementation in the flat backend. Captures are checked for Send/Sync and borrow rules. Runtime start/end operations allocate captured environments, create threads, and join them. See [task checker:234–250](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/hir/check/tasks.rs#L234-L250), [thread traits:44–103](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/hir/check/thread_safety.rs#L44-L103), and [task runtime:21–131](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/rust_core/src/ffi/tasks.rs#L21-L131).

Important distinctions:

- `spawn on(Topology::CPU)` is placement and is inlined. It is not this task-concurrency path. See [VxLowering.cpp:654–679](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/dialect/VxLowering.cpp#L654-L679).
- Legacy code generation can execute task bodies serially. No-fallback enforcement is therefore essential for native parallelism evidence. See [driver.rs:1423–1488](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/driver.rs#L1423-L1488).
- A thread-start failure executes the task synchronously. Peer-dependent barriers can deadlock such a fallback.
- Task handles cannot escape freely into arbitrary structures. Scoped task support is not a general scheduler API.
- The runtime requests an 8 MiB stack per spawned task. Requested virtual stack space is not resident memory.
- A separate host-kernel worker path creates pthreads per dispatch, rather than maintaining a standing worker pool. See [vx_host_call.h:135–295](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/runtime/vx_host_call.h#L135-L295).

Native atomics use LLVM atomic load/store/RMW/CAS/fence operations. Integer widths through 64 bits, AtomicUsize, and AtomicBool are present. No AtomicU128/U256/Ptr API was located in the inspected library. Independent atomic limbs do not make a U256 update atomic. See [atomic.vx:139–243](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/sync/atomic.vx#L139-L243) and [399–443](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/sync/atomic.vx#L399-L443).

`fetch_update` with an Option-returning closure still declines on the flat path. Its deliberate rejection confirmed that `VX_NO_AST_FALLBACK=1` was effective. Direct fetch_add and CAS loops worked.

No channel, general Mutex, cancellation, park/resume scheduler, or persistent worker-pool API was established by the scoped searches. Design documents propose these features. Treat them as planned, not shipped evidence. See [waiting_tasks_channels_and_task_errors.md:140–180](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/docs/discussions/waiting_tasks_channels_and_task_errors.md#L140-L180).

### Executed worker results

- 360 schedules at O0/O2/O3, using 1/2/4 workers and fetch_add/CAS job claiming.
- Job counts included 0, 1, w−1, w, w+1, and 10,017, with two seeds and skewed work.
- **721,752 job results** and **3,606,120 checked slots** matched independent expectations.
- Separate visitation accounting detected missing and duplicate processing.
- Wrong-value, missing-record, duplicate-record, and duplicate-visit controls all failed as intended.
- Initial overlap accounting included startup sleep. That evidence alone did not establish useful-compute overlap.
- A separate supplement ran eight O0/O2 schedules with **80,136 jobs**, each containing 1,000–1,016 useful arithmetic iterations.
- Supplementary active windows contained no sleep, blocking call, I/O, barrier, or peer wait. Peaks reached 2/4, every worker contributed, and final active counts returned to zero after joins.

This establishes overlapping useful-compute intervals and distinct OS threads. It does not prove simultaneous instruction retirement, fairness, or speedup.

Maximum original sampled RSS/HWM was 81,372 KiB. Maximum sampled threads were five, including main. Maximum requested worker stacks totaled 32 MiB. These measurements must not be interpreted as per-worker resident-stack cost.

The fixed-compiler rerun used the 72-schedule worker smoke: **240,488 job outputs and 721,224 slots** passed. That smoke does not replace the original full schedule matrix or repeat every resource measurement.

## 7. Packaging, memory, alignment, and collections

### Compiler requirements are not runtime payload requirements

The ordinary CLI run path invokes `mlir-translate`, `opt`, `llc`, and clang. It uses host target settings and `mcpu=native`, with runner/runtime libraries on its link path. See [jit.rs:359–444](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/jit.rs#L359-L444) and [508–577](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/jit.rs#L508-L577).

That does not imply deployed callable libraries need the compiler, LLVM tools, MLIR runner utilities, or a GPU stack. Native C harnesses loaded the test libraries from clean runtime environments without compiler/LLVM search paths.

| Payload | Fixed compiler O2 stripped shared size | Tested runtime requirement |
|---|---:|---|
| Scalar / caller-owned U256 buffer | 14,000 bytes | No Vx runtime required |
| Allocation + Vec | 18,536 bytes | Deployed `libvx_std_core.so` |
| Tasks + atomics | 14,360 bytes | Deployed `libvx_std_core.so` |

These are shared-library sizes, not whole-application sizes or the size of a future EVM. The original legacy allocation+Vec library was 18,496 bytes. Do not mix it with the fixed-flat measurement.

The tested development runtime shared file was 913,976 bytes, or 641,848 bytes stripped. Its static archive was 21,584,808 bytes. A static archive is not a final static executable size. Experiment archives contain only their experiment objects, not that runtime archive.

Runtime dependencies observed for the tested deployments included libc/libgcc_s, not LLVM/MLIR. Missing-runtime controls failed for allocation/tasks and succeeded for scalar. Deploying the runtime beside dependent libraries with `$ORIGIN` worked. Exported work depended on caller inputs, so dependency-free scalar results were not merely unused-code elimination.

### Memory results and limitations

- Explicit 32/64-byte aligned allocations met their requested alignment.
- Growth preserved all initialized bytes. Newly required zeroes were written explicitly, not assumed from allocation.
- Four-u64 U256 records had size/array stride 32 bytes and C offsets 0/8/16/24.
- A 32-byte size does not imply 32-byte alignment.
- Native generic Vec stores pointer, length, and capacity, but its allocation helpers hardcode 16-byte alignment.
- A measured Vec pointer had residue 16 modulo 32 and 64. That allocation was not 32/64-byte aligned.
- Repeated allocation/grow/drop and C field/canary checks passed.
- C ASan/UBSan harnesses passed. They did not instrument Vx-generated memory accesses, and do not prove exhaustive leak or memory safety.
- OOM was not induced. Source failure handling was inspected without exhausting the machine.

Allocation helpers and Vec implementation: [alloc.vx:13–43](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/std/alloc.vx#L13-L43), [vec.vx:28–96](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/std/vec.vx#L28-L96).

The inspected HashMap exposes Rust-backed i32→i32/f32 FFI, not a ready U256-keyed EVM state map. `align_of` is not established as a supported intrinsic. Bulk pointer copy/zero APIs are incomplete in the inspected library. Pointer arithmetic exists despite stale source comments. See [hash_map.vx:8–40](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/std/hash_map.vx#L8-L40), [mem.vx:17–28](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/mem.vx#L17-L28), and [ptr.vx:14–25](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/stdlib/core/ptr.vx#L14-L25).

ABI documentation contains stale Linux and collection claims. Prefer matching source and executed deployment evidence over those broad statements.

## 8. Compiler defects, fixes, and upstream issues

### A. Incorrect i128 signed overflow guard

The original pass built −1 using a 64-bit constant constructor. For i128, generated IR compared the divisor against +2^64−1.

Consequences:

- MIN/−1 missed the overflow trap at O0/O2/O3 and reached undefined signed LLVM division.
- Valid MIN/(2^64−1) and MIN%(2^64−1) could be falsely rejected.
- Corresponding i64 overflow and unsigned zero-divisor controls trapped correctly.

The local fix constructs `APInt::getAllOnes(ty.getWidth())`. It corrects the shared division/remainder guard. See the original [VxLowering.cpp:6294–6305](https://github.com/vx-lang/Vx/blob/dc6a460873e191e394f5afaa827c1997e43633ac/src/dialect/VxLowering.cpp#L6294-L6305).

Two new tests check a valid positive divisor before the final overflow trap. They fail on the original compiler and pass fixed O0/O2/O3. Removing the final overflow makes each trap check fail. The full guarded semantics corpus and original i128 trap controls pass with the fix.

Upstream report: [#2278 — i128 division and remainder overflow guards](https://github.com/vx-lang/Vx/issues/2278).

### B. Struct-pointer field access and stores

The original flat backend lost struct layout information on indexed addresses. Constructed struct stores could also use a stack-slot pointer where an actual struct value was required.

The local fix in `src/codegen/flat/emit/aggregate.rs`:

1. Preserves aggregate layout on an indexed address.
2. Loads a constructed struct value before storing it into a struct destination.
3. Checks the destination too, preserving pointer-to-pointer assignment rather than copying the pointed-to record.
4. Leaves already-loaded struct values unchanged.

Regression coverage includes constructed/named/read struct stores, borrowed field mutation, pointer-to-pointer storage, and Vec growth with complete field checks. The baseline fixture fails on the original compiler. Fixed O0/O2/O3 runs pass. Three independently wrong expected fields are all detected.

The original malformed-store reproducer now compiles into callable libraries at O0/O2/O3. C and independent Python field/canary checks pass. All six payload libraries now compile through the flat backend, without an explicit legacy workaround.

Existing upstream issue: [#2154 — indexed raw-pointer struct stores](https://github.com/vx-lang/Vx/issues/2154). Posted evidence: [before/after verification](https://github.com/vx-lang/Vx/issues/2154#issuecomment-6423209341) and [final coverage verification](https://github.com/vx-lang/Vx/issues/2154#issuecomment-6423374877).

### C. Integration coverage expectation

The broader integration run finished with **440 passed, 1 failed**. Its sole failure explicitly reported improved flat-backend coverage. `backend/pass/borrow_element_through_pointer.vx` no longer belonged in `KNOWN_DECLINES`.

That single entry is now removed locally. Fresh native O0/O2 runs both print exactly `42 42 47 43 42` and exit 0, with fallback disabled. The full corpus was not repeated after this one-line update. We therefore do not claim a newly executed 441/441 pass.

The Rust library suite passed **826 tests**, with 1 ignored. `git diff --check` passes. The complete six-file patch passes `git apply --check` against the original clean pin.

An unrelated existing Rust comment-formatting difference was left unchanged. The repository check updater did not support the runtime RUN prefixes used by the new tests, so direct FileCheck validation retained their runtime expectations.

### Delivery state

The compiler fixes, three new tests, and coverage expectation update are local and uncommitted. No branch was pushed, no PR was opened, and no deployment occurred. GitHub issue/comment publication was explicitly requested. No repository or organization issue templates were available, so the new report used standard bug-report sections.

## 9. Current per-probe gate summary

| Probe | Original pinned compiler | Locally fixed compiler |
|---|---|---|
| 01 Exact U256 | Pass O0/O2 | Full pass, 57,816 outputs |
| 02 Guarded semantics | Guarded outputs pass; i128 trap control fails | Full pass O0/O2/O3, 49,020 guarded outputs plus trap controls |
| 03 Dispatch | Supported shapes pass; optional shapes rejected | Same supported shapes pass O0/O2/O3; limitations unchanged |
| 04 Workers | Full 360-schedule pass plus useful-compute supplement | 72-schedule smoke passes |
| 05 Payload/alignment | Partial flat success; scalar/Vec explicit legacy workaround | All six O0/O2 libraries flat; harnesses pass |

All five fixed-compiler rerun processes exit 0. Original mixed reports and their nonzero exits are retained, not rewritten into historical passes.

## 10. Remaining EVM and Vx work

### Required for an actual EVM

- Select a fork and freeze opcode, gas, exceptional-halt, and transaction semantics.
- Implement and test the stack, bytecode decoding, jump destinations, memory expansion, and complete opcode behavior.
- Add exact EXP, SIGNEXTEND, BYTE, signed comparisons, hashing, environment operations, and other behavior beyond the tested arithmetic subset.
- Define account/storage access, journaling, nested calls, reverts, logs, refunds, warm/cold access, precompiles, and fork-dependent behavior.
- Define ownership and lifetime of host-provided buffers and state.
- Differential-test against an independent fork-specific reference. Arithmetic probes do not establish whole-EVM compatibility.

### Required for deterministic parallel transactions

- Define state versions, read/write sets, conflict validation, retries, and deterministic commit order.
- Test conflicting transactions, rollback, cross-transaction visibility, starvation, and serial fallback.
- Preserve sequential observable behavior. Independent worker jobs are not a transaction executor.
- Avoid full-state copying per worker unless measurements justify it.
- Keep useful worker loops long-lived within a bounded task scope. Do not spawn a thread per opcode or tiny job.

### Vx improvements that could help

- Submit and integrate the two local compiler fixes and their tests.
- Improve full-range integer literal handling, if required by the application.
- Support direct function-table expressions and native function-array construction, if measurements favor them.
- Add enforceable tail-call/calling-convention controls only if the chosen execution strategy needs them.
- Provide layout-aware aligned collections and a suitable U256 state map, or use an explicit host-owned state boundary.
- Improve indirect aggregate returns if APIs need them. The tested scalar-status ABI avoids that requirement.
- Investigate reusable worker pools, failure handling, and channels based on measured scheduling needs.

These are potential follow-on contributions, not all prerequisites for a first prototype.

## 11. Performance work still needed

No current measurement supports the 20× target. Before comparing:

1. Freeze baseline revisions, fork, workload corpus, hardware, and execution/state boundaries.
2. Compare identical work and validate identical outputs.
3. Include representative arithmetic, storage-heavy, memory-heavy, call-heavy, and conflict-heavy workloads.
4. Separate single-transaction latency, block throughput, and scaling.
5. Separate compilation/predecoding, process startup, host I/O, and oracle work from kernel measurements.
6. Include warmup and at least ten measured repetitions. Report median and spread.
7. Serialize timed windows under the shared lock and record host contention.
8. Charge retries, validation, scheduling, and commit work to parallel execution.
9. Report cold/warm cache and state-access conditions explicitly.

The subsequent [matched C/Vx arithmetic and dispatch probes](performance.md) cover the first kernel comparisons. Representative EVM workloads and the end-to-end baseline remain outstanding. Their results should guide representation and dispatch, rather than instruction counts alone.

Thread creation/join overhead must be measured separately from steady-state job processing. Thread-start failure behavior and fairness remain untested.

## 12. Evidence availability and reproduction limits

The author's local research archive retains the shared protocol, five probe specs, hardware/build manifests, Vx sources, Python oracles, fixed seeds, complete outputs, native artifacts, IR/disassembly, compiler patches, and regression logs. Those machine-specific files and compiler builds are not shipped in this documentation snapshot.

Public source citations above refer to the exact upstream pin. The compiler issue reports in section 8 contain public reproducer and verification details. The tests described here require the recorded source pin plus the local fixes; a stock compiler with the same version string is not equivalent.

The original smoke retains the original compiler failures. Fixed-compiler reruns are separate evidence, not rewrites of the original results. There is no portable build/test command in this repository yet; publishing a self-contained experiment harness is follow-up work, not an existing feature.

## 13. Prioritized next steps, not yet executed

1. **Freeze the first EVM milestone.** Select the fork, implementation layout, host/state interface, and benchmark contract. The repository now exists; do not treat the 20× target as an achieved result.
2. **Preserve and submit the compiler patch.** Keep the six-file change independently reviewable. This requires separate authorization for commits/push/PR actions.
3. **Build the sequential correctness core.** Reuse the tested limb algorithms and an ordinary dispatcher initially. Label any opcode subset as a subset, not a complete EVM.
4. **Extend matched performance probes.** The [C/Vx kernel pass](performance.md) is complete. Measure representative EVM workloads before locking the word representation or interpreter strategy.
5. **Establish aligned state storage and rollback.** Test a small U256-keyed store or host boundary against independent expectations.
6. **Add bounded transaction parallelism.** Validate conflicts and ordered commit against the sequential core before making speed claims.

No EVM implementation was started after the request to consolidate these findings. The next implementation scope remains a decision, not an implicit completed plan.

Next: read the [matched performance results](performance.md) and review the [draft execution contract](../SPEC.md).
