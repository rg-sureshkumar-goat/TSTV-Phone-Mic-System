# TSTV Phone Mics — Project Brief

**Owner:** RG Sureshkumar, Technical Director, TSTV News
**Date:** November 3, 2026
**Status:** Board compatibility confirmed by on-site test. Ready to build.

---

## 1. Goal

Let TSTV anchors and talent use their own phones as on-air microphones, using either the phone's built-in mic or Bluetooth earbuds such as AirPods. This replaces wireless lavaliers and wired snake mics. Each phone's audio arrives on its own channel of the station's PreSonus StudioLive 16 mixer, where the board operator EQs and mixes it like any other mic.

Two apps:

1. **Desktop app (Mac + Windows):** runs on a computer connected to the mixer by USB. It receives audio from the phones, sends each phone to its own mixer channel, and gives the operator level and mute control for each phone.
2. **Phone app (iOS + Android):** runs on each talent's phone. It captures the chosen mic and streams it to the desktop app over the station Wi-Fi, with a large mute/unmute button.

**Out of scope:** Microsoft Teams. The crew keeps using Teams for comms, and the apps do not touch it. No crew talkback or intercom features.

---

## 2. Confirmed hardware facts (tested on site, Nov 2, 2026)

| Item | Detail |
|---|---|
| Mixer | PreSonus **StudioLive 16 Series III** (original, not SE). Front panel: "32 Channel Professional Digital Mix System" |
| Firmware | **2.5.84445** (Nov 19, 2021). Do **not** update it as part of this project. |
| Universal Control | v5.1.1 installed on the Mac used for testing |
| USB | USB-B port on the back panel, previously unused. On a Mac the mixer appears as a multichannel CoreAudio device ("StudioLive 16") with no extra driver. On Windows, the PreSonus driver installed with Universal Control is needed. |
| **Channel mapping** | **Computer USB output N → mixer channel N** when that channel's Input source is set to USB. Verified: audio sent to USB output 31 appeared on channel 31. |
| Existing inputs | Channels 1–16 are in use (snake via StudioLive 16R stagebox over AVB, lavs, KVRX). The snake/16R uses the AVB network port, so the phone system must not use AVB. |
| Phone channels | Channels **17–32 are unused**. Phones use **17–21** (up to 5 phones). 22–32 are spares. |
| Backup scene | Original board state is saved as scene **"Before Phones"** (project "Sneak Peek"). |

**Board prep, done manually by the TD, not by the software:** on channels 17–21, set Input = USB, turn off Gate, name them "Phone 1"–"Phone 5", and store as a scene called "Phone Mics."

---

## 3. System architecture (recommended; Claude Code may propose changes with reasons)

```
[Phone app] --mic--> Opus encode --> WebRTC (LAN, Wi-Fi) --> [Desktop app] --> decode --> USB output 17..21 --> StudioLive ch 17..21
     ^                                                            |
     +-------------- control messages (mute, levels, status) <----+  (WebSocket signaling)
```

- **Transport:** WebRTC audio (Opus, mono, 48 kHz) over the local network only, using host candidates with no STUN/TURN. WebRTC supplies the jitter buffer, packet-loss concealment and low latency. The desktop app hosts a WebSocket server for signaling and control messages.
- **Discovery:** the desktop app advertises itself over mDNS/Bonjour. **Fallback:** the desktop app shows a QR code (IP + port + session code) that the phone app scans, because campus Wi-Fi often blocks mDNS and sometimes blocks device-to-device traffic (see Risks).
- **Desktop audio output:** a native audio layer opens the StudioLive as a multichannel output device and writes each phone's decoded stream to its assigned channel index (phone slot 1 → output 17, and so on). macOS: CoreAudio. Windows: ASIO preferred (PreSonus driver), with WASAPI as fallback.
- **Suggested stack:**
  - Desktop: **Tauri (Rust)**, with `webrtc-rs` for receiving, the `opus` crate for decoding, and `cpal` (CoreAudio / ASIO / WASAPI) for multichannel output. The UI uses web tech inside Tauri. *Alternative if better justified:* Electron with a native audio addon. The hard requirement is writing to specific channels (17–21) of a 64-channel device.
  - Phone: **React Native (Expo dev build, not Expo Go)** with `react-native-webrtc`, plus small native modules for audio-session and Bluetooth route control on iOS (AVAudioSession) and Android (AudioManager / Bluetooth SCO).
- **No cloud, no accounts, no paid services.** Everything runs on the station LAN. Hosting must be free.

---

## 4. Desktop app requirements (Mac + Windows)

**Setup**
- Pick the output audio device (default: the StudioLive 16 if found) and the channel range (default 17–21; configurable).
- Show connection info: a QR code plus a short session code. Optional session PIN so random phones on the network can't join.

