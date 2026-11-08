# Experimental Linux Wi-Fi bridge

This backend replaces the ESP32 and its USB link with a Linux Wi-Fi station and
an SSH tunnel. The Windows client still handles LDN authentication, media and
input. Association, static CCMP, and streaming with a Switch 2 still require
hardware validation.

Use a dedicated Linux Wi-Fi PHY with a driver that supports monitor mode,
managed mode, setting a station MAC, and installing static CCMP keys through
nl80211. A compatible USB Wi-Fi adapter can provide that PHY. Linux on bare
metal is the initial target. Stock Android's public Wi-Fi Direct API and USB
tethering do not provide the raw discovery and key installation used here.

## Set up the radio

On a trusted Linux machine with Python 3.12 or later, git, iw, and an SSH server,
install the bridge dependencies in a virtual environment outside the checkout.

```sh
python3 -m venv ~/nsgs-bridge-venv
~/nsgs-bridge-venv/bin/pip install trio 'ldn @ git+https://github.com/kinnay/LDN.git@13edfafa1f7e3d2eda5ce1f5215491e144f3661c'
iw dev
sudo ~/nsgs-bridge-venv/bin/python bridge-linux/bridge.py --phy phy1
```

Substitute the PHY reported by `iw dev`. Keep other interfaces on that PHY
disconnected and unmanaged so they cannot change the channel during discovery
or claim the temporary station. Use a separate interface for SSH. The bridge
requires privileges to create wireless interfaces and open raw sockets. It
listens on IPv4 loopback only and accepts one client. Run it on a machine where
you trust local users with access to that port.

### Check a laptop's built-in Wi-Fi

The PHY can be the built-in adapter. With Linux booted directly on the laptop,
start a local GameShare session on Switch 2 and run the discovery probe before
starting the bridge server.

```sh
sudo ~/nsgs-bridge-venv/bin/python bridge-linux/bridge.py --phy phy0 --probe
```

Use the PHY reported by `iw dev`. This check needs only the laptop and the
Switch 2. It listens for 20 seconds without joining the session or using keys.
Receiving an advertisement confirms discovery on that adapter. To test the
next stages on the same laptop, start the server and run the Rust client's
`connect` command under Linux using its loopback port.

```sh
cargo run --release --locked --manifest-path client-windows/Cargo.toml -- connect \
  --port tcp:32990 --keys /path/to/prod.keys \
  --passphrase-file /path/to/gameshare-passphrase.bin --request-session
```

This command tests association, authentication and media packet receipt. The
playable `play` frontend currently requires Windows.

From Windows, open the tunnel and leave it running.

```powershell
ssh -N -o ExitOnForwardFailure=yes -L 127.0.0.1:32990:127.0.0.1:32990 user@linux-radio
```

## Run the Windows client

Build the Windows client as described in [Build](../docs/BUILD.md). Start local
GameShare on the Switch 2, then run this from the checkout.

```powershell
.\client-windows\target\release\NS2GameShare.exe play --port tcp:32990 --keys <path-to-keys> --passphrase-file <path-to-passphrase>
```

Replace the two path placeholders with quoted paths to your own files and
approve the participant on the Switch 2. The Windows client keeps those files
local and sends the derived session key through the tunnel. This bridge is
currently available through the command line.

During the first hardware test, check discovery, the `sta linked` event,
successful LDN authentication, IPv4 assignment, and then video, audio, and
input. Discovery alone only establishes that the driver can receive
advertisements. If the bridge reports a radio failure, check whether the driver
supports the required operations and whether another process controls the PHY.
An interrupted privileged process may leave `nsgsmon` or `nsgssta` behind.
Check `iw dev` before restarting and remove any leftover bridge interface with
`sudo iw dev <interface> del`.

The bridge uses [kinnay/LDN](https://github.com/kinnay/LDN) for Linux station
and monitor operations and is distributed separately from the MIT client
under GPL-3.0-only. Its pinned dependency uses private station helpers, so
revalidate them before updating the pin.
