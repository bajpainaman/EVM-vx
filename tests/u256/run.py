#!/usr/bin/env python3
"""Differential test: src/evm/u256.vx against independent Python-int EVM rules.

The oracle below uses Python big integers and the EVM definitions directly;
it shares no code or algorithm with the limb implementation.

Usage: tests/u256/run.py [--smoke] [-O 0 2 3]
"""
import argparse
import os
import random
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / 'build'
VXC = ROOT / 'scripts' / 'vxc'
CLANG = os.environ.get('CLANG', '/usr/lib/llvm-22/bin/clang')
RUNTIME = Path(os.environ.get(
    'VX_RUNTIME_LIB_DIR', '/data/fast/vx-toolchain/vx-compiler-fixes-build/target/debug'))

MASK = (1 << 256) - 1
SIGN = 1 << 255
SEED = 0x256_20261026

OPS = {
    0x01: 'ADD', 0x02: 'MUL', 0x03: 'SUB', 0x04: 'DIV', 0x05: 'SDIV', 0x06: 'MOD',
    0x07: 'SMOD', 0x08: 'ADDMOD', 0x09: 'MULMOD', 0x0a: 'EXP', 0x0b: 'SIGNEXTEND',
    0x10: 'LT', 0x11: 'GT', 0x12: 'SLT', 0x13: 'SGT', 0x14: 'EQ', 0x15: 'ISZERO',
    0x16: 'AND', 0x17: 'OR', 0x18: 'XOR', 0x19: 'NOT', 0x1a: 'BYTE',
    0x1b: 'SHL', 0x1c: 'SHR', 0x1d: 'SAR',
}


def signed(x):
    return x - (1 << 256) if x & SIGN else x


def expected(op, a, b, c):
    """EVM semantics; a is the top of the stack."""
    name = OPS[op]
    if name == 'ADD': return (a + b) & MASK
    if name == 'MUL': return (a * b) & MASK
    if name == 'SUB': return (a - b) & MASK
    if name == 'DIV': return a // b if b else 0
    if name == 'MOD': return a % b if b else 0
    if name == 'SDIV':
        if b == 0: return 0
        x, y = signed(a), signed(b)
        q = abs(x) // abs(y)
        return (-q if (x < 0) != (y < 0) else q) & MASK
    if name == 'SMOD':
        if b == 0: return 0
        x, y = signed(a), signed(b)
        r = abs(x) % abs(y)
        return (-r if x < 0 else r) & MASK
    if name == 'ADDMOD': return (a + b) % c if c else 0
    if name == 'MULMOD': return (a * b) % c if c else 0
    if name == 'EXP': return pow(a, b, 1 << 256)
    if name == 'SIGNEXTEND':
        if a >= 31: return b
        bit = 8 * a + 7
        low = b & ((1 << (bit + 1)) - 1)
        return (low | (MASK ^ ((1 << (bit + 1)) - 1))) if b >> bit & 1 else low
    if name == 'LT': return int(a < b)
    if name == 'GT': return int(a > b)
    if name == 'SLT': return int(signed(a) < signed(b))
    if name == 'SGT': return int(signed(a) > signed(b))
    if name == 'EQ': return int(a == b)
    if name == 'ISZERO': return int(a == 0)
    if name == 'AND': return a & b
    if name == 'OR': return a | b
    if name == 'XOR': return a ^ b
    if name == 'NOT': return a ^ MASK
    if name == 'BYTE': return (b >> (8 * (31 - a))) & 0xff if a < 32 else 0
    if name == 'SHL': return (b << a) & MASK if a < 256 else 0
    if name == 'SHR': return b >> a if a < 256 else 0
    if name == 'SAR': return (signed(b) >> min(a, 256)) & MASK
    raise ValueError(name)


def boundary_values():
    v = {0, 1, 2, 3, 7, 8, 30, 31, 32, 33, 63, 64, 65, 255, 256, 257,
         MASK, MASK - 1, SIGN, SIGN - 1, SIGN + 1}
    for k in (8, 16, 31, 32, 63, 64, 65, 127, 128, 129, 191, 192, 193, 248, 255):
        v.update(((1 << k) - 1, 1 << k, (1 << k) + 1, MASK ^ ((1 << k) - 1)))
    v.add(1 << 64 << 64 << 64)  # a count whose only set bit is above limb 0
    return sorted(x & MASK for x in v)


