#!/usr/bin/env python3
"""BLAKE2b F precompile (0x09) against hashlib and a Python model of F.

Build first: scripts/build-testlib.sh 2
"""
import hashlib
import random
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from pclib import run_precompile  # noqa: E402

IV = [0x6A09E667F3BCC908, 0xBB67AE8584CAA73B, 0x3C6EF372FE94F82B, 0xA54FF53A5F1D36F1,
      0x510E527FADE682D1, 0x9B05688C2B3E6C1F, 0x1F83D9ABFB41BD6B, 0x5BE0CD19137E2179]
SIGMA = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
]
M64 = (1 << 64) - 1


def rotr(x, n):
    return ((x >> n) | (x << (64 - n))) & M64


def model_f(rounds, h, m, t0, t1, f):
    v = h[:] + IV[:]
    v[12] ^= t0
    v[13] ^= t1
    if f:
        v[14] ^= M64

    def g(a, b, c, d, x, y):
        v[a] = (v[a] + v[b] + x) & M64
        v[d] = rotr(v[d] ^ v[a], 32)
        v[c] = (v[c] + v[d]) & M64
        v[b] = rotr(v[b] ^ v[c], 24)
        v[a] = (v[a] + v[b] + y) & M64
        v[d] = rotr(v[d] ^ v[a], 16)
        v[c] = (v[c] + v[d]) & M64
        v[b] = rotr(v[b] ^ v[c], 63)

    for r in range(rounds):
        s = SIGMA[r % 10]
        g(0, 4, 8, 12, m[s[0]], m[s[1]])
        g(1, 5, 9, 13, m[s[2]], m[s[3]])
        g(2, 6, 10, 14, m[s[4]], m[s[5]])
        g(3, 7, 11, 15, m[s[6]], m[s[7]])
        g(0, 5, 10, 15, m[s[8]], m[s[9]])
        g(1, 6, 11, 12, m[s[10]], m[s[11]])
        g(2, 7, 8, 13, m[s[12]], m[s[13]])
        g(3, 4, 9, 14, m[s[14]], m[s[15]])
    return b''.join(struct.pack('<Q', h[i] ^ v[i] ^ v[i + 8]) for i in range(8))


def encode(rounds, h, m, t0, t1, f):
    return (struct.pack('>I', rounds) + struct.pack('<8Q', *h) + struct.pack('<16Q', *m)
            + struct.pack('<2Q', t0, t1) + bytes([f]))


def main():
    fails = 0
    # blake2b("abc"): 12 rounds over the default parameter block.
    h = IV[:]
    h[0] ^= 0x01010040
    m = list(struct.unpack('<16Q', b'abc' + bytes(125)))
    st, out, gas = run_precompile(9, encode(12, h, m, 3, 0, 1), 1000)
    if (st, out, gas) != (0, hashlib.blake2b(b'abc').digest(), 12):
        fails += 1
        print('abc vector failed', st, out.hex(), gas)
    rng = random.Random(152)
    for _ in range(300):
        rounds = rng.choice([0, 1, 2, 9, 10, 11, 12, 20, rng.randrange(0, 200)])
        h = [rng.getrandbits(64) for _ in range(8)]
        m = [rng.getrandbits(64) for _ in range(16)]
        t0, t1, f = rng.getrandbits(64), rng.getrandbits(64), rng.randrange(2)
        st, out, gas = run_precompile(9, encode(rounds, h, m, t0, t1, f), 10 ** 6)
        if (st, out, gas) != (0, model_f(rounds, h, m, t0, t1, f), rounds):
            fails += 1
    # Errors: wrong length, bad final flag, out of gas.
    good = encode(12, IV[:], [0] * 16, 0, 0, 0)
    checks = [
        (run_precompile(9, good[:-1], 1000)[0], 2),
        (run_precompile(9, good + b'\0', 1000)[0], 2),
        (run_precompile(9, good[:-1] + b'\x02', 1000)[0], 2),
        (run_precompile(9, good, 11)[0], 1),
    ]
    fails += sum(1 for got, want in checks if got != want)
    print(f'blake2f: {fails} failures')
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
