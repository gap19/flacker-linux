#!/usr/bin/env python3
"""Collect dependency notices from Cargo's resolved local source tree."""
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--format-version', '1'], cwd=root))
output = root / 'LICENSES'
output.mkdir(exist_ok=True)
index = ['# Third-party dependency notices', '', 'Generated from Cargo.lock. Crate sources in source releases retain their original licenses.', '']
for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
    if package['name'] == 'flacker':
        continue
    base = Path(package['manifest_path']).parent
    destination = output / (package['name'] + '-' + package['version'])
    destination.mkdir(exist_ok=True)
    index.append(f"- **{package['name']} {package['version']}** — {package.get('license') or 'See crate license files'}")
    candidates = set()
    for pattern in ['LICENSE*', 'LICENCE*', 'COPYING*', 'NOTICE*']:
        candidates.update(base.glob(pattern))
    if package.get('license_file'):
        candidates.add(base / package['license_file'])
    for path in candidates:
        if path.is_file() and path.stat().st_size < 1024 * 1024:
            (destination / path.name).write_bytes(path.read_bytes())
(output / 'README.md').write_text('\n'.join(index) + '\n')
print(f'Notices for {len(metadata["packages"]) - 1} resolved dependencies in LICENSES/')
