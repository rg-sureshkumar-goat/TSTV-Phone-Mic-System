# TSTV Phone Mics

Lets TSTV News talent use their own iPhone (built-in mic or AirPods) as an on-air microphone.
Each phone streams over the local network to a desktop app on the studio Mac, which plays it
out of its own USB channel into the PreSonus StudioLive 16 (phones use channels 17–21).

**Status: Milestone 2 in progress.** The Mac app and iPhone app handle up to five phones and
have been tested on real devices. The Android and Windows apps run on real devices but each
has an audio-gap problem to fix first. What's left is in [docs/PLAN.md](docs/PLAN.md); device test
steps are in [docs/M2_DEVICE_TESTS.md](docs/M2_DEVICE_TESTS.md) and the on-site test in
[docs/M1_TEST.md](docs/M1_TEST.md). Full brief: [PROJECT_BRIEF.md](PROJECT_BRIEF.md).

| Folder | What's in it |
|---|---|
| `desktop/` | Desktop app (Mac and Windows): Tauri + Rust |
| `mobile/ios/` | iPhone app: Swift / SwiftUI, open in Xcode |
| `mobile/android/` | Android app: Kotlin / Jetpack Compose |
| `.github/workflows/` | Builds the Windows installer and Android app on GitHub |
| `shared/PROTOCOL.md` | How the two apps talk to each other |
| `third_party/opus/` | Opus audio codec source (BSD licence), built into both apps |
| `docs/` | On-site test procedure, design notes |

---

## 1. One-time setup on the studio Mac

You need a Mac with **Xcode** installed (free, from the App Store). Then, in **Terminal**
(Applications › Utilities › Terminal), run each of these once:

1. Install Rust (the language the desktop app is written in). Press Enter to accept the
   defaults when it asks:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   Then close Terminal and open it again.
2. Install **Node.js**, the LTS version, from <https://nodejs.org> (normal Mac installer). It's
   only used to run the desktop app's build tool.
3. Download this project:
   ```bash
   git clone https://github.com/rg-sureshkumar-goat/TSTV-Phone-Mic-System.git
   ```
   This makes a folder called `TSTV-Phone-Mic-System` in your home folder. Use
   `git pull` inside it later to get updates.

## 2. Build and run the desktop app

In Terminal:

```bash
cd ~/TSTV-Phone-Mic-System/desktop
```

```bash
npm install
```

```bash
npm run build
```

The first build takes a few minutes. When it finishes, the app is at
`desktop/src-tauri/target/release/bundle/macos/TSTV Phone Mics.app`. Open that folder
with:

```bash
open "src-tauri/target/release/bundle/macos"
```

Drag **TSTV Phone Mics** into your **Applications** folder, then open it from there.

**First launch:**
- If macOS asks *"Allow TSTV Phone Mics to find devices on your local network?"*, click **Allow**.
- If it asks *"Do you want the application to accept incoming network connections?"*, click **Allow**.
- If you copied the app from another Mac and macOS says it *can't check it for malicious
  software*: right-click the app › **Open** › **Open**. Or go to System Settings › Privacy &
  Security, scroll down, and click **Open Anyway**. (Unsigned app; this is expected.)

**Using it:**
- The top bar shows the address phones connect to, e.g. `10.150.143.42`.
- **Connect a phone** shows a QR code for talent to scan. You can also require a four-digit
  join code there, so other phones on the network can't put audio on the board.
- **Output device** defaults to *Automatic*: the StudioLive if it's plugged in, otherwise
  the Mac's speakers. If you pick the StudioLive by name, the app waits for it rather than
  playing anywhere else.
- There's one strip per phone, **PHONE 1** to **PHONE 5**. From top to bottom:
  - **Out** picks the output channel (defaults: channels 17–21).
  - Status: Live, Reconnecting, Disconnected, and so on.
  - **Lock** keeps this slot for this phone, even if it drops or the app restarts. A phone
    that drops normally returns to its slot anyway, unless someone else has taken it.
  - Talent name and which mic they're using. Click the name to change it (clear it to go
    back to the name on the phone).
  - Level meter and gain fader (double-click the fader for 0 dB).
  - **Mute**: the operator's mute. It overrides the phone and the phone shows it.
  - **Test tone**: a 1 kHz tone at −18 dBFS on that channel, so you can check routing
    without a phone.
  - **Latency (est.)**: the app's estimate of the total delay.
  - **Buffer**, **Loss**, **Jitter**, **Round trip**: network health.
