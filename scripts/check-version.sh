#!/bin/sh
# Checks that Cargo.toml, the Android app and (optionally) a tag agree on
# the version. Usage: scripts/check-version.sh [v1.2.3]
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cargo=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
android=$(sed -n "s/^ *versionName '\(.*\)'/\1/p" "$root/android/app/build.gradle" | head -n 1)
echo "Cargo.toml: $cargo, Android: $android${1:+, tag: $1}"
[ "$cargo" = "$android" ] || { echo "error: versions differ" >&2; exit 1; }
if [ $# -gt 0 ] && [ "${1#v}" != "$cargo" ]; then
    echo "error: tag $1 doesn't match version $cargo" >&2
    exit 1
fi
