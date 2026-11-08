# Protocol reference

[日本語](PROTOCOL.ja.md)

Developer reference for the formats implemented by NS2GameShare. Start with the
[implementation guide](DEVELOPMENT.md) for component boundaries and testing.

This is an implementation-derived, incomplete interoperability reference, not an
official GameShare specification. **Implemented** describes the current code;
**observed** describes layouts represented by regression fixtures; **experimental**
or **inferred** marks behavior that still needs gameplay or broader compatibility testing.
An observed constant is not evidence that other versions use the same value.
Offsets below are zero-based byte offsets. BE/LE mean big/little endian.
Unlisted bytes are zero in the current writer, without assigning them a protocol meaning.

## 1. Layers and session flow

```text
Windows client                                    ESP32                 Switch 2
  discovery/authentication/IPv4/TCP/UDP <-- USB --> Wi-Fi station <-- radio --> host
  LRP control, LSP media, input feedback
```

The bridge carries raw monitor frames and Ethernet frames. IPv4/TCP/UDP run in
the Windows process using smoltcp, rather than through a Windows network adapter.
The LDN application authentication exchange is separate from Wi-Fi association.

Current connection flow, implemented in [connect.rs](../client-windows/src/connect.rs):

1. Scan permitted channels and authenticate/decrypt a GameShare advertisement.
2. Choose a fresh station MAC, derive the session temporal key, configure the bridge,
   and join the advertised BSSID/channel.
3. Exchange the proprietary Ethernet authentication request/response and verify its GCM tag.
4. Wait for a subsequent advertisement to list the client's assigned IPv4 address.
   The host address also comes from that list; this path does not use DHCP.
5. Open TCP to the advertised LRP control port and request a session.
   User approval on the host is a separate part of session admission.
6. Read host feedback/time-sync ports, send controller information, and receive video-format notices.
7. Synchronize clocks, send input over UDP, reassemble LSP media, and send LRP stream feedback.

Local feedback/media sockets currently use ports 51000/51001; the time-sync socket
uses 51002. These are client choices, not universal host ports.

## 2. PC–bridge transport

Sources: [wire.rs](../client-windows/src/wire.rs), [radio.rs](../client-windows/src/radio.rs),
[shared firmware](../firmware/main/main.c).

PC commands are ASCII lines terminated by LF. Ethernet transmissions are binary
frames interleaved with those lines. Device replies, events and packets all use
the binary framing below; do not parse an entire USB read as one message.

| Offset | Size | Encoding | Meaning |
| --- | --- | --- | --- |
| 0 | 2 | bytes | Sync `A5 5A` |
| 2 | 1 | u8 | Type |
| 3 | 2 | LE u16 | Payload length `N` |
| 5 | N | bytes | Payload |
| 5 + N | 2 | LE u16 | CRC over type, length and payload |

CRC is CRC-16/CCITT-FALSE: polynomial `0x1021`, initial `0xFFFF`, no reflection,
no final XOR. The synthetic check string `123456789` produces `0x29B1`.
The Rust framing limit is 4200 payload bytes. Bad types, lengths and CRCs trigger
resynchronization instead of trusting an unbounded length.

| Type | Direction | Payload |
| --- | --- | --- |
| 1 / PKT | Device → PC | 24-byte capture header followed by an 802.11 frame without FCS |
| 2 / TEXT | Device → PC | Command response text |
| 3 / H2D | PC → device | 18-byte data header followed by an Ethernet frame |
| 4 / D2H | Device → PC | 18-byte data header followed by an Ethernet frame |
| 5 / EVENT | Device → PC | Asynchronous status text |

### Ethernet data header

| Offset | Size | Encoding | Meaning |
| --- | --- | --- | --- |
| 0 | 8 | LE u64 | Device timestamp in microseconds on RX; current PC writer sends zero |
| 8 | 2 | LE u16 | Ethernet frame length |
| 10 | 1 | u8 | RX channel |
| 11 | 1 | i8 | RX RSSI, if available |
| 12 | 1 | bits | Metadata flags |
| 13 | 1 | u8 | Current value zero |
| 14 | 4 | LE u32 | Direction-specific sequence counter |

The header is **18 bytes**, not 16. The Rust Ethernet writer accepts 14–4078-byte
frames; its D2H reader requires 32–4096-byte payloads and an exact declared-length match.
The native Wi-Fi Ethernet callback sets broadcast bit 1 but has no RSSI, key-ID or
retry metadata. Do not infer decryption status from a missing flag. Firmware also
has an experimental H2D bit-4 raw-802.11 path; ordinary client traffic uses Ethernet.

