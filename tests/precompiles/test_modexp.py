#!/usr/bin/env python3
"""MODEXP precompile (0x05) against Python pow() and revm's gas rules, for
Byzantium (4), Berlin (7) and Osaka (13).

Build first: scripts/build-testlib.sh 2
"""
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from pclib import PC_ERROR, PC_OK, PC_OOG, run_precompile  # noqa: E402

BYZANTIUM, BERLIN, OSAKA = 4, 7, 13
U64 = (1 << 64) - 1


def word(b, off):
    return int.from_bytes(b[off:off + 32].ljust(32, b'\0'), 'big')


def iterations(mult, el, highp):
    bits = highp.bit_length()
    if el <= 32:
        it = bits - 1 if bits else 0
    else:
        it = min(U64, mult * (el - 32)) + max(1, bits) - 1
        it = min(U64, it)
    return max(it, 1)


def complexity(spec, x):
    if spec >= OSAKA:
        return 16 if x <= 32 else 2 * ((x + 7) // 8) ** 2
    if spec >= BERLIN:
        return ((x + 7) // 8) ** 2
    if x <= 64:
        return x * x
    if x <= 1024:
        return x * x // 4 + 96 * x - 3072
    return x * x // 16 + 480 * x - 199680


def expected(spec, data, gas_limit):
    """(status, output, gas) per revm modexp.rs."""
    min_gas, mult, div = 0, 8, 20
    if spec >= BERLIN:
        min_gas, div = 200, 3
    if spec >= OSAKA:
        min_gas, mult, div = 500, 16, 1
    if min_gas > gas_limit:
        return PC_OOG, b'', gas_limit
    bl, el, ml = word(data, 0), min(word(data, 32), U64), word(data, 64)
    if bl > U64 or ml > U64:
        return PC_ERROR, b'', gas_limit
    if spec >= OSAKA and (bl > 1024 or ml > 1024 or el > 1024):
        return PC_ERROR, b'', gas_limit
    body = data[96:]
    hn = min(el, 32)
    highp = int.from_bytes(body[bl:bl + hn].ljust(hn, b'\0'), 'big') if hn else 0
    gas = min(U64, complexity(spec, max(bl, ml)) * iterations(mult, el, highp) // div)
    gas = max(gas, min_gas)
    if gas > gas_limit:
        return PC_OOG, b'', gas_limit
    if bl == 0 and ml == 0:
        return PC_OK, b'', gas
    pad = body.ljust(bl + el + ml, b'\0')
    b = int.from_bytes(pad[:bl], 'big')
    e = int.from_bytes(pad[bl:bl + el], 'big')
    m = int.from_bytes(pad[bl + el:bl + el + ml], 'big')
    r = 0 if m == 0 else pow(b, e, m)
    return PC_OK, r.to_bytes(ml, 'big'), gas


def encode(b, e, m, bl=None, el=None, ml=None, trunc=None):
    bl = len(b) if bl is None else bl
    el = len(e) if el is None else el
    ml = len(m) if ml is None else ml
    d = bl.to_bytes(32, 'big') + el.to_bytes(32, 'big') + ml.to_bytes(32, 'big') + b + e + m
    return d[:trunc] if trunc is not None else d


def cases(rng):
    out = []
    p = 2 ** 256 - 2 ** 32 - 977
    out.append(encode(b'\x03', (p - 2).to_bytes(32, 'big'), p.to_bytes(32, 'big')))
    out.append(encode(b'', b'', b''))
    out.append(encode(b'\x05', b'\x01', b''))
    out.append(encode(b'\x05', b'\x00', b'\x00\x00'))
    out.append(encode(b'\x05', b'\x02', b'\x01'))
    out.append(encode(b'\x05', b'\x02', b'\x07', trunc=97))
    out.append(encode(b'\x05' * 40, b'\xff' * 40, b'\x09' * 40))
    out.append(bytes(10))
    for _ in range(250):
        bl, el, ml = (rng.choice([0, 1, 7, 8, 9, 31, 32, 33, 64, 65, 100]) for _ in range(3))
        b = bytes(rng.getrandbits(8) for _ in range(bl))
        e = bytes(rng.getrandbits(8) for _ in range(min(el, 40)))
        m = bytes(rng.getrandbits(8) for _ in range(ml))
        out.append(encode(b, e, m, el=len(e)))
    for size in (256, 512, 1024, 1025):
        out.append(encode(b'\x07' * size, b'\x03', b'\xfd' * size))
    out.append(encode(b'\x02', b'\x03', b'\x05', bl=2 ** 64))
    out.append(encode(b'\x02', b'\x03', b'\x05', el=2 ** 70))
    return out


def main():
    rng = random.Random(198)
    fails = 0
    total = 0
    for spec in (BYZANTIUM, BERLIN, OSAKA):
        for data in cases(rng):
            for gas_limit in (10 ** 9, 600, 150):
                want = expected(spec, data, gas_limit)
                got = run_precompile(5, data, gas_limit, spec=spec)
                total += 1
                if got != want:
                    fails += 1
                    if fails <= 5:
                        print(f'spec {spec} gas {gas_limit} input {data[:100].hex()}...\n'
                              f'  got  {got[0]} {got[2]} {got[1][:40].hex()}\n'
                              f'  want {want[0]} {want[2]} {want[1][:40].hex()}')
    print(f'modexp: {total} cases, {fails} failures')
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
