# Milestone 1 on-site test

Goal: decide whether phones are good enough to replace lavs, before building the full
5-phone system. Fill in the results table at the end and send it back.

**Bring:** the studio Mac with both apps installed (see README), an iPhone with TSTV Mic,
AirPods (Pro 2 / AirPods 4 or newer if possible), a lav for comparison, and if possible a
spare Wi-Fi router.

## 0. Prep (10 min)

1. On the StudioLive: recall scene **"Phone Mics"**. Or set channel 17 Input = **USB**,
   Gate off, name it "Phone 1". **Don't update the mixer firmware.**
2. Plug the Mac into the StudioLive by USB and open **TSTV Phone Mics**. The status pill
   should say `Output: StudioLive 16 · 64 ch · 48 kHz`.
3. **Routing check, no phone:** press **Test tone** on PHONE 1.
   - Board channel 17 should show a steady tone at about −18 dBFS. Press it again to stop.
   - Optional: change **Out** to Ch 18 (with channel 18 set to USB) and confirm the tone
     moves. Then set it back to 17.

## 1. AirPods → channel 17

1. Pair the AirPods with the iPhone and put them in.
2. Open TSTV Mic, connect to the desktop, and check the mic menu (bottom left) shows the
   AirPods.
3. Talk. Check the meter moves on the phone, on the desktop strip and on board channel 17.
4. Tap the big button: MUTED on the phone → "Talent muted on phone" on the desktop and
   silence on ch 17. Tap again → LIVE.
5. Press **Mute** on the desktop strip → the phone shows "Muted by the board operator".
   Un-mute.
6. Lock the phone and keep talking for 2 minutes. Audio should continue.

## 2. Lip-sync test (the important one)

We measure two ways. If time is short, do **2a** only.

### 2a. Clap on camera (total delay you'd see on air)

1. Talent wears **both** the lav and the AirPods, close to camera, face and hands in shot.
2. On the TriCaster, record program output with audio for about 30 seconds.
3. Talent claps sharply **5 times**, 2 seconds apart.
4. Do two takes:
   - Take A: lav fader up, phone (ch 17) down.
   - Take B: phone up, lav down.
5. Afterwards, open each recording in a video editor that shows the audio waveform
   (DaVinci Resolve is free; iMovie/Premiere also work). For each clap, find the frame where
   the hands first touch and the frame where the audio spike starts.
   - **Delay = (audio frame − video frame) × 33.4 ms** at 29.97 fps (× 16.7 ms at 59.94 fps).
   - Average the 5 claps.
6. The number that matters is **phone delay − lav delay**: what the phones add compared
   with today's mics.

### 2b. Sample-accurate (phone vs lav directly)

1. Record board channels 1–32 to a computer over USB (PreSonus Capture, Studio One, or
   Audacity, from the "StudioLive 16" input). This can run on the same Mac.
2. Talent wears the lav and the AirPods and claps 5 times.
3. In the recording, compare the clap on the lav channel with the clap on channel 17.
   The time difference is exactly what the phone path adds.

### Repeat for these setups

Note the desktop's **Latency (est.)** and **Loss** each time, so we can calibrate the estimate.

| # | Mic | AirPods HQ setting | Desktop Buffer |
|---|---|---|---|
| 1 | AirPods | off | 40 ms (default) |
| 2 | AirPods | off | 20 ms |
| 3 | AirPods | **on** (Settings on the phone) | 40 ms |
| 4 | iPhone mic (pick it in the mic menu) | – | 40 ms |
| 5 | iPhone mic | – | 20 ms |

At 20 ms buffer, listen for crackles. If you hear any, try 30 ms.

**Target:** under ~100 ms total. Above ~150 ms is likely visible. If the delay is steady,
another option is to delay the TriCaster's video to match.

## 3. Sound quality vs lav

Same person, lav and AirPods at once. Record or listen on headphones from the board,
switching between ch 17 and the lav channel. Try normal speech, loud laughter and turning the
head. Try the AirPods HQ setting both ways. Notes: tone, noise, dropouts, level.

## 4. Network

Run test 1 and setup #1 of test 2 on each network you might use:

- **Campus Wi-Fi.** Does the desktop appear in the phone's list, or did you have to type
  the IP? Does it connect at all?
- **Dedicated router** (Mac and phone both on it).

For each, write down **Loss**, **Jitter** and **Round trip** from the desktop strip, and
walk around the studio and set to see if they get worse.

## 5. Teams on the same phone

While the phone is live, start or join a Teams call **on the same phone**.

Expected: Teams takes the mic. The phone shows "NO MIC / Microphone in use by another app"
and the desktop shows "Mic taken by another app".

Note what actually happens, and whether audio comes back by itself when the call ends
(also try opening TSTV Mic again).

## 6. Optional: 15-minute run

Leave the phone live for 15 minutes (screen locked). The desktop's latency number should
stay steady (within a few ms), with no dropouts. Afterwards, send the log file from the
path shown at the bottom of the desktop window.

---

## Results (fill in and send back)

**Date / place:** ________________ **iPhone model / iOS:** ________________ **AirPods model:** ________________

| Setup | Network | Mic | HQ | Buffer | Desktop latency est. | Loss % | 2a: phone − lav (ms) | 2b: phone − lav (ms) | Crackles? |
|---|---|---|---|---|---|---|---|---|---|
| 1 | | AirPods | off | 40 | | | | | |
| 2 | | AirPods | off | 20 | | | | | |
| 3 | | AirPods | on | 40 | | | | | |
| 4 | | iPhone | – | 40 | | | | | |
| 5 | | iPhone | – | 20 | | | | | |

- **Lav delay on camera (2a, take A):** ______ ms
- **Sound quality vs lav (AirPods normal / AirPods HQ / iPhone mic):**
- **Campus Wi-Fi:** discovery works? yes / no · connects? yes / no · loss ___ % · jitter ___ ms
- **Dedicated router:** loss ___ % · jitter ___ ms
- **Locked screen:** kept streaming? yes / no
- **Teams on the same phone:** what happened, did it recover?
- **Anything odd** (crashes, dropouts, confusing screens):
- **Log file** attached: yes / no
