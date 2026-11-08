# NS2GameShare

[日本語](README.ja.md)

An experimental Windows client for local Nintendo Switch 2 GameShare.
Play on your PC with video, audio, controllers, keyboard and mouse.
A Switch 2 hosting a compatible game is required.
This is an independent project and is not affiliated with Nintendo.

## Requirements

- Windows x64 with Media Foundation and a compatible GPU driver.
- One of the following wireless bridge configurations:

| Connection setting | Hardware |
| --- | --- |
| ESP32 + FX2LP | M5Stack Basic and FX2LP, connected using the [wiring guide](firmware-basic/README.md) |
| ESP32 (Native USB) | ESP32-C5 with native USB; lower USB bandwidth |

Use the firmware for your specific board. Other ESP32 boards, ordinary USB Wi-Fi adapters and Android are not supported.
You also need your own `prod.keys` and GameShare passphrase; neither is included.
See [passphrase extraction](docs/PASSPHRASE.md) for supported sources.

## Getting started

1. Follow the [build and firmware setup instructions](docs/BUILD.md).
2. Run `client-windows/target/release/NS2GameShare.exe`.
3. Select **English** or **日本語** in the sidebar.
4. In **Connection**, set your participant name, connection method and authentication file paths.
   For **ESP32 + FX2LP**, select the FX2 RAM firmware and enable automatic loading.
5. Start local GameShare on Switch 2, click **Save & connect**, then approve the participant on Switch 2.
6. Focus the game window to play. Use **Disconnect** in settings to end the session.

See [settings and default controls](client-windows/README.md) and [controller and mouse configuration](docs/INPUT.md).

## Known limitations

- Block artifacts and audio dropouts can occur, particularly during fast movement.
- DS4 USB/Bluetooth is supported. XInput and Switch Pro support has limited hardware validation.
- Mouse gyro is experimental and has not been verified in gameplay.
- Requested resolution depends on the host. End-to-end latency has not been measured.
- Host-signature verification is not implemented. Connect only to a trusted host.

## For developers

See the [implementation guide](docs/DEVELOPMENT.md) and [protocol reference](docs/PROTOCOL.md).

## Privacy and license

Keep keys, passphrases and captures private; see [SECURITY.md](SECURITY.md).
Project-authored code is [MIT licensed](LICENSE). Dependencies retain their [own licenses](THIRD_PARTY_NOTICES.md).
