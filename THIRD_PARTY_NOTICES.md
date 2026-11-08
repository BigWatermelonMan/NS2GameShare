# Third-party notices

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

Regenerate the inventory after dependency changes using
`python tools/release/generate_notices.py`.

The virtual mouse IMU model is adapted from [mizuyoukanao/nxic-pico](https://github.com/mizuyoukanao/nxic-pico),
MIT, Copyright (c) 2026 mizuyoukanao. The original notice is retained in
`licenses/third-party/nxic-pico-MIT.txt`.
