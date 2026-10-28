# Writing Vx for EVM-vx

Rules learned from the pinned compiler (upstream Vx dc6a460 plus the local fixes). Each rule is a construct that fails today and the form that works. Follow them in all `src/` code.

## Build

- `scripts/vxbuild <entry.vx> <out.o> [0-3]` compiles through `--emit-llvm`, `mlir-translate`, `opt` and `llc`. Use it instead of `vxc -c`. `vxc -c` resolves `extern` symbols at compile time, so it rejects host callbacks that are linked in later.
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
| `p[i].field` read or write | `let r: &mut T = unsafe { &mut p[i] }; r.field` |
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

## Style

- Use typed literals (`0u64`, `1u128`) in arithmetic to avoid inference surprises.
- Keep memory-resident structs flat where you can: scalars, pointers and `U256`.
