# DroidMirror

See and control your Android phone from your Mac, in a phone-shaped
window, like iPhone Mirroring.

[![CI](https://github.com/codrixotech/DroidMirror/actions/workflows/ci.yml/badge.svg)](https://github.com/codrixotech/DroidMirror/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

- **Native and light:** a Rust + AppKit app with hardware video decoding.
  About 17 MB of memory while mirroring. No Electron, no FFmpeg.
- **Wi-Fi without a cable:** scan a QR code once; after that DroidMirror
  finds your phone by itself.
- **Full control:** click, swipe, scroll, type; Back, Home and Recent apps
  shortcuts; clipboard sync; drop files onto the phone.
- **Calls:** incoming calls pop up on the Mac with Answer and Decline.
- **No Developer options?** Use the DroidMirror Android app instead.

> **Status: beta.** Tested on a realme phone with Android 15. Please
> [report](https://github.com/codrixotech/DroidMirror/issues) how it works
> on yours.

## Requirements

- A Mac with Apple Silicon, macOS 14 Sonoma or later.
- An Android phone: Android 11+ for Wi-Fi pairing, Android 12+ for the
  companion app (USB works with older versions).
- The phone and the Mac on the same Wi-Fi network (for wireless use).

## Install

1. Download `DroidMirror-<version>-macos-arm64.dmg` from
   [Releases](https://github.com/codrixotech/DroidMirror/releases), open it
   and drag **DroidMirror** to **Applications**.
2. Open DroidMirror. If macOS says it can't verify the developer, open
   **System Settings → Privacy & Security** and click **Open Anyway**
   (only needed for builds that aren't notarized).
3. When macOS asks whether DroidMirror may find devices on your local
   network, click **Allow**.

Then connect your phone in one of two ways:

| | A. Developer options (recommended) | B. DroidMirror Android app |
|---|---|---|
| One-time setup | Turn on Developer options; install adb on the Mac | Install the app; allow control once |
| Each session | Automatic | Tap **Start sharing** on the phone |
| Video | Best (scrcpy) | Good (Android screen recording) |
| Typing | Full keyboard | Into the focused text field |
| Drag on screen | Live | Shown when you release the mouse |
| Clipboard | Both ways | Mac → phone |
| Drop files | Yes | No |
| Calls | Answer / decline | Answer / decline |

DroidMirror picks the method automatically: Developer options when
available, the app otherwise.

### A. With Developer options

1. On the Mac, install adb:

   ```bash
   brew install android-platform-tools
   ```

   (If you use Android Studio, its adb works too. Use only one: two
   different adb versions keep disconnecting each other.)
2. On the phone: **Settings → About phone → Version** (or Software
   information) → tap **Build number** 7 times.
3. **Settings → Developer options** (on realme: Settings → Additional
   settings → Developer options): turn on **Wireless debugging**.
   On realme / OPPO / OnePlus phones, also turn on **Disable permission
   monitoring**, or taps and typing won't work.
4. Open DroidMirror on the Mac. It shows a QR code. On the phone, tap
   **Wireless debugging → Pair device with QR code** and scan it.

That's it: next time, just open DroidMirror. If Android has switched
Wireless debugging off (after a restart or a Wi-Fi change), switch it back
on; there's no need to pair again.

**Over USB instead:** turn on **USB debugging**, plug the phone in and
accept "Allow USB debugging?".

### B. With the DroidMirror Android app

1. Install **DroidMirror** on the phone (the APK from
   [Releases](https://github.com/codrixotech/DroidMirror/releases); Google
   Play listing coming).
2. Open it and follow the three steps: **Allow control** (turns on its
   Accessibility service), **Allow** notifications and calls, then
   **Start sharing** and choose **Entire screen**.
3. Open DroidMirror on the Mac. The first time, both devices show the same
   6-digit code: tap **Allow** on the phone if they match.

After that, the Mac connects whenever sharing is on. Android asks for
screen-sharing permission each time you start sharing.

## Using it

Drag the window by its dark border; resize it from the edges. It follows
the phone's rotation.

| Mac | Phone |
|---|---|
| Click / drag | Tap / swipe |
| Scroll wheel or trackpad | Scroll |
| Typing | Text (any keyboard layout) |
| Return, Delete, arrows, Tab, Esc, Ctrl+key | Key presses |
| Cmd+B or right click | Back |
| Cmd+H | Home |
| Cmd+S | Recent apps |
| Cmd+C | Copy on the phone, into the Mac clipboard |
| Cmd+V | Paste the Mac clipboard on the phone |
| Drop files from Finder | Copied to the phone's Download folder |
| Cmd+Return or **Answer** | Answer an incoming call |
| Cmd+Delete or **Decline** / **End** | Decline or end a call |
| Cmd+Q | Quit |

**Calls** are answered on the phone: the audio stays on the phone (tap the
speaker button on the mirrored screen to talk hands-free). Using the Mac's
microphone and speakers for calls isn't possible.

If the connection drops, DroidMirror says so and reconnects by itself.

## Troubleshooting

- **Video works but taps and typing don't (realme / OPPO / OnePlus):** turn
  on **Disable permission monitoring** in Developer options, then
  reconnect.
- **"Several devices are connected":** run DroidMirror with
  `--serial SERIAL` (see `adb devices`).
- **The Mac doesn't find the phone over Wi-Fi:** check both are on the
  same network, and that DroidMirror is allowed in **System Settings →
  Privacy & Security → Local Network**.
- **Companion app: "Restricted setting" when turning on control:** this
  happens with APKs not installed from Google Play. Open **Settings → Apps →
  DroidMirror → ⋮ → Allow restricted settings**, then try again.
- **Logs for bug reports:** **Help → Show Log File** in DroidMirror.

### Command-line options

Run `/Applications/DroidMirror.app/Contents/MacOS/DroidMirror --help`:

```text
-s, --serial SERIAL      device to use when several are connected
    --tcpip[=IP[:PORT]]  connect over Wi-Fi (switch a USB device, or an address)
    --forward            use `adb forward` instead of `adb reverse`
    --headless           no window: log video packet rate and bitrate only
    --no-call-alerts     don't watch for incoming calls
```

Environment: `DROIDMIRROR_LOG=debug` for verbose logs, `ADB=/path/to/adb`
if adb isn't found.

## Building from source

Mac app (Rust stable, Xcode command line tools):

```bash
./scripts/fetch-server.sh
```

```bash
cargo run --release
```

The first command downloads the pinned scrcpy-server and checks its
SHA-256; the build refuses to run without it.

To build `DroidMirror.app` and a `.dmg` (optionally signed and notarized,
see the script):

```bash
./scripts/package-macos.sh
```

Android app: see [android/README.md](android/README.md).

More: [architecture and protocols](docs/ARCHITECTURE.md),
[contributing](CONTRIBUTING.md), [security](SECURITY.md),
[privacy](PRIVACY.md), [changelog](CHANGELOG.md).

## License

Apache License 2.0, see [LICENSE](LICENSE). The Mac app embeds
[scrcpy-server](https://github.com/Genymobile/scrcpy) (Apache 2.0, Copyright
Genymobile and Romain Vimont), see [NOTICE](NOTICE).

DroidMirror is an independent project, not affiliated with or endorsed by
Genymobile, Google or Apple. Android is a trademark of Google LLC; Mac and
iPhone are trademarks of Apple Inc.
