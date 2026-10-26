#!/usr/bin/env python3
"""Keccak-256 in src/evm/keccak.vx against pycryptodome, through build/libevmvx.so.

Build first: scripts/build-lib.sh 2 && gcc -shared build/evmvx-O2.o -o build/libevmvx.so
"""
import ctypes
import random
import sys
from pathlib import Path

from Crypto.Hash import keccak

ROOT = Path(__file__).resolve().parents[2]


def main():
    lib = ctypes.CDLL(str(ROOT / 'build' / 'libevmvx.so'))
    fn = lib.evmvx_test_keccak
    fn.argtypes = [ctypes.c_char_p, ctypes.c_uint64, ctypes.c_char_p]
    rng = random.Random(1600)
    # Every length around the 136-byte rate boundaries, plus long random inputs.
    lengths = list(range(0, 300)) + [543, 544, 545, 1088, 4096, 100000]
    bad = 0
    for n in lengths:
        data = bytes(rng.getrandbits(8) for _ in range(n))
        out = ctypes.create_string_buffer(32)
        fn(data, n, out)
        want = keccak.new(digest_bits=256, data=data).digest()
        if out.raw != want:
            bad += 1
            if bad <= 3:
                print(f'len {n}: got {out.raw.hex()} want {want.hex()}')
    print(f'{len(lengths)} lengths, {bad} mismatches')
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
