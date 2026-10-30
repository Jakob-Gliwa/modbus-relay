#!/usr/bin/env python3
"""Cross-build frozen variants with one Rust toolchain and the bundled LLVM linker.

Requires: rustup target add aarch64-unknown-linux-musl (or the selected musl target).
All artifacts stay under --root. No installed runtime libraries are needed by the
resulting binaries. Dependencies must already be downloaded for --offline builds.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--root', type=Path, default=Path('target/optimization'))
    p.add_argument('--target', default='aarch64-unknown-linux-musl')
    p.add_argument('--variant', action='append', help='case[:output-label:opt-level:lto]')
    p.add_argument('--examples', action='store_true')
    args = p.parse_args()
    root = args.root.resolve()
    specs = args.variant or ['baseline', 'targets', 'ftlog', 'sockets', 'channels', 'combined']
    rustc = subprocess.check_output(['rustc', '-vV'], text=True)
    host = next(line.split(': ', 1)[1] for line in rustc.splitlines() if line.startswith('host: '))
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    linker = sysroot / 'lib' / 'rustlib' / host / 'bin' / 'rust-lld'
    if not linker.is_file():
        p.error(f'Rust LLVM linker missing: {linker}')
    out = root / 'linux-bin'
    out.mkdir(exist_ok=True)
    metadata_path = out / 'builds.json'
    metadata = json.loads(metadata_path.read_text()) if metadata_path.exists() else {'rustc': rustc, 'builds': []}
    for spec in specs:
        parts = spec.split(':')
        if len(parts) == 1:
            case, label, level, lto = parts[0], parts[0], '3', 'thin'
        elif len(parts) == 4:
            case, label, level, lto = parts
        else:
            p.error(f'invalid variant: {spec}')
        env = os.environ.copy()
        env['CARGO_TARGET_' + args.target.upper().replace('-', '_') + '_LINKER'] = str(linker)
        env['RUSTFLAGS'] = '-C linker-flavor=ld.lld'
        env['CARGO_PROFILE_RELEASE_OPT_LEVEL'] = level
        env['CARGO_PROFILE_RELEASE_LTO'] = lto
        cmd = ['cargo', 'build', '--locked', '--offline', '--release', '--target', args.target,
               '--manifest-path', str(root / case / 'Cargo.toml'), '--target-dir', str(root / 'cross'), '--bins']
        if args.examples:
            cmd.append('--examples')
        start = time.monotonic()
        # All snapshots share package name/version. Cargo can otherwise reuse a
        # fresh root artifact from another path when copied sources have older
        # mtimes. Keep dependency caching, but force each root package to rebuild.
        subprocess.run(['cargo', 'clean', '--offline', '--release', '--target', args.target,
                        '--manifest-path', str(root / case / 'Cargo.toml'),
                        '--target-dir', str(root / 'cross'), '--package', 'modbus-relay'],
                       check=True, env=env)
        subprocess.run(cmd, check=True, env=env)
        source = root / 'cross' / args.target / 'release'
        dest = out / label
        shutil.copy2(source / 'modbus-relay', dest)
        if args.examples:
            for name in ['benchmark', 'logging-bench']:
                if (root / case / 'examples' / (name + '.rs')).exists():
                    shutil.copy2(source / 'examples' / name, out / (label + '-' + name))
        record = {'variant': label, 'source': case, 'target': args.target, 'opt_level': level, 'lto': lto,
                  'build_seconds': time.monotonic() - start, 'binary_bytes': dest.stat().st_size,
                  'binary_sha256': hashlib.sha256(dest.read_bytes()).hexdigest()}
        metadata['builds'] = [b for b in metadata['builds'] if b['variant'] != label] + [record]
        metadata_path.write_text(json.dumps(metadata, indent=2) + '\n')
        print(json.dumps(record), flush=True)


if __name__ == '__main__':
    main()