The capture header is 24 bytes: timestamp LE u64 at 0, length LE u16 at 8,
channel at 10, secondary-channel metadata at 11, RSSI/noise at 12/13, PHY/rate at
14/15, SIG-A1 LE u32 at 16, SIG-A2 LE u16 at 20, packet type at 22, zero at 23.
Treat PHY metadata according to the firmware target, not as a portable radio ABI.

### Commands used by the client

`hello`, `stop`, `mode monitor`, `ch`, `hop`, `filter`, `rssi`, `mac clear`,
`start`, `mode softsta`, `sta macset`, `sta mac`, `sta set`, `sta join`, `sta leave`, and `sta stats`.
`sta set` carries session-derived material: do not log or publish its complete arguments.
Events such as `sta linked` and `sta join fail` are asynchronous, not replies to
the immediately preceding command. See `Radio::command` for reply/event handling.

### FX2LP transport

Sources: [fx2.rs](../client-windows/src/fx2.rs),
[FX2 firmware](../firmware-fx2/main.c), [FIFO driver](../firmware-basic/main/fx2_fifo.c).

- USB ID `04B4:8613`, WinUSB, High Speed; bulk OUT `0x02` and IN `0x86`, 512-byte packets.
- Device → PC uses EP6 parallel AUTOIN. PC → device uses EP2 and the PA0 return UART.
- Zero padding completes USB blocks **between complete framed messages**, not inside them.
  The stream decoder skips padding and retains fragments across USB transfer boundaries.
- Vendor IN request `0xB0` supplies the bridge identity; the client checks the 8-byte response
  starts with `B5FX` followed by version byte 1. Request `0xB1` supplies diagnostics.
- RAM loading uses vendor OUT `0xA0`; CPUCS at `0xE600` is set to 1 while loading and
  restored to 0 afterward. This loader does not program EEPROM.

## 3. LDN discovery and authentication

Source: [ldn.rs](../client-windows/src/ldn.rs). All key names below refer to
user-supplied files; this document contains no key values or session secrets.

The implemented advertisement selector matches an 802.11 action frame beginning
with frame-control byte `D0`, vendor bytes `7F 00 22 AA 04` at offset 24,
`01 01` at 30, and a BE u64 value `0x4D` at 36.
These selectors cover the implemented GameShare variant, not every LDN version.

Let `p` start at 802.11 frame offset 36:

| Offset in p | Size | Meaning |
| --- | --- | --- |
| 0 | 32 | Session descriptor |
| 32 | 1 | Version byte copied into authentication |
| 33 | 1 | Encryption selector; implementation requires 3 / AES-GCM |
| 34 | 2 | BE ciphertext length |
| 36 | 4 | Counter bytes |
| 40 | 16 | GCM tag |
| 56 | variable | Ciphertext |

GCM nonce is the four counter bytes followed by eight zero bytes. AAD is `p[0..40]`.
After decryption, plaintext bytes 0–15 contain the network-key material, byte 39
is the participant count (maximum 8), and 48-byte participant records begin at 40.
Within each record, IPv4 is at 0 and MAC at 4. A BE u16 application-data length
follows the participant array; application data must consume the remainder exactly.
The association SSID is lowercase hexadecimal for session-descriptor bytes 16–31.

### Implemented derivation

For source constant `S` and input `I`, `Keys::derive` computes:

```text
k0 = AES128_decrypt(master, aes_kek_generation_source)
k1 = AES128_decrypt(k0, S)
k2 = AES128_decrypt(k1, aes_key_generation_source)
result = AES128_decrypt(k2, SHA256(I)[0..16])
```

Advertisement decoding tries loaded `master_key_*` entries with source constant
`AD` and the session descriptor as input, accepting only a valid GCM tag.
The temporal key uses source `DATA` and `network_key || passphrase` as input.
`AD`/`DATA` are the derivation constants in the source, not extracted passphrases.
Passphrase input is 16–64 raw bytes in the connection path.

The firmware installs the temporal key using ESP-IDF private Wi-Fi interfaces;
the current implementation uses pairwise key index 0 and group RX index 1.
This is not ordinary WPA2-PSK password configuration.

### Ethernet authentication

Ethernet bytes 12–19 are `88 B7 00 22 AA 01 02 00`, combining EtherType and vendor prefix.
The request then contains a 72-byte header, 16-byte GCM tag and 100-byte encrypted payload.
The payload begins with a zero-padded 32-byte UTF-8 participant name and BE u16
application version; remaining bytes are zero in the current writer.

Header bytes 56–71 are fresh random input to an authentication-key derivation
using `DATA` and, currently, `master_key_12`. GCM uses header bytes 0–11 as nonce
and all 72 bytes as AAD. Response acceptance checks reversed MACs, direction,
version, context, exact length and GCM integrity before accepting status 0.
The hardcoded authentication revision is a compatibility limitation, not a rule
established for every console version.

