#!/bin/sh
# Builds dist/DroidMirror.app and dist/DroidMirror-<version>-macos-arm64.dmg.
# Run on an Apple Silicon Mac (macOS 14+) with Rust and the Xcode command line tools.
#
# Signing (optional; without it the app is ad-hoc signed, and people opening a
# downloaded copy must allow it in System Settings → Privacy & Security):
#   DEVELOPER_ID="Developer ID Application: Your Name (TEAMID)"
# Notarization (optional, needs DEVELOPER_ID), with an app-specific password:
#   NOTARY_APPLE_ID=you@example.com NOTARY_TEAM_ID=TEAMID NOTARY_PASSWORD=xxxx-xxxx-xxxx-xxxx
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
target=aarch64-apple-darwin
dist="$root/dist"
app="$dist/DroidMirror.app"
dmg="$dist/DroidMirror-$version-macos-arm64.dmg"

echo "==> DroidMirror $version"
./scripts/fetch-server.sh
cargo build --release --target "$target"

echo "==> Assembling $app"
rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$target/release/droidmirror" "$app/Contents/MacOS/DroidMirror"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
cp LICENSE NOTICE "$app/Contents/Resources/"

iconset="$dist/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" packaging/icon-1024.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" packaging/icon-1024.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

echo "==> Signing"
if [ -n "${DEVELOPER_ID:-}" ]; then
    codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$app"
else
    echo "    (ad-hoc: set DEVELOPER_ID to sign for distribution)"
    codesign --force --sign - "$app"
fi
codesign --verify --strict "$app"

echo "==> Creating $dmg"
stage="$dist/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "DroidMirror" -srcfolder "$stage" -ov -format UDZO "$dmg" >/dev/null
rm -rf "$stage"
if [ -n "${DEVELOPER_ID:-}" ]; then
    codesign --force --timestamp --sign "$DEVELOPER_ID" "$dmg"
fi

if [ -n "${DEVELOPER_ID:-}" ] && [ -n "${NOTARY_APPLE_ID:-}" ]; then
    echo "==> Notarizing (takes a few minutes)"
    xcrun notarytool submit "$dmg" --apple-id "$NOTARY_APPLE_ID" --team-id "$NOTARY_TEAM_ID" \
        --password "$NOTARY_PASSWORD" --wait
    xcrun stapler staple "$dmg"
fi

echo "==> Done"
echo "    $app"
echo "    $dmg"
