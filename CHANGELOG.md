# Changelog

## 0.1.0 (unreleased)

First public release.

- Native macOS app (Apple Silicon, macOS 14+) that mirrors and controls an
  Android phone in a phone-shaped window, using the official scrcpy-server
  5.0.1 over adb. Hardware video decoding; about 17 MB of memory.
- Mouse, scroll and keyboard control; Cmd+B / Cmd+H / Cmd+S for Back,
  Home, Recent apps; clipboard sync; drag and drop files to the phone.
- Wi-Fi setup without a cable: QR-code pairing for Wireless debugging and
  automatic discovery.
- Incoming-call alerts with Answer / Decline.
- Companion Android app for phones without Developer options: screen
  sharing, control through an Accessibility service, encrypted and paired
  connection. The Mac says so when the phone's Accessibility service isn't
  running.
- macOS app bundle and .dmg, log file (Help → Show Log File), CI and
  release automation.

Tested on a realme RMX3771 (Android 15) with an Apple Silicon Mac: both
modes, mirroring, control, clipboard and calls.
