# Release checklist

For maintainers preparing source or binary releases.

## Source checks

Run from a clean source checkout:

```powershell
python -B tools/release/audit_public.py .
python -B -m unittest discover -s tools/release -p "test_*.py"
cargo fmt --manifest-path client-windows/Cargo.toml -- --check
cargo test --locked --manifest-path client-windows/Cargo.toml
cargo clippy --locked --manifest-path client-windows/Cargo.toml --all-targets -- -D warnings
```

The audit checks the public file allowlist and detectable secrets. Review the release contents manually as well.
Optional `NS2GAMESHARE_KEYS` and `NS2GAMESHARE_PASSPHRASE` file paths enable comparison against local secret values.
Never put those values in CI or its logs. CI checks need no authentication files or hardware.

## Packaging

- Follow [BUILD.md](BUILD.md) to build the client and firmware.
- Check both UI languages, connection/disconnection, video, audio and input on supported hardware.
- Document known limitations and distinguish tested hardware from experimental support.
- Include no keys, settings, captures, console software, SDKs or local build caches.
- Regenerate the license inventory after dependency changes with `python -B tools/release/generate_notices.py`.
- Include applicable license/copyright notices with binaries, including the linked ESP-IDF components for firmware.
  See [third-party notices](../THIRD_PARTY_NOTICES.md).
