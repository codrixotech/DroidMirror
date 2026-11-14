# Architecture

DroidMirror has two parts:

- **The Mac app** (Rust, `src/`): a native AppKit window that shows the
  phone's screen and sends input. No webview, GUI framework or FFmpeg.
- **The phone side**, one of:
  - **scrcpy-server** (adb mode): the official server from
    [scrcpy](https://github.com/Genymobile/scrcpy), pinned to 5.0.1 and
    pushed to the phone over adb. Needs Developer options.
  - **The DroidMirror Android app** (companion mode, `android/`): for phones
    without Developer options.

Both phone sides send the same video stream format, so the Mac's display
pipeline is shared.

## Mac app

```
src/
  main.rs            arguments, headless mode, app entry
  session.rs         connect / stream / reconnect loop (video thread)
  adb.rs             adb wrapper: devices, push, tunnels, pairing, mDNS
  server.rs          start scrcpy-server, socket handshake
  pairing.rs         Wireless debugging QR pairing and discovery
  companion/         companion-app protocol, pairing store
  video.rs           packets → H.264 parameter sets / AVCC frames → sink
  h264.rs            Annex B parsing and AVCC conversion
  controller.rs      control writer / device-message reader threads
  call.rs            call state (adb mode)
  input.rs           key, mouse and window geometry mapping
  protocol/          scrcpy wire format (video, control, device messages)
  sha256.rs          fingerprints and pairing codes
  macos/             all unsafe FFI: app, window/view, display layer,
                     main-thread queue, TLS, Bonjour, clipboard, QR code
```

### Threads

- **Main thread:** AppKit and input. Background threads wake it through a
  `CFRunLoopSource` (event-driven, no timers).
- **Video thread:** owns the connection lifecycle. It reads the video
  socket continuously, converts Annex B to AVCC, builds a
  `CMVideoFormatDescription` from SPS/PPS (rebuilt when they change, e.g.
  on rotation), wraps each frame in a `CMSampleBuffer` marked *display
  immediately* and enqueues it on the layer's `sampleBufferRenderer`
  (documented as safe from a background thread). If the renderer falls
  behind, frames are dropped until the next key frame and `RESET_VIDEO`
  (at most once a second) asks for one, so nothing piles up.
- **Control threads:** one writes control messages from a bounded queue
  (the UI never blocks), one reads device messages (clipboard, calls).

### adb mode

`adb push` the embedded server, `adb reverse` a local port (falling back to
`adb forward`), start it with `app_process`, then open the video and
control sockets. Server arguments: `video_codec=h264 max_size=1024
video_bit_rate=3000000 max_fps=30 audio=false control=true`.

Every byte layout and server argument was checked against the scrcpy
5.0.1 source; the unit tests reuse scrcpy's own test vectors.

**Updating the pinned server:** update `version` and `sha256` in
`scrcpy-server.lock` (the checksum is in that release's `doc/build.md`),
re-check the protocol code against the new server source, then run
`./scripts/fetch-server.sh`. The server refuses to start if the versions
differ.

## Companion mode

The Android app runs a TLS server and advertises `_droidmirror._tcp` with
network service discovery (TXT record `id` = a random, stable device id).

Each connection starts, after the TLS handshake, with a hello from the Mac:

```
"DMC1" | channel u8 | payload
  PAIR    (3): nonce[16] | name_len u8 | name   → status | token[32] | id_len u8 | id
  VIDEO   (0): token[32]                        → status | 64-byte device name, then
                                                  the scrcpy video stream format
  CONTROL (1): token[32]                        → status | then scrcpy control messages
  EVENTS  (2): token[32]                        → status | then device messages
status: 0 ok, 1 denied, 2 unknown token, 3 not sharing
```

Device messages are scrcpy's, plus two extensions:

- `100 | state u8 (0 idle, 1 ringing, 2 in call) | len u32 | number`: call state.
- `101`: input can't be performed (the Accessibility service isn't
  running); the Mac tells the user how to turn it on.

On the phone, video comes from MediaProjection and MediaCodec (H.264,
1024 px max, 3 Mbit/s, 30 fps). Input goes through an Accessibility
service: gestures for taps and swipes (replayed whole when the mouse is
released), short swipes for scrolling, global actions for Back / Home /
Recents, and text edits on the focused field. Calls use
`TelephonyCallback` and `TelecomManager`.

## Security

- **adb mode** relies on adb's own authorization (the phone's "Allow USB
  debugging?" prompt, or Wireless debugging pairing). QR pairing uses
  Android's standard `WIFI:T:ADB;S:<name>;P:<password>;;` format with a
  random one-time password.
- **Companion mode:**
  - The phone's TLS key is an EC P-256 key generated in the Android
    Keystore; it never leaves the phone.
  - **Pairing** works like Bluetooth numeric comparison: both devices show
    `SHA-256(certificate fingerprint ‖ Mac nonce)` as 6 digits, and the user
    allows the Mac on the phone only if they match. A man in the middle
    would present a different certificate or nonce and the codes would
    differ.
  - After pairing the Mac **pins** the certificate fingerprint and keeps a
    random 32-byte token issued by the phone; the phone stores only the
    token's SHA-256. Every channel presents the token inside the pinned TLS
    connection.
  - The Mac uses Apple's SecureTransport (TLS 1.2), which is deprecated but
    supported and needs no Objective-C blocks; moving to Network.framework
    is a possible future change.
- Nothing is sent anywhere except between the user's own phone and Mac.
  See [PRIVACY.md](../PRIVACY.md).
