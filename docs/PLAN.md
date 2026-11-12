# What's left

Updated 2026-11-11. Packaging for the app stores waits until Milestone 3 passes.

## 1. Problems to fix

| | Problem | Notes |
|---|---|---|
| 🔴 | **Android loses 10-65% of its audio once the phone is locked** | Samsung SM-E156B on home Wi-Fi. Clean while unlocked. The connection stays up, and "Unrestricted" battery made no difference. The round trip stayed ~9 ms while locked, so Wi-Fi napping is unlikely; recording in bigger, uneven chunks with the screen off is the suspect. Plan: test over wireless debugging, then move recording to AAudio/Oboe low-latency. Fallback: a bigger buffer for that phone on the desktop (adds delay). |
| 🔴 | **Windows loses 1-30% of the audio** | Laptop on Wi-Fi, on battery. Jitter ~15 ms (the same iPhone into the Mac: under 1 ms); whenever jitter fell to ~1 ms, loss fell to 0. First check: plugged in, and Wi-Fi power saving set to Maximum Performance. If that doesn't fix it, look at the Windows receive path. |
| 🟡 | Android hiccup about every 2 minutes (~0.7 s late) | Seen with the screen on. |
| ⚪ | Meter "0" label overlaps the red channel warning | Cosmetic. |

## 2. Tests that need the studio or new gear

- Router: 30 minutes in the studio. Computer on the router's Wi-Fi (Auto-Join off for school Wi-Fi) or by cable.
- Clap lip-sync test (target under ~100 ms). Mac measures ~65 ms; Windows ~100 ms with laptop speakers.
- Windows laptop with the mixer: does WASAPI reach channels 17-21? If not, add ASIO.
- AirPods over Wi-Fi; Bluetooth earbuds on Android.
- Echo in the control room: where talent can stand.
- Windows: settings kept after a restart (not reported yet).

## 3. New feature: control the mixer from the app (revisit once the router arrives)

**Goal:** when a strip is set to a channel (Out 17), its Mute and fader also mute and move
that channel on the StudioLive, so nobody has to unmute on the board first. Changes made on
the board show in the app, so both stay in step.

**How:** the StudioLive's network port, which PreSonus's UC Surface uses. PreSonus doesn't
document it; the open-source [presonus-studiolive-api](https://github.com/featherbear/presonus-studiolive-api)
has worked it out and lists the StudioLive 16 as tested. Port what we need to Rust in the
desktop app.

**Needs:** the mixer's network port cabled to the router (at the mixer; the computer still
needs only USB). Studio time with the board to develop and test. Check it works with firmware
2.5; don't update firmware.

**Safety:** only touch the channels the strips are set to (17-21), never anything else. A
switch to turn mixer control off. Show clearly when the app is or isn't controlling the board.

**Risk:** an undocumented protocol could change with a firmware update.

**Until then:** board prep stores channels 17-21 unmuted at 0 dB in the "Phone Mics" scene.

## 4. Milestone 3: show-ready

- iPhone app on TestFlight so other people's iPhones can install it.
- 5 phones (iPhone and Android) for 30 minutes, then review the logs.
- One-page operator guide and talent quick-start.
- Optional: warnings when a phone's connection gets shaky, and an after-show report.

## 5. Packaging (after Milestone 3)

- iPhone: App Store listing, privacy policy page, screenshots, App Review, then unlisted distribution. The developer account is in place.
- Android: permanent signing key, then Google Play with a join link.
- Windows: Microsoft Store (free account) as a hidden, link-only app; check we can build MSIX, which the Store signs for free.
- Mac: sign and notarize with the developer account.
- Version 1.0, and merge into `main`.
