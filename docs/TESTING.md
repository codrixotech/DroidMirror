# Release test checklist

Unit tests cover the protocols, H.264 handling, input mapping and pairing,
but not real video, input injection or phone permissions. Run through this
on a real Mac and phone before tagging a release, and note the phone model
and Android version in the release notes.

Build the release packages first:

```bash
./scripts/package-macos.sh
```

```bash
cd android && ./gradlew assembleRelease
```

Install the `.dmg` like a user would (drag to Applications, open from
there) rather than running `cargo run`.

## Mac app basics

- [ ] Opens from Applications; Dock icon and "DroidMirror" menu bar name.
- [ ] macOS asks for Local Network access on first launch.
- [ ] With no phone, the setup screen shows instructions and a QR code.
- [ ] **Help → Show Log File** reveals the log in Finder.

## A. Developer options mode

- [ ] **QR pairing:** forget the Mac under Wireless debugging → Paired
      devices, run `adb disconnect`, then pair by scanning the QR code.
- [ ] Reconnects by itself after quitting and reopening, and after turning
      Wireless debugging off and on (new port).
- [ ] USB with USB debugging works too.
- [ ] Video: correct aspect ratio; window turns when the phone rotates.
- [ ] Click taps, drag swipes live, wheel and trackpad scroll.
- [ ] Typing, Return, Delete, arrows; Cmd+B / Cmd+H / Cmd+S.
- [ ] Clipboard: copy on the phone → paste on the Mac; Cmd+V on a phone field.
- [ ] Drop a file from Finder: "Saved to Download", file on the phone.
- [ ] Incoming call: window comes forward, banner, Answer, End, Decline,
      Cmd+Return, Cmd+Delete.
- [ ] Unplug or turn Wi-Fi off: "Disconnected", then reconnects.
- [ ] Cmd+Q: no `com.genymobile.scrcpy` process left on the phone
      (`adb shell "ps -A -o ARGS | grep com.genymobile.scrcpy | grep -v grep"`).
- [ ] Memory while mirroring: `footprint $(pgrep -x DroidMirror)`, target
      under 50 MB (about 17 MB measured).

## B. Companion app mode

Turn Wireless debugging and USB debugging off, so adb can't reach the phone.

- [ ] App setup: Allow control (disclosure dialog, then Accessibility
      settings), permissions, Start sharing → Entire screen.
- [ ] App says "Visible to Macs on this Wi-Fi".
- [ ] First connection: same 6-digit code on both; Allow → mirroring.
- [ ] Deny once: no connection; the Mac doesn't ask again for a minute.
- [ ] Video and rotation.
- [ ] Taps, swipes (shown on release), scrolling.
- [ ] Typing into a text field, Return, Delete, arrows, Ctrl+A/C/X/V.
- [ ] Back / Home / Recent apps shortcuts.
- [ ] Cmd+V pastes the Mac clipboard on the phone.
- [ ] Incoming call: banner, Answer, End, Decline.
- [ ] With "DroidMirror remote control" switched off: the Mac shows that
      the phone can't be controlled yet.
- [ ] Quit and reopen the Mac app: reconnects without a code.
- [ ] Stop sharing on the phone: the Mac shows it's disconnected; Start
      sharing again → reconnects.
- [ ] "Forget all paired Macs": the next connection asks for a code again.

## Phones tested

Add a line per release:

| Version | Phone | Android | Mode A | Mode B | Notes |
|---|---|---|---|---|---|
| 0.1.0 | realme RMX3771 | 15 | ✓ | ✓ | Needed "Disable permission monitoring" (A); Accessibility service had to be switched on (B) |
