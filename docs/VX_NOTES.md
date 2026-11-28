# Writing Vx for EVM-vx

Rules learned from the compiler. The code builds with the Vx v0.0.5 release (`vxc 0.0.5 (5b92da4f)`), which includes the earlier local i128-guard and struct-pointer fixes (#2278, #2154). The rules first came from upstream dc6a460; [Borrow checking](#borrow-checking-vx-005) lists what v0.0.5 changed. Follow them in all `src/` code.

## Build

- `scripts/vxbuild <entry.vx> <out.o> [0-3]` runs `vxc -a emit-obj` with `LLVM_BIN` on `PATH` (`vxc` calls `llvm-config`, `mlir-translate`, `opt` and `llc`). Since v0.0.5 the object leaves `extern` symbols undefined, so host callbacks link in later. The compiler targets the host, so `memmove` and `memset` stay libc calls. The earlier `--emit-llvm` → `mlir-translate` → `llc` path emitted no target triple, so `llc` expanded them into byte loops.
- `--package NAME=DIR` makes a module's functions visible to an entry file but emits only the entry's own code. A per-module build needs each file to import its dependencies, which `src/evm` does not do yet.
- `scripts/build-lib.sh` concatenates the files listed in `src/evm/MANIFEST` into one module, `build/evmvx.vx`, and builds `build/libevmvx.a`. The whole library is one compilation unit. A struct whose field type comes from another module cannot be indexed through a pointer ("an index whose element type is not modelled").
- A library needs no Vx runtime unless it uses `println!` or the runtime's own helpers.

## Syntax limits

| Fails | Write instead |
|---|---|
| `0x1f` (hex literal) | `31`. Generate large constants with a script. |
| `[u64; 8]` (fixed array) | A heap buffer: `let p: *mut u64 = <malloc>;` |
| `match x { .. }` where `x` is not `i32` | `let o: i32 = x as i32; match o { .. }` |
| `pub struct S { .. }` | `struct S { .. }`. Only functions take `pub`. |
| `p as *mut T` between pointer types | Assign through `*mut i8`, which converts implicitly both ways: `let raw: *mut i8 = malloc(n); let p: *mut T = raw;` |
| `{ .. }` as a bare block statement | It does not parse. Use a separate function, or let the reference's last use end the borrow. |
| `(p[0u64].buf)[i]` | `let q: *mut U = unsafe { p[0u64].buf };` then `q[i]`. Flat codegen cannot type the nested index, and the AST fallback overflows the compiler's stack on this library. |
| A function returning `&mut T` | Return a raw pointer, e.g. `offset_mut(base, i as i64)` from `core::ptr`, or an index. |
| `r.word = f(x);` where `word` is a struct-typed field | `let v: U256 = f(x); r.word = U256 { l0: v.l0, l1: v.l1, l2: v.l2, l3: v.l3 };` A struct literal on the right-hand side is required. |
| `let p = some_ptr_fn();` | Annotate pointer locals: `let p: *mut T = some_ptr_fn();` |
| `buf[135]` read through a `*mut u8` | `buf[135u64]`. An untyped literal index on a load takes the element type, so 135 wraps to -121 and the load reads before the buffer. Stores are not affected. Always type literal indexes as `u64`. |

## Things that work

- Structs passed and returned by value (`impl Copy for U256 {}`), including nested struct values in locals.
- `*mut U256` indexing by value: `let v: U256 = unsafe { p[i] }; unsafe { p[i] = v; }`.
- Recursion, `loop`, `while`, `break`, `continue`, `const NAME: u64 = 5u64;`.
- `extern "C" { fn memcpy(d: *mut u8, s: *const u8, n: u64) -> *mut u8; }` and calls to host functions defined in Rust or C.
- `pub fn` exports an unmangled C symbol.
- u128 arithmetic, `as` casts between integer types (u128 to u64 truncates).
- `sizeof<T>()` returns `i64`.

## Borrow checking (Vx 0.0.5)

`let r: &mut T = unsafe { &mut p[0u64] };` borrows the pointer variable `p` until the last use of `r`. Each of these then fails with E4002 or E4004:

- Pass or read `p` while `r` is still used later.
- Copy a pointer field out of `r` (`let q: *mut U = r.buf;`) and use `q` after `p` is used again. The copy keeps the borrow.
- Pass such a pointer to a function. The checker follows return provenance, so `dup_bytes(q, n)` keeps the borrow too.

Most of these errors are real: a helper given `p` reads or writes the struct while `r` claims it exclusively. Fixes, best first:

1. End the reference before the call. If the code needs it again, take a new one after the call.
2. Borrow shared when nothing writes the struct: `let e: &VxEnv = unsafe { &env[0u64] };`. A function that only reads takes `e: &T` (see `floor_gas`). A `&mut` passes where `&T` is expected.
3. Access fields through the raw pointer: `unsafe { p[0u64].f }` to read and `unsafe { p[0u64].f = v; }` to write. Use this when the struct is shared state that helpers also change, as `Ctx` and `BlockExec` are (`acc_load`, `evmvx_block_finish`). It claims no exclusivity, so it hides nothing.

Never reborrow through a copy of the pointer (`let pp: *mut T = p; let r: &mut T = unsafe { &mut pp[0u64] };`). That keeps the aliasing and hides it from the checker.

The checker has three limits in Vx 0.0.5:

- Liveness is by name. A dead `&mut` re-declared under the same name keeps the first borrow (E4004). Use a new name.
- Liveness follows source order, not control flow. In `if c { q.a = 1u64; return p; } q.a = 2u64;` the `return p` fails, although `q` is dead on that path. Write through the raw pointer on the early-exit path, or take `q` after the early exits.
- A shared `&T` is not checked against writes through the raw pointer. `let d: &T = unsafe { &p[0u64] }; touch(p); d.a` compiles even when `touch` writes `p[0u64].a`. Use `&T` only for data that nothing writes.

## Crypto instructions and `mlir!`

An `mlir!` block is inline MLIR: `vxc` parses and verifies it, wraps it in a private function, and inlines it at -O1 and above. It reaches what Vx has no syntax for:

- `llvm.intr.bswap` for big-endian loads and stores (`load_be64`, `put_be64` in `bytes.vx`).
- Whole-register vectors and the ARMv8 SHA3 (`EOR3`, `RAX1`, `XAR`, `BCAX`) and SHA2 (`SHA256H/H2/SU0/SU1`) intrinsics through `llvm.call_intrinsic`. `scripts/gen_hash_hw.py` writes the Keccak-f[1600] and SHA-256 permutations as one block each into `hash_hw_arm.vx`.
- `i256` arithmetic (`llvm.add %x, %y : i256` compiles to an `adds`/`adcs` chain).

A block that returns nothing still ends in `macro.yield`. A large block stays a call (`vx_flat_mlir_N` in a profile); small ones are inlined.

A block cannot be chosen by target ISA yet (the MLIR talk lists this as a known limit). `scripts/build-lib.sh` therefore picks `hash_hw_arm.vx` or `hash_hw_portable.vx` by the host compiler's `__ARM_FEATURE_SHA3` and `__ARM_FEATURE_SHA2`; set `EVMVX_HASH=arm|portable` to override. The portable file calls the scalar Vx permutations, which stay the reference.

On Apple cores, neither fully unrolling scalar Keccak nor lazy rotation (folding rho into `eor`/`bic` shifted operands) helps: the round loop measured 238 ns per permutation, the lazy-rotation version 351 ns.

## Tasks

- `spawn` rejects a captured `*mut T`: a raw pointer is not `Send` (E3055).
- A captured `&T` needs `T: Sync`. A struct with raw-pointer fields is not `Sync` until it says so: `unsafe impl Sync for BlockExec {}`, as the standard library does for `Vec`. The claim is yours to keep: tasks may only read the struct's fields, and write only data that no other task touches.
- Each `spawn` starts an OS thread, about 14 µs on an M4 Pro. A library that uses `tasks` links `libvx_std_core` from the release's `lib/`.

## Style

- Use typed literals (`0u64`, `1u128`) in arithmetic to avoid inference surprises.
- Keep memory-resident structs flat where you can: scalars, pointers and `U256`.