def cases(smoke):
    rng = random.Random(SEED)
    values = boundary_values()
    if smoke:
        values = values[::4]
    out = []
    moduli = [0, 1, 2, 3, MASK, SIGN, (1 << 128) + 1, rng.getrandbits(256)]
    for op in OPS:
        for a in values:
            for b in values:
                out.append((op, a, b, moduli[(a ^ b) % len(moduli)]))
        for _ in range(50 if smoke else 1000):
            # Mix widths so random cases also exercise small-operand fast paths.
            a, b, c = (rng.getrandbits(rng.choice((8, 64, 128, 200, 256))) for _ in range(3))
            out.append((op, a, b, c))
    return out


def limbs(x):
    return ' '.join(f'{(x >> (64 * i)) & ((1 << 64) - 1):016x}' for i in range(4))


def decode(line):
    words = line.split()
    if len(words) != 4 or any(len(w) != 16 for w in words):
        raise ValueError(f'malformed output line: {line!r}')
    return sum(int(w, 16) << (64 * i) for i, w in enumerate(words))


def build(opt):
    BUILD.mkdir(exist_ok=True)
    obj = BUILD / f'u256-driver-O{opt}.o'
    exe = BUILD / f'u256-driver-O{opt}'
    for p in (obj, exe):
        p.unlink(missing_ok=True)
    subprocess.run([str(VXC), str(ROOT / 'tests/u256/driver.vx'), f'-O{opt}', '-c', '-o', str(obj)],
                   check=True, cwd=ROOT, stdout=subprocess.DEVNULL)
    subprocess.run([CLANG, str(obj), f'-L{RUNTIME}', '-lvx_std_core', f'-Wl,-rpath,{RUNTIME}',
                    '-lm', '-o', str(exe)], check=True)
    return exe


def run(exe, rows):
    text = f'{len(rows):016x}\n' + ''.join(
        f'{op:016x} {limbs(a)} {limbs(b)} {limbs(c)}\n' for op, a, b, c in rows)
    proc = subprocess.run([str(exe)], input=text, capture_output=True, text=True, timeout=600)
    if proc.returncode != 0:
        raise RuntimeError(f'{exe.name} exited {proc.returncode}: {proc.stderr[-500:]}')
    lines = proc.stdout.splitlines()
    if len(lines) != len(rows):
        raise RuntimeError(f'{exe.name} printed {len(lines)} results for {len(rows)} cases')
    return [decode(line) for line in lines]


def check(rows, got):
    per_op = {name: [0, 0] for name in OPS.values()}
    first = {}
    for (op, a, b, c), g in zip(rows, got):
        name = OPS[op]
        per_op[name][0] += 1
        want = expected(op, a, b, c)
        if g != want:
            per_op[name][1] += 1
            first.setdefault(name, (a, b, c, want, g))
    return per_op, first


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--smoke', action='store_true')
    ap.add_argument('-O', dest='opts', type=int, nargs='+', default=[0, 2, 3])
    args = ap.parse_args()
    rows = cases(args.smoke)

    # Negative control: the checker must reject a corrupted result.
    bad = list(expected(*r) for r in rows[:1])
    bad[0] ^= 1 << 200
    if not check(rows[:1], bad)[1]:
        sys.exit('negative control failed: checker accepted a corrupted result')

    failed = False
    for opt in args.opts:
        exe = build(opt)
        per_op, first = check(rows, run(exe, rows))
        total = sum(n for n, _ in per_op.values())
        bad_n = sum(m for _, m in per_op.values())
        print(f'O{opt}: {total} cases, {bad_n} mismatches')
        for name, (a, b, c, want, g) in first.items():
            failed = True
            print(f'  {name}: {per_op[name][1]}/{per_op[name][0]} wrong; first a={a:#x} b={b:#x} '
                  f'c={c:#x}\n    want {want:#x}\n    got  {g:#x}')
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
