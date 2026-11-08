# Changelog

## 2.0.1

- Built-in HEVC decoder (FFmpeg, GPU-accelerated through D3D11 with a CPU
  fallback): iPhone and iPad on iOS 27 show their picture on PCs without the
  HEVC Video Extensions, and Android H.265 works there too. It also takes
  over when Windows' own HEVC decoder opens but decodes nothing, and handles
  10-bit streams.
- Interface language setting: Settings › Appearance › Language.
- A stopped Apple Mobile Device Service is started for you after one Windows
  prompt; the installer starts it right after installing it and asks for a
  restart only when it could not.
- Clear message when mouse and keyboard control needs a newer iOS (iOS 27 or
  later) and when Windows can't see the iPhone/iPad at all.
- The "Buy me a coffee" button is evenly padded.
- Consistent mode names in error messages (Studio mode, Connection & driver).

## 2.0.0

- New name and look: 3zT Mirroring, with a soft-black theme, macOS-style
  window controls and a "Buy me a coffee" button.
- Clean view: while mirroring, the window is only the device screen. F2 shows
  or hides the controls around it; Alt + drag moves the window.
- Arabic typing on iPhone and iPad (Arabic – PC hardware layout), with
  automatic English / Arabic switching and Alt+G to switch by hand.
- Control of iPhone and iPad from the PC (mouse and keyboard), over the cable
  or Wi-Fi after the first cable connection.
- One-step dark installer: everything is bundled; Apple Mobile Device Support
  is downloaded from Apple and installed when missing; no Bonjour needed.
- Signed in-app updates: a small "Update" button appears when a new version
  is out and installs it after checking its SHA-256 and signature.
- More reliable Wi-Fi connections: an iPhone that reconnects is no longer cut
  off by its own old session, and a session that went silent is closed after
  a minute instead of blocking the next attempt.
- The local service only answers the 3zT Mirroring window.
