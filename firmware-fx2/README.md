# FX2LP RAM USB bridge

Build with SDCC: `./build.ps1` from PowerShell. See [BUILD.md](../docs/BUILD.md).
The firmware uses the parallel FIFO on EP6 IN and UART return traffic from EP2 OUT.
It enumerates under 04B4:8613 and exposes WinUSB-compatible descriptors.
The Windows loader writes RAM only. No EEPROM programming is performed.

Use with the documented [M5Stack Basic wiring](../firmware-basic/README.md).
Changing pins, polarity or FIFO selection can stop communication; verify with `fx2-status`
and `link-bench` before starting GameShare. This is not a general FX2 logic-analyzer firmware.