- **Buffer** (top right) sets how much audio the desktop holds back to smooth over Wi-Fi
  hiccups. Lower = less delay but more risk of dropouts. Default 40 ms.
- Settings are remembered. A log of connections, dropouts and errors is written to
  `~/Library/Logs/org.tstvnews.phonemics/tstv-phone-mics.log` (path shown at the bottom
  of the window).

**Board prep** (done on the mixer by the TD, not by the app): on channels 17–21 set
Input = USB, turn Gate off, name them "Phone 1"–"Phone 5", **unmute them and set their
faders to 0 dB**, and store a scene called "Phone Mics". The original state is saved as
scene "Before Phones". With those channels open on the board, the app's Mute and gain
are all the operator needs; the board's own fader and mute still work as a backstop.
The app doesn't control the mixer yet (see [docs/PLAN.md](docs/PLAN.md)).

## 3. Install the iPhone app (free, from Xcode)

This uses Apple's free provisioning: no paid developer account, but the app **stops
opening after 7 days** and has to be reinstalled from Xcode (step 7). Up to 3 iPhones per
Apple Account.

1. **Add your Apple Account to Xcode:** open Xcode › Settings › Accounts › **+** ›
   Apple Account, and sign in.
2. **Open the project:** in Finder, open `TSTV-Phone-Mic-System/mobile/ios/` and
   double-click **TSTVMic.xcodeproj**. Wait for "Resolving packages" at the top to finish.
3. **Set the signing team:**
   - In the left sidebar click the blue **TSTVMic** project icon.
   - Under Targets select **TSTVMic**, then open the **Signing & Capabilities** tab.
   - For **Team**, choose *Your Name (Personal Team)*.
   - If Xcode says the bundle identifier isn't available, change **Bundle Identifier** to
     something unique, e.g. `org.tstvnews.phonemic.yourname`.
4. **Prepare the iPhone:**
   - Plug it into the Mac with a cable, unlock it, and tap **Trust** if it asks.
   - Turn on Developer Mode: iPhone **Settings › Privacy & Security › Developer Mode › On**.
     The iPhone restarts; confirm when it comes back.
5. **Install:** at the top of the Xcode window, choose your iPhone in the device menu, then
   press **▶ Run** (or ⌘R).
6. **First launch on the iPhone:** if it says *Untrusted Developer*, go to iPhone
   **Settings › General › VPN & Device Management**, tap your Apple Account, and tap
   **Trust**. Then open **TSTV Mic** from the home screen.
7. **Every 7 days** (or when you `git pull` an update): plug in, open the project, and
   press **▶ Run** again.

