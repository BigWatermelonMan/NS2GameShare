# Build from source

## Windows client

Windows x64, MSVC build tools and Rust 1.89 or newer are required (tested with Rust 1.96).
Media Foundation and an OpenGL-capable GPU/driver are required. Windows N editions may need Microsoft's Media Feature Pack.
Install Rust with the MSVC target, then run from the repository root:

```powershell
cargo build --release --locked --manifest-path client-windows/Cargo.toml
cargo test --locked --manifest-path client-windows/Cargo.toml
cargo clippy --locked --manifest-path client-windows/Cargo.toml --all-targets -- -D warnings
```

The executable is `client-windows/target/release/NS2GameShare.exe`.
Rust dependencies are pinned in Cargo.lock. No console keys are needed to build or run unit tests.

## ESP-IDF firmware

Install **ESP-IDF v5.5.6** and its target toolchains using Espressif's installer.
Set `IDF_PATH` to your installation. The build script uses that installation's `export.ps1`.

M5Stack Basic (GUI: ESP32 + FX2LP; 16 MB flash):

```powershell
.\firmware-basic\build.ps1
.\firmware-basic\build.ps1 -Flash -Port COMx
```

Use the actual USB-UART port, not the literal `COMx`. Flashing overwrites the application;
back up the board's existing firmware before flashing. The script uses 115200 baud.
After wiring, live playback uses FX2 and does not open the Basic USB-UART port.

ESP32-C5 (GUI: ESP32 (Native USB); run in an ESP-IDF environment):

```powershell
idf.py -C firmware set-target esp32c5
idf.py -C firmware build
idf.py -C firmware -p COMx -b 115200 flash
```

Use the board's **native USB** for playback. USB-UART is not supported for live streaming.
The firmware depends on ESP-IDF private Wi-Fi interfaces: other versions are not guaranteed compatible.

## FX2LP firmware

Install SDCC with the `mcs51` target and make `sdcc` available in PATH:

```powershell
.\firmware-fx2\build.ps1
```

Output: `firmware-fx2/build/basic-fx2.ihx`.
Bind WinUSB to **only the FX2LP 04B4:8613 device** using Zadig. Do not replace unrelated device drivers.
Load using the GUI or `NS2GameShare.exe fx2-load firmware-fx2/build/basic-fx2.ihx`.
Loading is RAM-only; unplugging loses the image. EEPROM is not written.
See [hardware wiring](../firmware-basic/README.md).
