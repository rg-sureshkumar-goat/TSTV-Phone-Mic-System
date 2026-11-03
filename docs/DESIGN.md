# Design notes

Why the system is built the way it is, and where it differs from the brief's suggested stack.

## Stack

| Part | Brief suggested | Built with | Why |
|---|---|---|---|
| Transport | WebRTC | Opus in plain UDP packets + JSON over WebSocket ([protocol](../shared/PROTOCOL.md)) | See below |
| Desktop | Tauri + webrtc-rs + opus + cpal | Tauri 2 (Rust) + cpal + libopus | Same, minus WebRTC |
| Phone | React Native (Expo) + react-native-webrtc | Native Swift (iOS); native Kotlin for Android (Milestone 2) | See below |

### Why not WebRTC

- WebRTC's jitter buffer and loss concealment live in Google's libwebrtc. The Rust
  `webrtc-rs` crate doesn't include them, so the desktop would have needed its own anyway.
- On iPhone, libwebrtc forces Apple's voice-processing mic mode (echo cancellation,
  auto gain, a "phone call" EQ). The brief wants all of that off, which needs a custom
  WebRTC build.
- Lip-sync is the deciding test, and WebRTC's adaptive buffer can't be pinned to a set
  delay. Here the operator sets the buffer directly.

### Why native phone apps

Without WebRTC, React Native would only have drawn one screen; everything that matters
(AVAudioSession, AirPods routing, background audio, interruptions) is native code either
way. Native Swift builds with nothing but Xcode, which suits free provisioning. The cost is
writing the small Android UI separately in Kotlin.

## Audio path and delay budget

```
mic ─▶ iOS input buffer (5 ms) ─▶ 10 ms frame ─▶ Opus low-delay (2.5 ms)
   ─▶ Wi-Fi (2–10 ms) ─▶ desktop jitter buffer (operator setting, default 40 ms)
   ─▶ CoreAudio 128-sample buffer + USB (~3–6 ms) ─▶ StudioLive ch 17
```

Estimate with the iPhone mic: about 50–75 ms. Bluetooth earbuds add their own delay,
which iOS only partly reports; the clap test measures it.

### Desktop playout (desktop/src-tauri/src/playout.rs)

- Frames play strictly in order, one per 10 ms. A frame that's late is concealed and
  dropped, which keeps the delay constant.
- Each packet also carries the previous frame, so a single lost packet is rebuilt with no
  extra delay (Wi-Fi losses are often single packets). Longer gaps get Opus loss
  concealment, faded out over 50 ms.
- The phone's and the mixer's sample clocks differ slightly (typically 20–100 ppm, which
  would add up to tens of ms over a show). The desktop keeps its buffer at the target by
  playing up to 0.5% faster or slower (inaudible), using cubic interpolation.
- If the buffer jumps far above the target (e.g. after a network stall), it skips back to
  the target at once rather than slowly.
- No audio for 0.5 s → the slot goes quiet and re-buffers when audio returns.

### Phone capture (mobile/ios/TSTVMic/AudioCapture.swift)

- `AVAudioSinkNode` on the real-time thread copies samples into a lock-free ring. A capture
  thread resamples to 48 kHz if needed (Bluetooth routes run at 16/24 kHz), cuts 10 ms
  frames and hands them to the encoder and sender.
- Session: `.playAndRecord`, `.measurement` mode (no system processing), Bluetooth HFP
  allowed, 5 ms IO buffer, alerts can't interrupt. The optional iOS 26 AirPods
  high-quality mode needs `.default` mode, and Apple warns it may add latency.
- While the talent is muted, no audio leaves the phone: packets keep flowing (to keep
  timing and the network path alive) but carry no audio.

## Things to revisit after the Milestone 1 test

- Buffer default (40 ms), depending on measured loss and jitter on the real network.
- Whether AirPods HQ mode is worth its delay.
- If the delay is steady but too high, delaying video on the TriCaster.
- Windows output: cpal's ASIO support needs Steinberg's ASIO SDK at build time; check what
  channel layout the PreSonus WDM driver exposes.
