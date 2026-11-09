# DroidMirror

A lightweight native macOS app that mirrors and controls an Android phone,
in the spirit of iPhone Mirroring. Written in Rust, no webview, no FFmpeg:
H.264 from the phone goes straight into a hardware-decoded
`AVSampleBufferDisplayLayer`.

It reuses the official [scrcpy](https://github.com/Genymobile/scrcpy)
server (Apache 2.0) on the phone. The client is pinned to **scrcpy-server
5.0.1**; see `scrcpy-server.lock`.

Requirements: Apple Silicon Mac, macOS 14 or later.

> Status: milestone 2. DroidMirror connects to the phone, starts the
> server and logs the video packet rate and bitrate in the terminal. There
> is no window yet.

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

   Options: `--serial SERIAL` picks a device when several are connected,
   `--forward` uses `adb forward` instead of `adb reverse`. Set
   `DROIDMIRROR_LOG=debug` for verbose logs, and `ADB=/path/to/adb` if adb
   is not on your `PATH`.

## Phone setup (Realme / Realme UI)

1. **Enable Developer options:** Settings → About device (or About phone)
   → Version → tap **Build number** 7 times, entering your PIN if asked.
2. **Enable USB debugging:** Settings → Additional settings → Developer
   options → turn on **USB debugging**.
3. **Connect the phone by USB.** Unlock it and accept the "Allow USB
   debugging?" prompt (tick "Always allow from this computer").
4. **Check the connection:**

   ```bash
   adb devices
   ```

   It should list your device as `device`. `unauthorized` means the prompt
   on the phone was not accepted; unplug, replug and accept it.
5. **If the video shows but touches and keys do nothing:** in Developer
   options, turn on **Disable permission monitoring**, then unplug and
   replug the phone. Realme UI blocks injected input without it.

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
