# Security policy

DroidMirror lets a computer see and control a phone, so security bugs
matter. Thank you for reporting them responsibly.

## Reporting a vulnerability

Please **don't open a public issue**. Use GitHub's private reporting:
the repository's **Security** tab → **Report a vulnerability**.

Include what's affected (Mac app, Android app, which mode), steps to
reproduce, and the impact you see. We aim to reply within 7 days.

## Scope

In scope, for example:

- Pairing or authentication bypass in companion mode (connecting to a
  phone without being approved on it).
- Breaking or downgrading the TLS connection, or certificate pinning.
- Reading screen, input, clipboard or call data from another device on
  the same network.
- Anything letting a website or another app control the phone through
  DroidMirror.

Out of scope: anything requiring an attacker who already controls the
Mac user account or has unlocked physical access to the phone, and
issues in adb or scrcpy-server themselves (report those upstream).

## Design notes

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#security) for how pairing,
TLS and pinning work.
