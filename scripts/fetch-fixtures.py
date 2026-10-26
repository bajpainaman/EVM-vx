#!/usr/bin/env python3
"""Downloads the Ethereum test fixtures that revm's own test script uses.

Versions match revm v120 scripts/run-tests.sh. Destination defaults to
/data/hdd/evm-fixtures; pass another directory as the first argument.
"""
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

DEST = Path(sys.argv[1] if len(sys.argv) > 1 else '/data/hdd/evm-fixtures')
EEST = 'https://github.com/ethereum/execution-spec-tests/releases/download/v5.4.0'
DEVNET = ('https://github.com/ethereum/execution-specs/releases/download/'
          'tests-glamsterdam-devnet%40v8.1.0/fixtures_glamsterdam-devnet.tar.gz')
ARCHIVES = [
    (f'{EEST}/fixtures_develop.tar.gz', 'main/develop'),
    (f'{EEST}/fixtures_stable.tar.gz', 'main/stable'),
    (DEVNET, 'devnet'),
]


def fetch(url, sub):
    out = DEST / sub
    if (out / 'fixtures').exists():
        print(f'have {sub}')
        return
    out.mkdir(parents=True, exist_ok=True)
    print(f'fetching {url}', flush=True)
    with urllib.request.urlopen(url) as resp:
        with tarfile.open(fileobj=resp, mode='r|gz') as tar:
            tar.extractall(out, filter='data')
    print(f'extracted {sub}', flush=True)


def main():
    DEST.mkdir(parents=True, exist_ok=True)
    for url, sub in ARCHIVES:
        fetch(url, sub)
    legacy = DEST / 'legacytests'
    if not legacy.exists():
        subprocess.run(['git', 'clone', '-q', '--depth', '1',
                        'https://github.com/ethereum/legacytests.git', str(legacy)], check=True)
    print('DONE')


if __name__ == '__main__':
    main()
