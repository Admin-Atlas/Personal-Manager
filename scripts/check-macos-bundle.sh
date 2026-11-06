# SPDX-FileCopyrightText: 2026 Bobby Yu
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# The Mac build is Apple Silicon only, for macOS 14 or newer: onnxruntime, which runs PM's
# search and photo models, publishes no Intel Mac or pre-14 Mac wheel, so on those Macs the
# document engine can't be set up. Shared verbatim by release.yml and macos-bundle-dryrun.yml
# so the two can't drift. It reads the built .app and fails unless its executable holds the
# arm64 slice alone and its Info.plist states the minimum macOS that tauri.conf.json sets.
# Without it, a universal or Intel binary, or a minimum that never reached the bundle, would
# build and package green and ship.
#
# Usage: bash scripts/check-macos-bundle.sh <bundle-dir>
#   <bundle-dir> = the tauri bundle output dir,
#   e.g. src-tauri/target/aarch64-apple-darwin/release/bundle.
# macOS only (lipo and plutil come with the Xcode command line tools).

set -euo pipefail

bundle="${1:?usage: check-macos-bundle.sh <bundle-dir>}"
conf="$(dirname "$0")/../src-tauri/tauri.conf.json"

app="$(find "$bundle/macos" -maxdepth 1 -name '*.app' -type d -print -quit)"
if [ -z "$app" ]; then
  echo "✗ no .app under $bundle/macos" >&2
  exit 1
fi
plist="$app/Contents/Info.plist"
exe="$app/Contents/MacOS/$(plutil -extract CFBundleExecutable raw "$plist")"

archs="$(lipo -archs "$exe")"
if [ "$archs" != "arm64" ]; then
  echo "✗ $exe holds [$archs]; the Mac build must be arm64 alone" >&2
  exit 1
fi

want="$(node -e 'const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")); process.stdout.write(String(c?.bundle?.macOS?.minimumSystemVersion ?? ""))' "$conf")"
got="$(plutil -extract LSMinimumSystemVersion raw "$plist")"
if [ -z "$want" ] || [ "$got" != "$want" ]; then
  echo "✗ Info.plist's LSMinimumSystemVersion is [$got]; tauri.conf.json sets [$want]" >&2
  exit 1
fi

echo "✓ $(basename "$app"): arm64 only, macOS $got or newer"
