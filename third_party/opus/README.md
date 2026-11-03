# Vendored libopus

Official Opus codec source from <https://opus-codec.org> (BSD licence, see `libopus/COPYING`).
Version is in `libopus/VERSION`; the tarball's SHA-256 was checked against
<https://downloads.xiph.org/releases/opus/SHA256SUMS.txt> when it was vendored.

Only the portable floating-point C sources are kept (no CPU-specific optimisations,
no machine-learning extras), so the desktop and phone apps all build the same files
with no extra tools.

| Used by | How |
|---|---|
| iPhone app | Swift package in this folder (`Package.swift`, products `COpus` + `COpusShim`) |
| Desktop app | `desktop/src-tauri/build.rs` compiles `libopus/` with the `cc` crate |

To upgrade: download and verify a new release from downloads.xiph.org, extract it,
then run `./update.sh /path/to/opus-X.Y.Z`.
