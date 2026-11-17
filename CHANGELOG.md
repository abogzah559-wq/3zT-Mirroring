# Changelog

## 2.1.0

- **Arabic typing on iPhone / iPad**: the complete Apple Arabic hardware
  layout (letters, hamza forms, punctuation and digits), or Arabic – PC. The
  device keyboard switches between English and Arabic as you type; the AR /
  EN mark and `Alt + G` show and change the expected language, and
  **Test the keyboard** checks the layout in one step. Fast typing and
  `Alt + V` paste no longer drop spaces or line breaks.
- **Auto unlock** (`Alt + U`): types the saved passcode only when the device
  is actually locked, over the cable and Wi-Fi, and says when it fails.
- **iPhone frame**: twelve of Apple's own finishes or your own colour, kept
  per iPhone, with the notch or Dynamic Island, front camera and earpiece of
  the real model. Zoom (`Ctrl + =` / `-`), move (`Ctrl + arrows`), actual
  size (`Ctrl + 1`) and fit (`Ctrl + 9`). Full screen with the frame shows
  nothing around it.
- **Files with iPhone / iPad**: send files and folders to an app, import
  photos, videos and app files to the PC, over the cable or Wi-Fi.
- **Screenshots** (`Alt + P`) at the stream's own size, with or without the
  frame, also copied to the clipboard; **screen recording** (`Alt + O`) to
  MP4 with sound, without re-encoding.
- **Colours**: iPhone streams are shown in their full range, no longer with
  boosted contrast and colours.
- **Picture on iOS 27**: Settings › iPhone picture can take the picture over
  AirPlay (up to 60 fps; pick 3zT Mirroring in Screen Mirroring on the
  device). The default stays Apple's own channel, which connects by itself.
- The stats line shows the received and shown frame rate and the decoder.
- Clearer Arabic interface font (IBM Plex Sans Arabic) and right-to-left
  settings rows.
- Exact model names for iPhone X to iPhone 12.

## 2.0.2

- Updates no longer stop at "Closing applications": Setup now closes
  3zT Mirroring, its service and its adb server before copying files.
- A failed or cancelled update leaves the installed version working; files
  the new version no longer ships are removed only after it is in place.

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
