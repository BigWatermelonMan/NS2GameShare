"""Fail closed on public-tree scope violations and detectable secrets.

Diagnostics contain relative filenames and rule names, never matching values.
This is a publication aid, not proof that arbitrary encodings contain no secrets.
"""
import sys
sys.dont_write_bytecode = True
import argparse
import base64
import os
from pathlib import Path
import re
import stat
from public_files import allowed

RULES = {
    'console key assignment': re.compile(rb'(?im)^\s*(?:master_key_[0-9a-f]+|master_kek_[0-9a-f]+|device_key|bis_key_[0-9a-f]+|titlekey)\s*=\s*[0-9a-f]{32,64}\s*$'),
    'private key block': re.compile(rb'-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----'),
    'personal Windows path': re.compile(rb'(?i)[a-z]:[/\\]+Users[/\\]+(?!Public\b|Default\b)[^/\\\s]+'),
    'GitHub token': re.compile(rb'\bgh[pousr]_[A-Za-z0-9]{30,}\b'),
}


def reparse(path):
    info = path.lstat()
    return (stat.S_ISLNK(info.st_mode)
            or bool(getattr(info, 'st_file_attributes', 0) & 0x400))


def local_secrets(root, explicit=()):
    paths = [Path(p) for p in explicit]
    for name in ('NS2GAMESHARE_KEYS', 'NS2GAMESHARE_PASSPHRASE'):
        if os.environ.get(name):
            paths.append(Path(os.environ[name]))
    for directory in ('private', 'secrets', 'captures/reference'):
        folder = root / directory
        if folder.is_dir() and not reparse(folder):
            paths.extend(folder.glob('*.keys'))
            paths.extend(folder.glob('*passphrase*.bin'))
    values = set()
    for path in paths:
        if not path.is_file() or reparse(path):
            raise ValueError('A configured private comparison file is unavailable or a link')
        data = path.read_bytes()
        if path.suffix.lower() == '.keys':
            for name, value in re.findall(rb'(?im)^\s*([a-z0-9_]+)\s*=\s*([0-9a-f]{32,64})\s*$', data):
                if name.endswith(b'_source') or len(set(value.lower())) < 3:
                    continue  # Public derivation constants and dummy test values.
                values.add(bytes.fromhex(value.decode()))
        elif 16 <= len(data) <= 64:
            values.add(data)
        else:
            raise ValueError('A private passphrase comparison file has an invalid size')
    return values


def audit(root, secrets=()):
    errors = []
    count = 0
    if reparse(root):
        return [('root', 'reparse point')], count
    for folder, directories, files in os.walk(root, followlinks=False):
        base = Path(folder)
        for name in list(directories):
            path = base / name
            if name == '.git':
                directories.remove(name)
            elif reparse(path):
                errors.append((path.relative_to(root).as_posix(), 'reparse point'))
                directories.remove(name)
        for name in files:
            path = base / name
            relative = path.relative_to(root).as_posix()
            if reparse(path):
                errors.append((relative, 'reparse point'))
                continue
            count += 1
            if not allowed(relative):
                errors.append((relative, 'outside public allowlist'))
            data = path.read_bytes()
            for rule, pattern in RULES.items():
                if pattern.search(data):
                    errors.append((relative, rule))
            lower = data.lower()
            hex_bytes = re.findall(rb'0x([0-9a-fA-F]{2})\b', data)
            source_bytes = bytes.fromhex(b''.join(hex_bytes).decode())
            for secret in secrets:
                if (secret in data or secret.hex().encode() in lower
                        or base64.b64encode(secret) in data or secret in source_bytes):
                    errors.append((relative, 'local secret match'))
                    break
    return errors, count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('tree', type=Path)
    parser.add_argument('--secret-file', action='append', default=[])
    args = parser.parse_args()
    try:
        if not args.tree.is_dir():
            raise ValueError('Public tree is not a directory')
        secrets = local_secrets(Path(__file__).resolve().parents[2], args.secret_file)
        errors, count = audit(args.tree.absolute(), secrets)
    except (OSError, ValueError) as error:
        print(f'Audit could not complete: {type(error).__name__}', file=sys.stderr)
        return 2
    for name, reason in errors:
        print(f'FAIL {name}: {reason}')
    print(f'Public audit: {count} files; {len(errors)} failures')
    return bool(errors)


if __name__ == '__main__':
    sys.exit(main())
