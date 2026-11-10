# DroidMirror

A lightweight native macOS app that mirrors and controls an Android phone,
in the spirit of iPhone Mirroring. Written in Rust, no webview, no FFmpeg,
no GUI framework: H.264 from the phone goes straight into a
hardware-decoded `AVSampleBufferDisplayLayer` in a plain `NSWindow`.

It reuses the official [scrcpy](https://github.com/Genymobile/scrcpy)
server (Apache 2.0) on the phone. The client is pinned to **scrcpy-server
5.0.1**; see `scrcpy-server.lock`.

Requirements: Apple Silicon Mac, macOS 14 or later, `adb`.

## Mac setup

1. Install Rust: <https://rustup.rs>
2. Install adb (DroidMirror shells out to it):

   ```bash
   brew install android-platform-tools
   ```

3. Fetch and verify the pinned scrcpy-server (the build refuses to run
   without it, or if its SHA-256 doesn't match):

   ```bash
   ./scripts/fetch-server.sh
   ```

4. Build and run:

   ```bash
   cargo run --release
   ```

## Two ways to connect

| | Developer options (adb) | DroidMirror Android app |
|---|---|---|
| Setup on the phone | Developer options once, then Wireless debugging | Install the app, allow control once |
| Each session | Automatic | Tap **Start sharing** (Android asks every time) |
| Video | scrcpy, best quality | Android screen recording |
| Typing | Full keyboard | Into the focused text field |
| Drag on screen | Live | Shown when you release the mouse |
| Clipboard | Both ways | Mac → phone only (Android blocks the other way) |
| Send files | Yes | No |
| Calls | Answer and decline | Answer and decline |

DroidMirror uses adb when a phone is available that way, and the app
otherwise. Nothing to choose on the Mac.

## Phone setup (Realme / Realme UI)

You need Developer options once per phone. No cable is needed.

### Over Wi-Fi (recommended)

1. **Enable Developer options:** Settings → About device (or About phone)
   → Version → tap **Build number** 7 times, entering your PIN if asked.
2. Settings → Additional settings → **Developer options**: turn on
   **Wireless debugging** and **Disable permission monitoring** (Realme UI
   blocks taps and typing from the Mac without it).
3. Start DroidMirror. With no phone connected, it shows a **QR code**.
4. On the phone, tap **Wireless debugging → Pair device with QR code** and
   scan it. DroidMirror pairs, connects and starts mirroring.

The phone and the Mac must be on the same Wi-Fi network. After pairing,
DroidMirror finds the phone by itself every time; if Android has switched
Wireless debugging off (after a restart or a Wi-Fi change), switch it
back on and it reconnects. There's no need to pair again.

### Over USB

Turn on **USB debugging** in Developer options instead, plug the phone in
and accept the "Allow USB debugging?" prompt.

### If adb isn't found

DroidMirror uses `adb` from your `PATH`, Homebrew
(`brew install android-platform-tools`) or Android Studio
(`~/Library/Android/sdk/platform-tools`). Use only one of them: two
different adb versions keep restarting each other and drop the phone.

### With the DroidMirror app (no Developer options)

1. Install **DroidMirror** on the phone (Google Play; or build it from
   `android/`, see `android/README.md`) and open it.
2. **Allow control:** the app explains what it does, then opens
   Settings → Accessibility; turn on **DroidMirror remote control**.
3. **Notifications and calls:** tap **Allow** (for pairing requests and
   call alerts).
4. **Start sharing** and choose **Entire screen** when Android asks.
5. Start DroidMirror on the Mac (same Wi-Fi). The first time, the Mac
   shows a 6-digit code and the phone asks "Pair with <your Mac>?" with the
   same code: tap **Allow** if they match. After that it connects by
   itself whenever sharing is on.

The connection is encrypted with TLS. The phone's key lives in the
Android Keystore, and the Mac pins its certificate after pairing (stored
in `~/Library/Application Support/DroidMirror/companion-pairings`). The
6-digit comparison stops anyone else on the network from pairing or
listening in.

## Using it

The window is shaped like the phone. Drag it by its dark border; resize
it from the edges (the aspect ratio is kept). It follows the phone's
rotation.

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
| Drop files (from Finder) on the window | Copied to `/sdcard/Download` |
| Cmd+Return or **Answer** | Answer an incoming call |
| Cmd+Delete or **Decline** / **End** | Decline or end a call |
| Cmd+Q | Quit (stops the server on the phone) |

The phone's clipboard is copied to the Mac whenever it changes; the Mac
clipboard is sent to the phone when you switch back to DroidMirror.

If the phone is unplugged, the window says so and reconnects by itself
when it comes back.

### Phone calls

When a call comes in, the window comes to the front, the Dock icon
bounces, and a banner shows **Answer** and **Decline**. Answering from the
Mac picks up the call **on the phone**: the audio stays on the phone (tap
the speaker button on the mirrored screen to talk hands-free). Using the
Mac's microphone and speakers for the call isn't possible over adb.

Call state is read with `adb shell dumpsys telephony.registry` once a
second while connected. Recent Android versions hide the caller's number
there, so the banner may just say "Incoming call"; the caller's name is
visible on the mirrored screen. Turn this off with `--no-call-alerts`.

### Options

```text
-s, --serial SERIAL      device to use when several are connected
    --tcpip[=IP[:PORT]]  connect over Wi-Fi (see below)
    --forward            use `adb forward` instead of `adb reverse`
    --headless           no window: log video packet rate and bitrate only
    --no-call-alerts     don't watch for incoming calls
```

Environment: `DROIDMIRROR_LOG=debug` for verbose logs (including video
throughput once per second), `ADB=/path/to/adb` if adb is not found.

### Wireless options

QR pairing (above) covers most cases. Also available:

- `--tcpip` with the phone plugged in by USB switches it to network adb
  (`adb tcpip 5555`), connects to its Wi-Fi address, and then you can
  unplug the cable (useful on Android 10 and older).
- `--tcpip=IP:PORT` connects to a specific address.

## Measuring memory

While mirroring:

```bash
footprint $(pgrep -x droidmirror)
```

The target is under 50 MB for the app process.

## How it works

- `adb push` the embedded server, `adb reverse` a local port (falling back
  to `adb forward`), start it with `app_process`, then open the video and
  control sockets. Server arguments: `video_codec=h264 max_size=1024
  video_bit_rate=3000000 max_fps=30 audio=false control=true`.
- **Video thread:** reads the socket continuously, converts Annex B to
  AVCC, builds a `CMVideoFormatDescription` from SPS/PPS (rebuilt when they
  change, e.g. on rotation), wraps each frame in a `CMSampleBuffer` marked
  *display immediately* and enqueues it on the layer's
  `sampleBufferRenderer`, which Apple documents as safe to feed from a
  background thread. If the renderer falls behind, frames are dropped
  until the next key frame and `RESET_VIDEO` (at most once a second) asks
  the phone for one, so nothing piles up on either side.
- **Main thread:** AppKit and input. Background threads wake it through a
  `CFRunLoopSource` (event-driven, no timers).
- **Control threads:** one writes control messages from a bounded queue,
  one reads device messages (clipboard).
- All unsafe FFI is in `src/macos/`.

## Updating the pinned server

The client protocol is written against one exact server version, and the
server refuses to start when the versions differ. To move to a new scrcpy
release, update `version` and `sha256` in `scrcpy-server.lock` (the
checksum is listed in that release's `doc/build.md`), re-check the
protocol code against the new server source, then rerun
`./scripts/fetch-server.sh`.

## License

DroidMirror is Apache-2.0. scrcpy-server is Copyright Genymobile and
Romain Vimont, Apache-2.0; it is downloaded at build time, not
redistributed in this repository.
