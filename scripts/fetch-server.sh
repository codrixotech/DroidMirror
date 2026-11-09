#!/bin/sh
# Download the pinned scrcpy-server and verify its SHA-256.
# Version and checksum come from scrcpy-server.lock at the repo root.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
lock="$root/scrcpy-server.lock"

version=$(sed -n 's/^version=//p' "$lock")
expected=$(sed -n 's/^sha256=//p' "$lock")
[ -n "$version" ] && [ -n "$expected" ] || { echo "error: cannot parse $lock" >&2; exit 1; }

name="scrcpy-server-v$version"
dest="$root/assets/$name"
url="https://github.com/Genymobile/scrcpy/releases/download/v$version/$name"

sha256() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        sha256sum "$1" | cut -d' ' -f1
    fi
}

if [ -f "$dest" ] && [ "$(sha256 "$dest")" = "$expected" ]; then
    echo "$name already present and verified"
    exit 0
fi

mkdir -p "$root/assets"
tmp=$(mktemp "$root/assets/.download.XXXXXX")
trap 'rm -f "$tmp"' EXIT

echo "downloading $url"
curl --fail --location --silent --show-error --output "$tmp" "$url"

actual=$(sha256 "$tmp")
if [ "$actual" != "$expected" ]; then
    echo "error: checksum mismatch for $name" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    exit 1
fi

chmod 644 "$tmp"
mv "$tmp" "$dest"
trap - EXIT
echo "verified $name ($actual)"