## 4. LRP control

Sources: [lrp.rs](../client-windows/src/lrp.rs), [flat.rs](../client-windows/src/flat.rs).

The advertisement's application data is a 64-byte signature slot followed by a
Zstandard-compressed FlatBuffers envelope. The decoder limits expansion to 64 KiB
and requires matching-information envelope kind 1. Selected table field indexes:

| Index | Current interpretation |
| --- | --- |
| 0 | Matching-ID vector |
| 2 | Control TCP port, u16 |
| 7 | Inline 16-byte host-user identifier |
| 15–18 | Width, height, FPS and bitrate, u32 each |
| 21 | Audio rate, u32 |

TCP records use `[BE u32 length][64-byte signature slot][FlatBuffers body]`.
Length includes the signature slot, excludes its own four bytes, and must exceed 64.
Body size is limited to 4 MiB. Buffer split/coalesced TCP reads until a complete record exists.
Unlike the advertisement, this TCP body is parsed directly, without Zstandard decompression.

The root envelope uses field 0 for a version table (current value `0x00070000`),
field 1 for kind, and field 2 for a payload table. FlatBuffers scalars/offsets are LE;
**field indexes are not fixed byte offsets**. The current client uses kinds
2=request, 3=response and 4=message. See the builders for nested transaction headers.

- `host_information_request`: request union kind 3; fresh 32-byte challenge and advertised host-user ID.
- `session_request`: request union kind 1; fresh challenge/user ID, UTF-8 display name,
  local feedback/media ports and two requested width/height entries.
- Session response: status 200 is accepted; session table fields 2/3 supply feedback/time-sync ports.
- `controller_info`: current JoyDual-style controller configuration.
- Video-format notification: message kind 3, notice kind 1; provides a stream ID and dimensions.
  **Use this host-provided ID** for keyframe, invalidation and QoS messages.
  It is not the same namespace as LSP stream indexes 0/1.
- Stream commands use message kind 4. Current command kind 3 is used for keyframe/reference
  recovery; kind 6 is QoS. `qos` encodes capacity/error values as LE u16
  `min(value_bps >> 15, 0x7FFF)`; treat the full control schema as incomplete.

**Signature verification is not implemented.** Outgoing records have a zeroed
signature slot and incoming signatures are retained but not checked. GCM validation
at the LDN layer does not make an LRP signature verified.

## 5. LSP media and MoLive records

Source: [lsp.rs](../client-windows/src/lsp.rs). These names follow the implementation.

UDP datagrams begin `EC C2`; bytes 2–4 hold a BE 24-bit sequence number.
The BE u16 at offset 8 is the inner-record length `R`; the full datagram must be
exactly `R + 16` bytes. The parser's inner region ends at `8 + R`; the last eight
bytes are an outer timestamp/footer and **must not become media bytes**.
Start inner-header parsing at offset 12. A zero fragment terminator may be present,
but full-size datagrams can end at the inner boundary without one.

Optional `L2` connection header:

- Two-byte marker, BE u16 check value, BE u64 time value, and two additional header bytes.
- Mask the time's high bit, XOR-fold its four 16-bit words, then require
  `check XOR folded == 0xAAAA`.
- Follow with extension `(id, length, bytes)` entries; integer bytes use seven data
  bits with the high bit indicating continuation. ID 0 terminates the extension list.
  Current bounds are four bytes per integer and 64 extensions.

Next is a flags byte; bit 1 adds a two-byte field that the decoder skips.
Each fragment descriptor is read MSB-first:

1. Read a bounded integer for stream index: count zero bits before the first one;
   `n = zeros + 1`, then read the next `n` bits as the value. This is the code's
   custom representation; do not substitute ordinary Exp-Golomb decoding.
2. Read one final-fragment bit.
3. If final, read a kind integer in the same representation, then a signed time delta:
   one sign bit, a zero-count/one prefix giving `n`, then `26 + 2*n` magnitude bits.
4. Read 13 bits and add 1 for payload byte length; round the descriptor up to a byte boundary.
5. Copy exactly that payload length and repeat within the inner boundary.

The decoder restricts stream indexes to below 32, descriptors to 256 per datagram,
and reassembled records to 4 MiB. It exposes fragment time deltas but does not use
them as the playback clock.

Reassembly appends fragments separately per stream until a final fragment.
A 24-bit sequence gap clears all partial streams. Duplicates/older packets are
discarded rather than reordered. Malformed datagrams are counted and clear partials;
the live path continues instead of terminating the session.

Complete media records begin `Tlv0` followed by BE u32 total record length,
including the eight-byte record header. Inner tags have four-byte names and BE u32
value lengths, excluding each tag's own eight-byte header:

