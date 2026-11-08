"""Collect pinned Cargo dependency license declarations and notice texts."""
import json
from pathlib import Path
import subprocess


def main():
    root = Path(__file__).resolve().parents[2]
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--manifest-path', str(root / 'client-windows/Cargo.toml'),
        '--locked', '--offline', '--format-version', '1',
    ], cwd=root))
    destination = root / 'licenses/third-party'
    destination.mkdir(parents=True, exist_ok=True)
    records = []
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        if package['source'] is None:
            continue
        source = Path(package['manifest_path']).parent
        candidates = []
        for path in source.rglob('*'):
            if not path.is_file() or path.is_symlink():
                continue
            upper = path.name.upper()
            if upper.startswith(('LICENSE', 'LICENCE', 'COPYING', 'COPYRIGHT', 'NOTICE')) or upper == 'OFL.TXT':
                candidates.append(path)
            elif package['name'] == 'epaint_default_fonts' and path.suffix.lower() in ('.txt', '.md'):
                candidates.append(path)
        notices = []
        for index, path in enumerate(sorted(candidates)):
            name = f"{package['name']}-{package['version']}-{index}.txt"
            text = path.read_text(encoding='utf-8', errors='replace')
            (destination / name).write_text(text, encoding='utf-8')
            notices.append('licenses/third-party/' + name)
        records.append({'name': package['name'], 'version': package['version'],
                        'license': package['license'], 'repository': package.get('repository'),
                        'notices': notices})
    (root / 'docs/THIRD_PARTY.json').write_text(json.dumps(records, indent=2) + '\n', encoding='utf-8')
    (root / 'THIRD_PARTY_NOTICES.md').write_text('''# Third-party notices

The root MIT license covers project-authored code only.
Pinned Cargo packages and their declared license expressions are listed in
[THIRD_PARTY.json](docs/THIRD_PARTY.json). Their license and copyright texts are in
`licenses/third-party/`; retain them when redistributing binaries.
The inventory includes target-specific/transitive packages, some of which are not linked on Windows.

egui/eframe includes default font data under OFL-1.1 and Ubuntu-font-1.0 in addition
to its MIT/Apache options. Their texts are included in the collected notices.
Windows Japanese fonts are loaded from the user's Windows installation at runtime;
they are not copied into the repository or distributed binary.
Other dependencies include Unicode-3.0, BSD, Zlib, ISC, BSL-1.0 and MPL-2.0 terms.
Packages offering MIT as an alternative do not make GPL/LGPL mandatory.
Dependency source remains available through the Cargo.lock package versions and upstream repositories.

Firmware builds depend on Espressif ESP-IDF v5.5.6 and its separately licensed
components/Wi-Fi libraries. No ESP-IDF SDK, console SDK, console binary or vendor
firmware blob is copied into this source export. Firmware binary distribution
requires preserving the notices for the exact ESP-IDF components linked into it.
The SDCC toolchain is a build dependency, not included in this export.

Protocol documentation consulted includes Nintendo Switch Brew's LDN service
description (https://switchbrew.org/wiki/LDN_services). DS4 report structures were
cross-checked against Linux hid-playstation. Neither page nor driver source is
vendored here; original upstream code retains its license.
Nintendo, Switch and GameShare names belong to their respective owners.


The virtual mouse IMU model is adapted from [mizuyoukanao/nxic-pico](https://github.com/mizuyoukanao/nxic-pico),
MIT, Copyright (c) 2026 mizuyoukanao. The original notice is retained in
`licenses/third-party/nxic-pico-MIT.txt`.

Regenerate the inventory after dependency changes using
`python tools/release/generate_notices.py`.
''', encoding='utf-8')
    print(f'Collected license inventory for {len(records)} Cargo packages')


if __name__ == '__main__':
    main()
