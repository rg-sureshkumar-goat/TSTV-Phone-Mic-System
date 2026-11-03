# TSTV Phone Mics

Lets TSTV News talent use their own phones (built-in mic or AirPods) as on-air microphones.
Each phone streams over the local network to a desktop app, which plays it out of its own
USB channel into the PreSonus StudioLive 16 (phones on channels 17–21).

See [PROJECT_BRIEF.md](PROJECT_BRIEF.md) for the full brief.

| Folder | What's in it |
|---|---|
| `desktop/` | Desktop app (Mac now, Windows in Milestone 2) — Tauri + Rust |
| `mobile/ios/` | iPhone app — Swift / SwiftUI |
| `shared/` | Network protocol spec shared by both apps |
| `third_party/opus/` | Opus audio codec source (BSD licence), compiled into both apps |
| `docs/` | Setup guides and on-site test procedures |

Setup instructions are added with Milestone 1.