| Tag | Implemented interpretation |
| --- | --- |
| `Sch0` | Exactly 24 bytes: BE u64 sequence, timestamp A, timestamp B |
| `Pac0` | Media bytes; must be the only payload tag and the final tag |

Other tags are skipped using their lengths. Current stream 0 is H.264 Annex B;
stream 1 is audio. The audio payload has BE u32 Opus length at offset 0, four
additional wrapper bytes, then the Opus packet at 8. The length must equal
`payload_length - 8` and be at most 1275. The renderer currently uses 48 kHz stereo.
The playback code interprets schedule timestamp A shifted right by 8 as nanoseconds;
the general timestamp semantics beyond this implementation remain incomplete.

## 6. Time sync and input feedback

Source: [feedback.rs](../client-windows/src/feedback.rs).

Time sync is an application-specific 24-byte UDP exchange, **not a standard NTP packet**.
Requests have 16 zero bytes then BE u64 local monotonic time `t0` in nanoseconds.
Replies contain BE u64 `t0, t1, t2`; receive time `t3` is sampled locally.

```text
host_offset = ((t1 - t0) + (t2 - t3)) / 2
round_trip = max(0, (t3 - t0) - (t2 - t1))
```

The client retains the lowest-RTT sample and starts input after obtaining an offset.
Initial probes are scheduled every 30 ms, then every second after synchronization.
Input is scheduled about every 8 ms; these are implementation choices, not timing guarantees.

Feedback datagram header, followed by Zstandard level-1 compressed record bytes:

| Offset | Size | Encoding | Current value/meaning |
| --- | --- | --- | --- |
| 0 | 4 | BE u32 | `0x00070000` |
| 4 | 4 | BE u32 | Constant 1 |
| 16 | 4 | BE u32 | Datagram sequence |
| 20 | 2 | BE u16 | Compressed length |
| 22 | 2 | BE u16 | Decompressed record-byte length |
| 24 | 8 | BE u64 | Local monotonic time plus estimated host offset |
| 56 | variable | bytes | Zstandard frame |

### Npad record: type 1, 96 bytes

| Offset | Size | Encoding | Current value/meaning |
| --- | --- | --- | --- |
| 0 | 1 | u8 | Type 1 |
| 2 | 2 | BE u16 | Record size 96 |
| 8 | 1 | u8 | Constant 1 |
| 16 | 8 | BE u64 | Style value 4 |
| 32 | 8 | BE u64 | Sampling number |
| 40 | 8 | BE u64 | Button mask |
| 48 | 16 | Four BE i32 | Left X/Y, right X/Y, clamped to ±32767 |
| 64 | 4 | BE u32 | Attributes `0x15`; followed by four zero bytes |

Button bits 0–15 are A, B, X, Y, L3, R3, L, R, ZL, ZR, Plus, Minus,
D-pad Left, Up, Right, Down. System buttons must not be put in stick-direction bits 16/17.
The attributes at 64 are **u32 plus padding**, not a BE u64.

### Experimental six-axis record: type 3, 144 bytes

| Offset | Size | Encoding | Current value/meaning |
| --- | --- | --- | --- |
| 0 | 1 | u8 | Type 3 |
| 2 | 2 | BE u16 | Record size 144 |
| 8 | 1 | u8 | Constant 1 |
| 12 | 4 | BE u32 | Handle: 5 or `0x10005` |
| 24 | 8 | BE u64 | Time/offset field; see limitation below |
| 32 | 8 | BE u64 | Sampling number |
| 44 | 12 | Three BE f32 | Acceleration |
| 56 | 12 | Three BE f32 | Angular velocity |
| 68 | 12 | Three BE f32 | Angle |
| 80 | 36 | Nine BE f32 | Direction matrix |
| 116 | 4 | BE u32 | Constant 1 |

Gyro mode appends one record for each handle after Npad: 384 uncompressed bytes total.
Without gyro it sends only the 96-byte Npad record. Neutral acceleration `(0,0,-1)`
and identity direction match the neutral regression fixture.

Peer batches represented by the research observations have offsets 5, 10, … ms for
successive samples. The current single-sample writer **infers** that its field at
24 should be the send interval in nanoseconds; this is not gameplay-validated.
Likewise, the mouse model uses rotations/second, Euler rotations and a direction
matrix as described in [DEVELOPMENT.md](DEVELOPMENT.md#mouse-gyro).
Neutral byte agreement validates framing, not moving-sensor units, axes or host behavior.

## 日本語の案内

[通信仕様リファレンスの日本語版](PROTOCOL.ja.md)と[開発ガイドの日本語版](DEVELOPMENT.ja.md)を参照してください。
