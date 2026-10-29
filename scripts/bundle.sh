#!/bin/sh
# Assembles StatsLite.app around the cargo binary (text-only status item, no icon).
# Usage: scripts/bundle.sh [debug|release]   → prints the .app path
set -eu

profile="${1:-debug}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

case "$profile" in
  debug) cargo build ;;
  release) cargo build --release ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

out="target/$profile"
app="$out/StatsLite.app"
version="$(cargo pkgid | sed 's/.*[#@]//')"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$out/stats-lite" "$app/Contents/MacOS/stats-lite"
sed "s/__VERSION__/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"

# Ad-hoc seal for local runs.
codesign --force --sign - "$app" >/dev/null 2>&1
echo "$app"
