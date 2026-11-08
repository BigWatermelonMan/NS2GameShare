# Windows client

Follow [BUILD.md](../docs/BUILD.md), then run `target/release/NS2GameShare.exe`.
Choose **English** / **日本語** in the sidebar.

## Settings

- **Connection**: participant name (up to 32 UTF-8 bytes), connection method, authentication files and FX2 firmware loading.
- **Video & audio**: resolution, receive capacity, audio, A/V synchronization and noise reduction; UI theme and scale.
- **Keyboard**: click a binding and press a key. Delete unassigns; Esc cancels. A WASD preset is available.
- **Controllers**: choose DirectInput or XInput, select a device/slot, and configure buttons and sticks. DS4 uses DirectInput settings.
- **Mouse**: capture key, right-stick or experimental gyro output, sensitivity and button mappings.
- **Input test**: check input without connecting to a Switch 2. Restart the test after changing settings.

**Save & connect** saves settings and starts a session. **Disconnect** ends it.
Ctrl+S saves settings. Input and streaming changes apply on the next connection; language and theme change immediately.
See [controller mapping and mouse gyro](../docs/INPUT.md) and [passphrase extraction](../docs/PASSPHRASE.md).

## Default controls

| PC control | Nintendo input |
| --- | --- |
| X / Z / S / A | A / B / X / Y |
| Arrow keys | D-pad and left stick |
| Q / W / 1 / 2 | L / R / ZL / ZR |
| Enter / Backspace | Plus / Minus |
| Mouse movement | Right stick while captured |
| F8 | Capture/release mouse |
| F7 | Reset attitude in experimental gyro mode |
| F9 | Toggle GPU noise reduction |
| Esc | Exit playback |

Focus the game window to play. Losing focus releases input and mouse capture.
DS4 Circle maps to Nintendo A, Cross to B. XInput uses physical-position mapping.

## Settings files

Settings are saved in `%LOCALAPPDATA%/NS2GameShare/settings.json`.
The input configuration is `settings.input.json`; session logs are under `logs/`.
These files can contain private paths or network information. Redact them before sharing.
Keys stay in the selected files; settings store only their paths.

## Command line

From the repository root:

```powershell
.\client-windows\run.ps1 -Gui
.\client-windows\run.ps1 -Port fx2 -Keys C:\private\prod.keys -PassphraseFile C:\private\gameshare-passphrase.bin
```

The wrapper also accepts `NS2GAMESHARE_KEYS` and `NS2GAMESHARE_PASSPHRASE` environment variables.
Run `NS2GameShare.exe --help` for additional commands.