**Using the iPhone app:**
1. Type your name (it's remembered).
2. In the desktop app, click **Connect a phone**, then scan the QR code with the iPhone's
   Camera or with **Scan QR code** in TSTV Mic. You can also tap the desktop under **Desktops
   on this network**, or type the address shown at the top of the desktop app.
3. Allow **Microphone** and **Local Network** when asked.
4. The big button is **green = LIVE**, **red = MUTED**. Tap it to switch.
   - It also turns red with "Muted by the board operator" when the desk mutes you.
   - It turns orange if a call or another app takes the microphone.
5. The menu at the bottom left picks the mic (iPhone mic, AirPods, other Bluetooth or
   wired mics). You can switch mid-show.
6. It keeps streaming with the screen locked. Turn on **Do Not Disturb** so calls can't
   interrupt.
7. **Settings** (gear icon on the first screen):
   - *AirPods high-quality recording* (iOS 26+): better sound on AirPods Pro 2 / AirPods 4,
     possibly more delay. Compare both settings in the test.
   - *Noise suppression* and *Auto gain*: off by default, since the mixer's Fat Channel
     normally does this job.

## Windows and Android

Both are downloaded from GitHub rather than built by hand. Open the repo's **Actions** tab, pick
the latest **Build apps** run, and download from **Artifacts** (you need to be signed in to
GitHub). Steps to install and check them are in [docs/M2_DEVICE_TESTS.md](docs/M2_DEVICE_TESTS.md).

## 4. Checking it without a phone

- **Routing only:** press **Test tone** on the desktop strip. The tone should appear on board
  channel 17 at −18 dBFS.
- **Whole desktop side:** a "fake phone" program streams beeps over the network exactly
  like the phone app does. With the desktop app running, in Terminal:
  ```bash
  cd ~/TSTV-Phone-Mic-System/desktop/src-tauri && cargo run --release --example fake_phone
  ```
  Add `-- --loss 2 --jitter 20` to that command to simulate a poor network. Press Ctrl-C
  to stop.

## 5. Troubleshooting

| Problem | Try |
|---|---|
| Phone doesn't list the desktop | Type the IP address instead. Campus Wi-Fi often blocks device discovery. |
| Desktop never appears, on any network | On the Mac: System Settings › Privacy & Security › Local Network › turn on TSTV Phone Mics, then quit and reopen it. |
| Phone shows OFFLINE | Read the small grey text at the top of the phone screen; it says what's wrong. On a laptop on campus Wi-Fi the Mac's address changes as it moves: the desktop app always shows the current one, so if you typed an address, tap **Disconnect** and type the new one. |
| "Local Network access is off" on the phone | iPhone Settings › Privacy & Security › Local Network › TSTV Mic: turn it on, then reopen the app. |
| "Nothing is answering" on the phone | Is the desktop app open? Is that still its address? |
| Phone asks for a join code | It's under **Connect a phone** in the desktop app. Scanning the QR code fills it in. |
| "All phone slots are in use" | All five are taken or locked. Unlock a slot you don't need. |
| Button is orange and says **NO MIC** | The phone isn't getting audio from its mic. It keeps trying to restart it; if it doesn't recover, pick another mic or reopen the app. |
| "Can't reach the desktop" / "didn't answer" on the phone | Campus Wi-Fi is probably stopping devices from talking to each other, even when both show the same network name. Use a dedicated Wi-Fi router for the phones, with the Mac on it too. For a quick test, turn on the iPhone's Personal Hotspot and join it from the Mac. Check the Mac firewall allows TSTV Phone Mics (System Settings › Network › Firewall). |
| Desktop says "Output not open" | Is the StudioLive plugged in and switched on? Pick it from **Output device**. |
| Tone/phone shows on the desktop meter but not on the board | Board channel 17 Input must be **USB**. |
| Desktop warns "set the mixer to 48 kHz" | Set the StudioLive's sample rate to 48 kHz (it still works otherwise, slightly lower quality). |
| Crackles or dropouts | Raise **Buffer** (e.g. 60 ms). Watch **Loss**; if it's above ~1% the Wi-Fi is struggling. |
| "Mic taken by another app" | A phone or Teams call on the same phone takes the mic. End it, and audio resumes. |
| iPhone app won't open after a week | Free provisioning expired: reinstall from Xcode (section 3, step 7). |
| Control port busy / "Is another copy running?" | Quit the other copy of TSTV Phone Mics. |

## For developers

- Desktop: `cd desktop && npm run dev` runs a development build that reloads on changes.
  Unit tests: `cd desktop/src-tauri && cargo test`. For a more detailed log, start the app
  from Terminal with `TSTV_LOG=debug open -a "TSTV Phone Mics"`.
- iPhone: `mobile/ios/TSTVMic.xcodeproj`. Source files in `mobile/ios/TSTVMic/` are picked up
  automatically.
- Protocol between the apps: [shared/PROTOCOL.md](shared/PROTOCOL.md). Design notes and
  reasons for the choices: [docs/DESIGN.md](docs/DESIGN.md).
