#!/usr/bin/env python3
"""Generates the hash permutations on the CPU's crypto instructions, written in Vx.

Vx has no vector types, so each permutation is one `mlir!` block in the LLVM dialect
that loads the state from memory, runs on vector<2 x i64> / vector<4 x i32> registers
with the ARMv8 SHA3 and SHA2 intrinsics, and stores it back.

  python3 scripts/gen_hash_hw.py arm      > src/evm/hash_hw_arm.vx
  python3 scripts/gen_hash_hw.py portable > src/evm/hash_hw_portable.vx

scripts/build-lib.sh builds one of the two, by the host's CPU features.

Keccak follows the XKCP/K12 ARMv8 SHA3 design: one lane per register, EOR3 for the
column parities, RAX1 for D, XAR for theta plus rho, BCAX for chi.
"""
import sys

RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
]
# Rotation offsets r[x][y] (left rotations).
ROT = [
    [0, 36, 3, 41, 18],
    [1, 44, 10, 45, 2],
    [62, 6, 43, 15, 61],
    [28, 55, 25, 21, 56],
    [27, 20, 39, 8, 14],
]
K256 = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]
V2 = 'vector<2xi64>'
V4 = 'vector<4xi32>'


def s64(v):
    """An i64 MLIR literal for the unsigned 64-bit value v."""
    return v - (1 << 64) if v >= 1 << 63 else v


def s32(v):
    return v - (1 << 32) if v >= 1 << 31 else v


def keccak_arm(w):
    w('// Keccak-f[1600] on the 25 lanes at st; lane (x, y) is st[x + 5 * y].')
    w('fn keccak_f1600_hw(st: *mut u64) -> void {')
    w('  mlir!( inputs : (%st = st : !llvm.ptr), dialects : ["llvm"]) {')
    w(f'    %u = llvm.mlir.undef : {V2}')
    w('    %z = llvm.mlir.constant(0 : i64) : i64')
    for i in range(25):
        w(f'    %p{i} = llvm.getelementptr %st[{i}] : (!llvm.ptr) -> !llvm.ptr, i64')
        w(f'    %l{i} = llvm.load %p{i} : !llvm.ptr -> i64')
        w(f'    %a{i}_0 = llvm.insertelement %l{i}, %u[%z : i64] : {V2}')
    cur = [f'%a{i}_0' for i in range(25)]
    for r in range(24):
        a = cur
        c = []
        for x in range(5):
            t = f'%t{x}_{r}'
            w(f'    {t} = llvm.call_intrinsic "llvm.aarch64.crypto.eor3u.v2i64"({a[x]}, {a[x + 5]}, {a[x + 10]}) : ({V2}, {V2}, {V2}) -> {V2}')
            w(f'    %c{x}_{r} = llvm.call_intrinsic "llvm.aarch64.crypto.eor3u.v2i64"({t}, {a[x + 15]}, {a[x + 20]}) : ({V2}, {V2}, {V2}) -> {V2}')
            c.append(f'%c{x}_{r}')
        d = []
        for x in range(5):
            # d[x] = c[x-1] ^ rol(c[x+1], 1)
            w(f'    %d{x}_{r} = llvm.call_intrinsic "llvm.aarch64.crypto.rax1"({c[(x - 1) % 5]}, {c[(x + 1) % 5]}) : ({V2}, {V2}) -> {V2}')
            d.append(f'%d{x}_{r}')
        # theta + rho + pi: b[pi(x, y)] = rol(a[x, y] ^ d[x], r[x, y]); XAR rotates right.
        b = [None] * 25
        for y in range(5):
            for x in range(5):
                src = x + 5 * y
                dst = y + 5 * ((2 * x + 3 * y) % 5)
                name = f'%b{dst}_{r}'
                rot = ROT[x][y]
                if rot == 0:
                    w(f'    {name} = llvm.xor {a[src]}, {d[x]} : {V2}')
                else:
                    w(f'    %k{dst}_{r} = llvm.mlir.constant({(64 - rot) % 64} : i64) : i64')
                    w(f'    {name} = llvm.call_intrinsic "llvm.aarch64.crypto.xar"({a[src]}, {d[x]}, %k{dst}_{r}) : ({V2}, {V2}, i64) -> {V2}')
                b[dst] = name
        # chi: a = b0 ^ (~b1 & b2) = BCAX(b0, b2, b1); iota on lane 0.
        nxt = [None] * 25
        for y in range(5):
            for x in range(5):
                i = x + 5 * y
                b0, b1, b2 = b[i], b[(x + 1) % 5 + 5 * y], b[(x + 2) % 5 + 5 * y]
                name = f'%a{i}_{r + 1}'
                if i == 0:
                    w(f'    %x0_{r} = llvm.call_intrinsic "llvm.aarch64.crypto.bcaxu.v2i64"({b0}, {b2}, {b1}) : ({V2}, {V2}, {V2}) -> {V2}')
                    w(f'    %rc{r} = llvm.mlir.constant(dense<[{s64(RC[r])}, 0]> : {V2}) : {V2}')
                    w(f'    {name} = llvm.xor %x0_{r}, %rc{r} : {V2}')
                else:
                    w(f'    {name} = llvm.call_intrinsic "llvm.aarch64.crypto.bcaxu.v2i64"({b0}, {b2}, {b1}) : ({V2}, {V2}, {V2}) -> {V2}')
                nxt[i] = name
        cur = nxt
    for i in range(25):
        w(f'    %o{i} = llvm.extractelement {cur[i]}[%z : i64] : {V2}')
        w(f'    llvm.store %o{i}, %p{i} : i64, !llvm.ptr')
    w('    macro.yield')
    w('  };')
    w('}')


