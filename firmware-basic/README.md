# ESP32 + FX2LP wiring

This configuration uses M5Stack Basic's 2.4 GHz radio and an 8-bit parallel connection to an FX2LP USB bridge.
Other ESP32 boards require board-specific pin and firmware changes.
The FX2LP pin labels below refer to the blue HW-27-style board; verify the labels on your board.

**Disconnect both USB cables and switch Basic fully off before wiring.**
Power each board from its own USB port and connect a common GND.
Use 3.3 V logic only. Verify FX2LP VCC is 3.3 V; never connect Basic's 5 V to signal pins.

## Between Basic and FX2LP

| Basic external pin | FX2LP label | Signal |
| --- | --- | --- |
| G16 | PB0 | FD0 |
| G17 | PB1 | FD1 |
| G18 | PB2 | FD2 |
| G19 | PB3 | FD3 |
| G23 | PB4 | FD4 |
| G26 | PB5 | FD5 |
| G2 | PB6 | FD6 |
| G1 (TX / TXD0) | PB7 | FD7 |
| G5 | RDY1 | SLWR |
| G35 | CTL0 | FIFO full flag: Low = full, High = space available |
| G36 | PA0 | Return UART input to Basic |
| GND | GND | Common ground |

Add a **15 kΩ pull-down** from Basic G35 to GND and a **15 kΩ pull-up** from G36 to Basic 3.3 V.

## Connections on FX2LP

| FX2LP label | Connect to | Purpose |
| --- | --- | --- |
| PA4 | GND | FIFOADR0 = 0 |
| PA5 | FX2LP VCC (3.3 V) | FIFOADR1 = 1; select EP6 |
| PA2 | FX2LP VCC (3.3 V) | Disable SLOE; data bus stays input |
| RDY0 | FX2LP VCC (3.3 V) | Disable SLRD |
| PA6 | FX2LP VCC (3.3 V) | Disable PKTEND |
| PA7 | GND | Select SLCS# |

- **SLWR is RDY1**, not CTL1. CTL0 is the full flag.
- G1 is used for data. Do not open Basic's USB-UART port during playback.
- G2 is a boot strap. Keep SLOE high so FX2LP does not drive the data bus.
- Leave IFCLK, CLK, CTL1/2, PD0–7 and SCL/SDA unconnected.
- Do not substitute G15 or G3 for data pins. LCD and SD functions are disabled by this firmware.

## Setup and link check

1. [Build and flash](../docs/BUILD.md) the Basic firmware, and build the FX2LP firmware.
2. Install WinUSB on the FX2LP **04B4:8613** device only.
3. Load the FX2LP RAM image and check the link from the repository root:

```powershell
.\client-windows\target\release\NS2GameShare.exe fx2-load firmware-fx2/build/basic-fx2.ihx
.\client-windows\target\release\NS2GameShare.exe fx2-status
.\client-windows\target\release\NS2GameShare.exe link-bench --seconds 5
```

Check that USB High Speed and endpoints 02/86 are present and the link test reports no transfer errors.
In the GUI, select **ESP32 + FX2LP**, the authentication files and the FX2 RAM image, then enable automatic loading.
The RAM image must be reloaded after unplugging FX2LP; EEPROM is not changed.
Basic's USB-UART connection is used for power/flashing; live streaming uses FX2LP.