**Main screen: one strip per phone slot (5 slots)**
- Talent name, as entered on the phone and editable by the operator
- Connection status: connected / reconnecting / disconnected, with signal quality (packet loss %, jitter)
- **Live level meter** (post-gain)
- **Gain/level slider** (−∞ to +12 dB) and a **mute button**. Operator mute overrides the phone.
- Shows when the talent has muted themselves on their phone
- Mic source in use (e.g., "AirPods Pro", "iPhone mic")
- **Latency readout** (estimated end-to-end, ms)
- Assigned output channel, with drag/drop or dropdown to reassign

**Other**
- Master "Mute all phones" button
- Optional per-slot "Lock," so a phone that drops and reconnects returns to the same slot and channel
- Optional test tone generator per slot, for checking routing without a phone
- Remembers settings between launches
- Must run for a full show without memory growth or audio dropouts. Log errors to a file.

---

## 5. Phone app requirements (iOS + Android)

- **Connect:** auto-discover the desktop app on the network, or scan its QR code. Enter display name once (remembered).
- **Big mute/unmute button,** taking up most of the screen. Clear colors: green = live, red = muted.
- **Mic picker:** list the available inputs: built-in mic, AirPods, other connected Bluetooth mics, and wired headset if present. Switching must work mid-show without disconnecting.
- **Level meter,** so talent can see they're being picked up
- **Connection indicator** and auto-reconnect if Wi-Fi blips
- **Keep streaming with the screen locked or the app in the background** (iOS background audio mode; Android foreground service with notification). Keep the screen awake while the app is open.
- **Audio processing:** echo cancellation off (the phone isn't playing program audio). Noise suppression and auto-gain should be **toggleable**, defaulting to off or light, so the board's Fat Channel handles processing. Expose these toggles in a settings screen.
- **Interruptions:** if a phone call, Teams call, or another app takes the mic, show a clear on-screen warning and report it to the desktop app. Resume automatically when possible.
- Mirrors operator mute: if the desktop mutes this phone, the phone shows it.

---

## 6. Build plan and milestones

**Milestone 1: Proof of concept (do first; gates everything else)**
- Desktop app (Mac only is fine) receives **one** phone and plays it to **USB output 17**.
- Phone app (iPhone first) with connect + mute only.
- **Acceptance test at the station:**
  1. Talent wears AirPods. Audio reaches channel 17 on the board.
  2. **Lip-sync test:** talent claps on camera. Compare the clap's sound with the video on the TriCaster program output. Record the measured delay. Target: **under about 100 ms** end-to-end. Above roughly 150 ms is likely visible.
  3. Compare sound quality against a lav on the same person.
  4. Test on the actual network the station will use (campus Wi-Fi vs. a dedicated router).
- **Also test:** whether Teams in an active call on the **same** phone breaks capture. It is expected to break it, because iOS/Android give the mic to the call. Document the result.

**Milestone 2: Full feature set:** 5 phone slots, all desktop controls, mic picker, background mode, reconnect, Android app, Windows desktop.

**Milestone 3: Show-ready:** 30-minute soak test with 5 phones, a crash/dropout log review, and a one-page operator guide plus a talent quick-start guide.

---

## 7. Risks and known constraints

| Risk | Impact | Mitigation |
|---|---|---|
| **Latency / lip sync** | Audio late vs. video | Minimize the jitter buffer; measure in M1. If the delay is consistent, investigate delaying video on the TriCaster TC1 to match. |
| **Campus Wi-Fi blocks device-to-device traffic or mDNS** | Phones can't find or reach the desktop | QR/IP fallback. Recommend a **dedicated Wi-Fi router** for the phones, with the desktop computer on it too. |
| Bluetooth mic quality | AirPods mic uses a lower-quality call codec; quality varies by model | AirPods 4 / Pro 2 or newer on current iOS are best. Allow the built-in phone mic as a fallback. |
| Teams on the same phone | Teams call takes the mic | Crew runs Teams on a separate device. The app warns when the mic is taken. |
| Notifications / calls during show | Interruptions | App reminds talent to enable Do Not Disturb / Focus on connect. |
| Battery | Phone or AirPods die mid-show | Show phone battery % in the desktop strip if feasible. |
| Mixer firmware update | Behavior could change | Don't update the mixer firmware during this project. |

---

## 8. Distribution (decision pending)

- **iPhone:** either (a) Apple Developer Program, **$99/yr**, distributed via TestFlight, or (b) free provisioning from Xcode on the owner's Mac (installs expire every 7 days; limited devices). **Build so either works.** Use free provisioning during development.
- **Android:** sideload a signed APK (free).
- **Desktop:** unsigned builds are fine for station use. Document how to bypass the macOS Gatekeeper and Windows SmartScreen warnings.

---

## 9. Conventions

- The owner creates the GitHub repo and provides the URL. **Claude Code should not create repos.**
- Monorepo suggested: `/desktop`, `/mobile`, `/shared` (message schema), `/docs`.
- README with setup steps a non-developer TD can follow.
- Everything free and open-source. No paid services, no cloud backend.