def sha256_arm(w):
    w('// Compresses one 64-byte block at blk into the state words a..h at st.')
    w('fn sha256_block_hw(st: *mut u32, blk: *mut u8) -> void {')
    w('  mlir!( inputs : (%st = st : !llvm.ptr, %blk = blk : !llvm.ptr), dialects : ["llvm"]) {')
    w('    %s4 = llvm.getelementptr %st[4] : (!llvm.ptr) -> !llvm.ptr, i32')
    w(f'    %abcd_in = llvm.load %st : !llvm.ptr -> {V4}')
    w(f'    %efgh_in = llvm.load %s4 : !llvm.ptr -> {V4}')
    w('    %u8 = llvm.mlir.undef : vector<16xi8>')
    for q in range(4):
        w(f'    %mp{q} = llvm.getelementptr %blk[{16 * q}] : (!llvm.ptr) -> !llvm.ptr, i8')
        w(f'    %mb{q} = llvm.load %mp{q} : !llvm.ptr -> vector<16xi8>')
        # Big-endian words: reverse the bytes of each 32-bit word.
        w(f'    %mr{q} = llvm.shufflevector %mb{q}, %u8 [3, 2, 1, 0, 7, 6, 5, 4, 11, 10, 9, 8, 15, 14, 13, 12] : vector<16xi8>')
        w(f'    %m{q}_0 = llvm.bitcast %mr{q} : vector<16xi8> to {V4}')
    m = [f'%m{q}_0' for q in range(4)]
    abcd, efgh = '%abcd_in', '%efgh_in'
    for i in range(16):
        k = ', '.join(str(s32(v)) for v in K256[4 * i:4 * i + 4])
        w(f'    %k{i} = llvm.mlir.constant(dense<[{k}]> : {V4}) : {V4}')
        w(f'    %wk{i} = llvm.add {m[i % 4]}, %k{i} : {V4}')
        if i < 12:
            # Extends the schedule: W[4i+16 ..] from W[4i ..], W[4i+4 ..], W[4i+8 ..], W[4i+12 ..].
            w(f'    %su0_{i} = llvm.call_intrinsic "llvm.aarch64.crypto.sha256su0"({m[i % 4]}, {m[(i + 1) % 4]}) : ({V4}, {V4}) -> {V4}')
            w(f'    %su1_{i} = llvm.call_intrinsic "llvm.aarch64.crypto.sha256su1"(%su0_{i}, {m[(i + 2) % 4]}, {m[(i + 3) % 4]}) : ({V4}, {V4}, {V4}) -> {V4}')
            m[i % 4] = f'%su1_{i}'
        w(f'    %h{i} = llvm.call_intrinsic "llvm.aarch64.crypto.sha256h"({abcd}, {efgh}, %wk{i}) : ({V4}, {V4}, {V4}) -> {V4}')
        w(f'    %h2_{i} = llvm.call_intrinsic "llvm.aarch64.crypto.sha256h2"({efgh}, {abcd}, %wk{i}) : ({V4}, {V4}, {V4}) -> {V4}')
        abcd, efgh = f'%h{i}', f'%h2_{i}'
    w(f'    %abcd_out = llvm.add {abcd}, %abcd_in : {V4}')
    w(f'    %efgh_out = llvm.add {efgh}, %efgh_in : {V4}')
    w(f'    llvm.store %abcd_out, %st : {V4}, !llvm.ptr')
    w(f'    llvm.store %efgh_out, %s4 : {V4}, !llvm.ptr')
    w('    macro.yield')
    w('  };')
    w('}')


def main():
    kind = sys.argv[1] if len(sys.argv) > 1 else ''
    if kind not in ('arm', 'portable'):
        sys.exit('usage: gen_hash_hw.py arm|portable')
    out = []
    w = out.append
    w(f'// Generated by scripts/gen_hash_hw.py {kind}. Do not edit by hand.')
    if kind == 'arm':
        w('// The hash permutations on the ARMv8 SHA3 and SHA2 instructions (FEAT_SHA3, FEAT_SHA256).')
        w('')
        keccak_arm(w)
        w('')
        sha256_arm(w)
    else:
        w('// The hash permutations for a CPU without the ARMv8 SHA3 and SHA2 instructions: the')
        w('// scalar Vx versions in keccak.vx and pc_sha256.vx.')
        w('')
        w('fn keccak_f1600_hw(st: *mut u64) -> void { keccak_f(st); }')
        w('')
        w('fn sha256_block_hw(st: *mut u32, blk: *mut u8) -> void { evmvx_sha256_blocks_vx(st, blk, 1u64); }')
    print('\n'.join(out))


if __name__ == '__main__':
    main()
