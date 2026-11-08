# Third-party notices

3zT Mirroring is free software under the GNU General Public License v3.0 or
later (see `LICENSE`). It is a modified version of other open-source work and
bundles the components below, each under its own licence.

## Based on

- **ZenLink** by Zenvy Zenshiro (GPL-3.0-or-later) —
  https://github.com/zenvyzenshiro/zenlink. 3zT Mirroring started from ZenLink
  2.5.1 and changes its name, look, window, keyboard handling, updater and
  installer.
- **pane-iphone-mirror-windows** by teyboxv1 (GPL-3.0) —
  https://github.com/teyboxv1/pane-iphone-mirror-windows
- **pymobiledevice3** by Doron Zavelevsky (GPL-3.0) —
  https://github.com/doronz88/pymobiledevice3
- **quicktime_video_hack** by Daniel Paulus (MIT) — reference for the
  QuickTime screen-capture USB protocol. https://github.com/danielpaulus/quicktime_video_hack
- **UxPlay** by F. Duncanh and **RPiPlay / shairplay** (GPL-3.0) — AirPlay
  mirroring protocol reference. The **playfair** FairPlay code in
  `native/playfair` is used unmodified (GPL-3.0).
- **libwdi / Zadig** by Pete Batard (LGPL / GPL) — the driver-package
  approach; Zadig itself is not bundled.

## libusb-win32, in `drivers/libusb-win32`

Stephan Meyer & Travis Lee Robinson. Driver GPL, library LGPL. Bundled
unmodified (signed binaries); source: https://sourceforge.net/projects/libusb-win32/

## FFmpeg (AAC decoder only), in `mirroring/aacdec.dll`

- The AAC / AAC-ELD decoder (`libavcodec`), `libswresample` and `libavutil`,
  statically linked into `aacdec.dll` (source: `native/aacdec/`, built by
  the release workflow from the FFmpeg tarball below).
- FFmpeg **9.0.2**, https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz
  (SHA-256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`).
- Licence: **LGPL-2.1-or-later** (no `--enable-gpl`, no `--enable-nonfree`).
  The configure line is in `native/build_ffmpeg.sh`.

## scrcpy server, in `third_party/scrcpy`

- The official scrcpy server (`scrcpy-server-v5.0.1`), pushed to the Android
  device and run there. 3zT Mirroring has its own client
  (`rust/crates/mirror-android`).
- https://github.com/Genymobile/scrcpy/releases/tag/v5.0.1, SHA-256
  `764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536`
  (as in the release's `SHA256SUMS.txt`, kept next to it).
- Licence: **Apache-2.0** (`third_party/scrcpy/LICENSE`), Copyright (C) 2018
  Genymobile, Copyright (C) 2018-2026 Romain Vimont.

## Android platform-tools `adb`, in `third_party\platform-tools` (installed copy)

- `adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll` and `NOTICE.txt` from
  Google's Android SDK Platform-Tools **37.0.1**, unmodified and signed by
  Google LLC (`platform-tools_r37.0.1-win.zip`, SHA-256
  `45f4d63113e895ebde0c90f194099a4676b6ac653bd28d54314a9e022bbc1a99`).
- adb is Android Open Source Project code under Apache-2.0; its notices are in
  `NOTICE.txt`. It runs as a separate process.

## Apple Mobile Device Support

Not bundled. When it is missing, Setup downloads Apple's own
`AppleMobileDeviceSupport64.msi` from Apple's servers (checked against its
SHA-256) and installs it. It is Apple software under Apple's licence.

## Design references

Motion constants and the glass look are re-implemented from these projects
(no code copied verbatim):

- **shayann07/liquidglass** (Apache-2.0) — https://github.com/shayann07/liquidglass
- **al1h3n/liquid-rust** (MIT OR Apache-2.0) — https://github.com/al1h3n/liquid-rust

## Python libraries (in the installed service)

aiohttp (Apache-2.0) · pymobiledevice3 (GPL-3.0) · pyusb (BSD-3) ·
python-sounddevice (MIT) with PortAudio (MIT) · python-zeroconf (LGPL-2.1) ·
PyCryptodome (BSD / Public Domain) · cryptography (Apache-2.0 / BSD).

## Rust crates (in the window)

Slint (GPL-3.0 for open-source use), windows-rs (MIT / Apache-2.0),
serde / serde_json (MIT / Apache-2.0), qrcodegen (MIT), miniz_oxide
(MIT / Zlib / Apache-2.0), ureq (MIT / Apache-2.0), log (MIT / Apache-2.0),
base64 (MIT / Apache-2.0).

Android is a trademark of Google LLC. iPhone, iPad, iOS and AirPlay are
trademarks of Apple Inc. 3zT Mirroring is not affiliated with Apple or Google.
