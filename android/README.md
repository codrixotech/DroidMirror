# DroidMirror for Android

Companion app that lets the DroidMirror Mac app mirror and control the
phone **without Developer options**.

- Screen: Android's screen recording (MediaProjection) → H.264
  (MediaCodec), sent in the same stream format scrcpy uses, so the Mac
  reuses its display pipeline.
- Input: an Accessibility Service performs taps and swipes (gestures),
  scrolling, Back / Home / Recent apps, and typing into the focused field.
- Calls: `TelephonyCallback` for call state, `TelecomManager` to answer
  and end calls.
- Network: TLS server with a key generated in the Android Keystore,
  advertised as `_droidmirror._tcp` with network service discovery.
  Pairing uses a 6-digit code shown on both devices; Macs then use a
  per-Mac token (only its hash is stored on the phone).

Java, no third-party libraries. Requires Android 12 or later.

## Build

With Android Studio: open this `android/` folder and press Run.

From a terminal (needs the Android SDK; `ANDROID_HOME` or a
`local.properties` with `sdk.dir=...`):

```bash
./gradlew testDebugUnitTest lintDebug assembleDebug
```

The APK is `app/build/outputs/apk/debug/app-debug.apk`. Install it with
`adb install`, or copy it to the phone and open it.

## Release signing

Create an upload key once and keep it (and its passwords) safe and backed
up; every Play update must be signed with it:

```bash
keytool -genkeypair -v -keystore ~/droidmirror-release.jks -keyalg RSA -keysize 4096 -validity 10000 -alias droidmirror
```

Then either copy `keystore.properties.example` to `keystore.properties`
(git-ignored) and fill it in, or set `ANDROID_KEYSTORE_FILE`,
`ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` and `ANDROID_KEY_PASSWORD`.
`./gradlew assembleRelease bundleRelease` then produces a signed APK and
AAB. For GitHub releases, add the same values as repository secrets (the
keystore base64-encoded as `ANDROID_KEYSTORE_BASE64`); see
`.github/workflows/release.yml`.

## Publishing on Google Play

`./gradlew bundleRelease` builds `app/build/outputs/bundle/release/app-release.aab`.
Sign it with your upload key (Android Studio → Build → Generate Signed
Bundle) and enable Play App Signing. Then, in Play Console:

1. **Accessibility API declaration** (Policy → App content): DroidMirror
   is *not* an accessibility tool (`isAccessibilityTool="false"`). Declare
   the use as remote control of the user's own device from their own
   paired computer, initiated by the user. The in-app disclosure dialog
   (shown before opening Accessibility settings) and the service
   description cover the required prominent disclosure. Google also asks
   for a short video showing the disclosure and the feature.
2. **Foreground service type `mediaProjection`**: describe it as "user-
   initiated screen sharing to the user's paired Mac" and provide a video.
3. **Phone permissions** (`READ_PHONE_STATE`, `ANSWER_PHONE_CALLS`): used
   only to show incoming calls on the paired Mac and answer or end them
   from there.
4. **Data safety**: no data collected or shared with third parties; data
   is sent only to the user's own paired Mac on the local network,
   encrypted in transit.
5. **Privacy policy** URL: use
   `https://github.com/codrixotech/DroidMirror/blob/main/PRIVACY.md`
   (the app links to it too).
6. **Store listing** text is in `metadata/en-US/` (title, short and full
   description, in the layout fastlane `supply` uses). Screenshots still
   need to be taken on a real phone.

Review of Accessibility-based remote control apps can take longer and
may need several rounds; ship the Mac app's adb mode independently.
