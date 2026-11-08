"""Create an allowlisted, audited source snapshot under dist/. Never push."""
import sys
sys.dont_write_bytecode = True
import argparse
import hashlib
import json
from pathlib import Path
import shutil
from audit_public import audit, local_secrets, reparse
from public_files import ROOT_FILES, allowed


def export(root, output):
    dist = root / 'dist'
    if output.exists() or output.is_symlink():
        raise ValueError('Output already exists; choose a new directory')
    if not output.resolve().is_relative_to(dist.resolve()) or output.resolve() == dist.resolve():
        raise ValueError('Output must be a new directory beneath workspace dist/')
    for parent in (dist, *output.parents):
        if parent.exists() and reparse(parent):
            raise ValueError('Output ancestry contains a reparse point')
        if parent == root:
            break
    sources = []
    # Only enumerate the small public candidate trees, never private captures.
    candidates = [root / name for name in ROOT_FILES if name != 'PUBLIC_MANIFEST.json']
    for name in ('client-windows/src', 'tools/release', 'licenses/third-party'):
        candidates.extend((root / name).rglob('*'))
    for path in candidates:
        if not path.exists():
            raise ValueError('A required public source file is missing')
        if any(reparse(p) for p in (path, *path.parents) if p.is_relative_to(root)):
            raise ValueError('Source ancestry contains a reparse point')
        if path.is_dir():
            continue
        relative = path.relative_to(root).as_posix()
        if not allowed(relative):
            continue
        sources.append((relative, path))
    secrets = local_secrets(root)
    output.mkdir(parents=True)
    records = []
    for name, source in sorted(set(sources)):
        destination = output / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        # The allowlist contains text only. Keep Git/archive hashes independent
        # of Windows checkout line endings; leave the working sources untouched.
        destination.write_bytes(source.read_bytes().replace(b'\r\n', b'\n'))
        records.append({'path': name, 'sha256': hashlib.sha256(destination.read_bytes()).hexdigest()})
    errors, count = audit(output, secrets)
    if errors:
        for name, reason in errors:
            print(f'FAIL {name}: {reason}')
        raise ValueError('Export rejected by public audit; output is not ready for publication')
    (output / 'PUBLIC_MANIFEST.json').write_text(json.dumps({'files': records}, indent=2) + '\n', encoding='utf-8')
    print(f'Public export ready: {count + 1} files at {output.relative_to(root)}')


def main():
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('dist/github'))
    args = parser.parse_args()
    try:
        export(root, (root / args.output).absolute())
    except (OSError, ValueError) as error:
        print(f'Export stopped: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
