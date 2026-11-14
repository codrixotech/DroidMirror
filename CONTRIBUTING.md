# Contributing to DroidMirror

Thanks for helping! Bug reports, testing on different phones and pull
requests are all welcome.

## Reporting bugs

Open an issue with the bug template. Please include:

- Mac model and macOS version; phone model and Android version.
- Mode: Developer options (adb, USB or Wi-Fi) or the companion app.
- The log: in DroidMirror, **Help → Show Log File** (or run from a
  terminal with `DROIDMIRROR_LOG=debug`). For the Android app,
  `adb logcat -s DroidMirror` if you can.

## Building

Mac app (Apple Silicon, macOS 14+, Rust stable):

```bash
./scripts/fetch-server.sh
```

```bash
cargo run --release
```

Android app: open `android/` in Android Studio, or see
[android/README.md](android/README.md).

## Before sending a pull request

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

```bash
cd android && ./gradlew testDebugUnitTest lintDebug
```

Project rules:

- **Small and light.** Memory use while mirroring is a feature (about
  17 MB today). No webviews, GUI frameworks or FFmpeg.
- **Dependencies:** the Mac app only uses `objc2` framework crates,
  `anyhow`, `thiserror` and `log`; the Android app uses no libraries.
  Please open an issue before proposing a new one.
- **All `unsafe` code lives in `src/macos/`**, behind small safe types.
- **Protocol changes** (scrcpy or companion) need unit tests on both
  sides; see the shared test vectors in `src/companion/tests.rs` and
  `android/app/src/test/`.
- **scrcpy-server is pinned** in `scrcpy-server.lock`. Bumping it means
  re-checking the protocol code against the new server source.

By contributing you agree that your contributions are licensed under the
Apache License 2.0.
