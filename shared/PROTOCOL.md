# TSTV Phone Mics — network protocol, version 1

How the phone app and the desktop app talk to each other. Both apps must follow this
document exactly; change it here first, then in both apps.

Everything stays on the local network. There are two connections per phone:

| Channel | Transport | Port (default) | Direction | Carries |
|---|---|---|---|---|
| Control | WebSocket (TCP), JSON text frames | 47800 | both ways | hello/welcome, mute state, mic in use, stats |
| Audio | UDP datagrams, binary | 47801 | phone → desktop (plus ping/pong) | Opus audio, 10 ms per packet |

## Discovery

The desktop advertises a Bonjour/mDNS service:

- Type: `_tstvmic._tcp` (the service port is the control port)
- Instance name: the desktop's display name, e.g. `TSTV Phone Mics (Studio Mac)`
- TXT record: `proto=1`

If discovery is blocked by the network, the phone can be given the desktop's IP address
by hand (Milestone 2 adds a QR code containing the same information).

Both ports accept IPv4 and IPv6, and phones may use either: many phone hotspots are IPv6-only.

## Control channel (WebSocket)

Connect to `ws://<desktop-address>:47800/`. Each WebSocket text frame is one JSON object with a
`type` field. Unknown fields must be ignored, so either side can add fields later.

### Phone → desktop

**`hello`** — first message after connecting.

```json
{
  "type": "hello",
  "protocol": 1,
  "deviceId": "6F1C…",          // stable per app install (UUID string)
  "name": "Alex",               // talent display name
  "platform": "ios",            // "ios" | "android"
  "model": "iPhone15,3",
  "appVersion": "0.1.0"
}
```

**`status`** — send after `welcome`, then whenever anything in it changes (and at least every 2 s).

```json
{
  "type": "status",
  "talentMuted": false,
  "input": { "name": "AirPods Pro", "kind": "bluetooth" },  // kind: builtIn | bluetooth | wired | other
  "captureLatencyMs": 18.5,   // phone-side delay: mic + OS buffer + 10 ms frame + encoder lookahead
  "inputSampleRate": 24000,   // hardware rate of the mic route
  "battery": 0.82,            // 0..1, or null if unknown
  "interrupted": false        // true while another app/call has taken the mic
}
```

### Desktop → phone

**`welcome`** — reply to `hello` when the phone is given a slot.

```json
{
  "type": "welcome",
  "protocol": 1,
  "slot": 1,
  "channel": 17,
  "token": 2882400018,        // u32; goes in every UDP packet from this phone
  "udpPort": 47801,
  "bitrate": 128000,          // Opus bitrate the phone should use (bits/s)
  "desktopName": "TSTV Phone Mics (Studio Mac)"
}
```

**`reject`** — reply to `hello` when the phone can't join. The desktop closes the socket after it.

```json
{ "type": "reject", "reason": "full", "message": "All phone slots are in use." }
```

`reason` is one of `full`, `protocol` (version mismatch), `bad_hello`.

**`operator`** — the board operator's mute for this phone. Sent after `welcome` and on every change.

```json
{ "type": "operator", "operatorMuted": true }
```

**`stats`** — sent about twice a second so the phone can show connection health.

```json
{ "type": "stats", "latencyMs": 64.2, "lossPct": 0.4, "channel": 17 }
```

### Sessions and reconnects

- A phone's `deviceId` identifies it. If a phone connects with the same `deviceId` as an
  existing connection, the old connection is closed and the new one takes over the same slot.
- When the WebSocket closes, the desktop stops playing that phone (fades to silence).
- Each `welcome` carries a new random `token`. UDP packets with an unknown token are ignored.

## Audio channel (UDP)

The phone sends UDP datagrams to `<desktop-ip>:<udpPort>`. The desktop learns the phone's
UDP address from these packets and sends pings back to it.

All integers are **big-endian**.

### Common header (8 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | Magic `0x54 0x56` (ASCII "TV") |
| 2 | 1 | Version = `1` |
| 3 | 1 | Kind: `1` audio, `2` ping, `3` pong |
| 4 | 4 | Token from `welcome` (u32) |

### Audio packet (kind 1), phone → desktop

| Offset | Size | Field |
|---|---|---|
| 8 | 4 | Sequence number (u32). +1 per 10 ms frame, starting anywhere; wraps |
| 12 | 1 | Flags. Bit 0: talent muted |
| 13 | 1 | Codec: `1` = Opus, 48 kHz, mono, 10 ms frames (480 samples) |
| 14 | 2 | Length `N` of the primary frame |
| 16 | N | Opus frame for this sequence number |
| 16+N | 2 | Length `M` of the redundant frame (0 if none) |
| 18+N | M | Copy of the Opus frame for sequence number − 1 |

Rules:

- The phone sends one packet every 10 ms for as long as it is connected, **including while
  muted**, so the timing and the network path stay alive.
- While the talent is muted, the phone sets flag bit 0 and sends `N = 0` (no audio leaves the
  phone). The desktop plays silence for that frame.
- The redundant frame lets the desktop recover any single lost packet with no extra delay.
  Packets must fit in one datagram (keep under 1200 bytes).
- Opus encoder settings (phone): 48 kHz, mono, `OPUS_APPLICATION_RESTRICTED_LOWDELAY`,
  10 ms frames, bitrate from `welcome` (default 128 kb/s), VBR.

### Ping (kind 2), desktop → phone; Pong (kind 3), phone → desktop

| Offset | Size | Field |
|---|---|---|
| 8 | 8 | Desktop timestamp (opaque to the phone) |

The phone replies to every ping immediately with a pong carrying the same 8 bytes. The
desktop uses the round-trip time in its latency estimate.

## Latency estimate (shown on the desktop)

```
end-to-end ≈ captureLatencyMs (phone)
           + round-trip time / 2
           + desktop jitter-buffer depth
           + desktop output latency (audio device buffer + driver)
```

This doesn't include delay inside Bluetooth earbuds that iOS doesn't report, or the mixer's
own processing (~1 ms). The clap test in `docs/M1_TEST.md` measures the real number.
