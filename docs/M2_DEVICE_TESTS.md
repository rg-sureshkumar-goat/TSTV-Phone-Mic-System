# Milestone 2: testing Android and Windows

Both apps are built, but no real Android phone or Windows PC has run them yet.

| | Tested so far | Needs a real device |
|---|---|---|
| **Android** | Emulator: QR link, join code, connecting, mute, screen off, automatic reconnect | Real sound from the mic, Bluetooth mics, QR scanner, interruptions, delay |
| **Windows** | Tests and installer build on GitHub's Windows machines | **Everything else, including the app opening and reaching channels 17-21** |

Keep the channel faders down until you're ready to listen.

## Getting the apps

On GitHub: **Actions** tab › latest **Build apps** run › **Artifacts** (sign in first).

- `TSTV-Mic-Android` contains `app-debug.apk`.
- `TSTV-Phone-Mics-Windows` contains the installer.

## Android

**Install:**
1. Copy `app-debug.apk` to the phone and open it.
2. Allow installing from that source if asked. Android may warn about an unknown app.
3. To update later, uninstall TSTV Mic first, because each GitHub build is signed differently.

**Check:**
1. Allow Microphone, Nearby devices and Notifications when asked.
2. Connect by each route: **Scan QR code**, the phone's camera app, the list, and a typed address.
3. Talk. The meter should move on the phone, on the desktop strip and on the board.
4. Pick a Bluetooth mic from the menu at the bottom left, and talk again. *This is the part most
   likely to need fixing, because it varies between phones.*
5. Lock the phone for 2 minutes and keep talking. The notification should stay, and the strip
   should stay Live. Some phones' battery savers stop background apps: if audio stops, set TSTV Mic
   to "unrestricted" in its battery settings, and tell me which phone it was.
6. Make a phone call while live. The button should turn orange and say NO MIC, then recover.
7. Turn on Noise suppression and Auto gain in Settings, and listen.
8. Run the clap test in [M1_TEST.md](M1_TEST.md) on an Android phone, because its delay may differ
   from an iPhone's.

## Windows

**Install:**
1. Install the PreSonus driver (it comes with Universal Control) and connect the mixer by USB.
2. Run the installer. If SmartScreen warns you: **More info › Run anyway**. The app is unsigned, so
   this is expected.
3. When Windows Firewall asks, allow **Private networks**. If you say no, phones can't connect.
4. On networks Windows treats as "Public", the firewall may still block phones. Set the network
   to Private in Windows Settings.

**Check, in this order:**
1. **Does the app open and show five strips?** If the window stays blank, install Microsoft's
   WebView2 Runtime.
2. **Is the StudioLive in Output device, and how many channels does the status show?** This is
   the most important check. The app needs at least 21. If it shows fewer, send me the number and
   every StudioLive entry in the list: I'd then add ASIO support.
3. **Test tone** on Phone 1: board channel 17 should read about -18 dBFS.
4. Connect a phone, by QR code and from the list. The list uses a different discovery method on
   Windows than on the Mac, so tell me if the entry looks wrong or appears twice.
5. Talk, and check the **Latency (est.)**. Windows audio may add more delay than the Mac.
6. Quit and reopen the app, and check that settings are remembered.

**Send me:** what happened at each step, plus the log file (its path is at the bottom of the window).
