#!/usr/bin/env python3
"""Mutation check: each planted bug in src/evm/u256.vx must fail tests/u256/run.py.

Copies the repository into a temporary directory per mutant, so the real
source is never modified.
"""
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# (name, exact source text, replacement)
MUTANTS = [
    ('ADDMOD drops the 257th bit',
     'if u256_lt(s, a) { hi = u256_one(); }', ''),
    ('SAR without sign fill',
     'if neg && k > 0 { return u256_or(r, shift_by(u256_max(), 256 - k, true)); }', ''),
    ('SDIV ignores operand signs',
     'if an != bn { return u256_neg(q); }', ''),
    ('SUB drops the top borrow',
     'let t3: u128 = base + (a.l3 as u128) - (b.l3 as u128) - c2;',
     'let t3: u128 = base + (a.l3 as u128) - (b.l3 as u128);'),
    ('shift count narrowed before the range check',
     'if !u256_fits_u64(count) || count.l0 >= 256u64 { return 256; }',
     'if count.l0 >= 256u64 { return 256; }'),
    ('SIGNEXTEND off by one byte',
     'let bit = (b.l0 as i32) * 8 + 7;', 'let bit = (b.l0 as i32) * 8 + 15;'),
]


def main():
    src = (ROOT / 'src/evm/u256.vx').read_text()
    survived = []
    for name, old, new in MUTANTS:
        if src.count(old) != 1:
            sys.exit(f'mutant {name!r}: pattern must match exactly once')
        with tempfile.TemporaryDirectory() as tmp:
            copy = Path(tmp) / 'repo'
            shutil.copytree(ROOT, copy, ignore=shutil.ignore_patterns('build', '.git'))
            (copy / 'src/evm/u256.vx').write_text(src.replace(old, new))
            proc = subprocess.run([sys.executable, '-I', str(copy / 'tests/u256/run.py'),
                                   '--smoke', '-O', '2'], capture_output=True, text=True)
        caught = proc.returncode != 0 and 'mismatches' in proc.stdout
        print(f'{"caught  " if caught else "SURVIVED"} {name}')
        if not caught:
            survived.append(name)
    sys.exit(1 if survived else 0)


if __name__ == '__main__':
    main()
