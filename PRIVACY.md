# Privacy policy

_Last updated: 14 November 2026_

DroidMirror (the macOS app and the Android app) lets you see and control
your own Android phone from your own Mac.

## What we collect

**Nothing.** DroidMirror has no accounts, no analytics, no advertising and
no servers. We (the developers) never receive your screen, input, calls,
clipboard, files or any other data.

## What stays between your phone and your Mac

To work, the apps exchange data directly between your phone and your Mac,
over a USB cable or your local Wi-Fi network:

- **Screen contents** of the phone, as a video stream to the Mac.
- **Input** from the Mac (taps, swipes, scrolling, typed text) to the phone.
- **Clipboard text**, when you copy or paste between them.
- **Call state** (ringing, in a call, idle) so the Mac can show incoming
  calls, and your choice to answer or end a call.
- **Files** you choose to drag onto the Mac window (Developer options mode).

In companion-app mode this traffic is encrypted with TLS. Each Mac must be
approved on the phone once, after both devices show the same 6-digit code.

## What is stored

- On the phone: a random device id, a TLS key kept in the Android Keystore,
  and a hash of each paired Mac's access token. Excluded from backups and
  device transfers. Removed when you uninstall the app or tap
  "Forget all paired Macs".
- On the Mac: paired phones (id, certificate fingerprint, access token) in
  `~/Library/Application Support/DroidMirror/`, readable only by your user
  account, and a diagnostic log of the last two runs in
  `~/Library/Logs/DroidMirror/`. The log contains connection events and
  errors, never screen contents, typed text or clipboard text.

## Android permissions and why

| Permission | Used for |
|---|---|
| Accessibility service | Performing the taps, swipes and typing you send from your Mac |
| Screen capture (asked each session) | Showing the phone's screen on your Mac |
| Phone state, answer phone calls | Showing incoming calls on your Mac and answering or ending them from there |
| Notifications | Pairing requests and the "sharing" notification |
| Network | Talking to your Mac on your local network |

## Contact

Questions or concerns: open an issue at
https://github.com/codrixotech/DroidMirror/issues
