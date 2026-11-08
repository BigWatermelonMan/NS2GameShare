import tempfile
import unittest
from pathlib import Path
from audit_public import audit, local_secrets
from export_public import export
from public_files import allowed


class PublicationTests(unittest.TestCase):
    def test_private_and_unfinished_paths_are_denied(self):
        for path in ('prod.keys', 'captures/play.pcap', 'client-android/src/Main.kt',
                     'client-windows/target/release/NS2GameShare.exe',
                     'firmware/sdkconfig', 'docs/internal/notes.md', '../README.md',
                     'docs/gameshare-analysis.md', 'docs/windows-client-progress-20261106.md'):
            self.assertFalse(allowed(path), path)
        self.assertTrue(allowed('client-windows/src/settings/i18n.rs'))
        self.assertTrue(allowed('docs/PROTOCOL.md'))
        self.assertTrue(allowed('docs/DEVELOPMENT.md'))
        self.assertTrue(allowed('docs/PROTOCOL.ja.md'))
        self.assertTrue(allowed('docs/DEVELOPMENT.ja.md'))

    def test_detects_secret_bytes_hex_and_key_assignments_without_echoing(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            secret = bytes(range(16, 32))
            path = root / 'README.md'
            for payload in (secret, secret.hex().encode(),
                            b'master_' + b'key_12 = ' + b'42' * 16):
                path.write_bytes(payload)
                errors, _ = audit(root, {secret})
                self.assertTrue(errors)
                self.assertNotIn(secret.hex(), repr(errors))
            path.write_text('No private data', encoding='utf-8')
            self.assertEqual(audit(root, {secret})[0], [])

    def test_private_file_comparison_and_missing_explicit_file_fail_closed(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / 'private').mkdir()
            value = bytes(range(32, 48))
            (root / 'private/own.keys').write_text('master_' + 'key_12 = ' + value.hex())
            self.assertIn(value, local_secrets(root))
            with self.assertRaises(ValueError):
                local_secrets(root, [root / 'missing.keys'])

    def test_export_cannot_overwrite_or_escape_dist(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / 'dist/exists').mkdir(parents=True)
            (root / 'dist/exists/keep.txt').write_text('keep')
            for output in (root / 'dist/exists', root / 'elsewhere', root / 'dist'):
                with self.assertRaises(ValueError):
                    export(root, output)
            self.assertEqual((root / 'dist/exists/keep.txt').read_text(), 'keep')


if __name__ == '__main__':
    unittest.main()
