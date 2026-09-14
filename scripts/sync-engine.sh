#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/engine"
FINAL_DEST="$ROOT/android/app/src/main/assets/nodejs-project"
if [ ! -d "$SRC/node_modules/webtorrent" ]; then
  echo "engine/node_modules missing; run npm ci --omit=optional --omit=dev in engine/" >&2
  exit 1
fi
mkdir -p "$ROOT/android/app/src/main/assets"
DEST="$(mktemp -d "$ROOT/android/app/src/main/assets/.nodejs-project.XXXXXXXX")"
trap 'rm -rf -- "$DEST"' EXIT
cp "$SRC/package.json" "$SRC/main.js" "$SRC/protocol.js" "$SRC/document-store.js" "$DEST/"
node "$SRC/scripts/patch-webtorrent.mjs"
node "$SRC/scripts/patch-native-addons.mjs"
cp -a "$SRC/node_modules" "$DEST/node_modules"
# assets cannot contain files named like this on some packagers
find "$DEST" -name '.bin' -type d -prune -exec rm -rf {} +
# Desktop npm may install host .node binaries. Android loads the jniLibs copies.
find "$DEST" -name '*.node' -delete
rm -rf "$DEST/node_modules/utp-native/deps" \
       "$DEST/node_modules/node-datachannel/src" \
       "$DEST/node_modules/node-datachannel/build"
rm -f "$DEST/node_modules/utp-native/binding.cc" \
      "$DEST/node_modules/utp-native/binding.c" \
      "$DEST/node_modules/node-datachannel/CMakeLists.txt"
# Drop docs, types, maps, tests, and browser bundles. Runtime JS stays.
find "$DEST" -type f \( \
  -name '*.md' -o -name '*.markdown' -o -name '*.map' -o -name '*.ts' \
  -o -name '*.d.ts' -o -name '*.yml' -o -name '*.yaml' -o -name '*.bc.js' \
  -o -name '.*ignore' -o -name '.*rc' -o -name '*.editorconfig' \
  -o -name '*.clang-format' -o -name '*.iml' -o -name '*.tsbuildinfo' \
  -o -name '*.h' -o -name '*.hpp' -o -name '*.c' -o -name '*.cc' -o -name '*.cpp' \
  -o -name '*.gyp' -o -name '*.gypi' -o -name 'binding.gyp' \
  -o -name 'CHANGELOG*' -o -name 'AUTHORS*' -o -name 'Makefile*' -o -name '.gitmodules' \
\) -delete
find "$DEST/node_modules" -type f -name '*LICENSE*' ! -name '*.js' ! -name '*.json' -delete
find "$DEST" -type d \( \
  -name test -o -name tests -o -name docs -o -name example -o -name examples \
  -o -name spec -o -name .github \
\) -prune -exec rm -rf {} +
rm -rf "$DEST/node_modules/webtorrent/dist"
rm -f "$DEST/node_modules/xml2js/lib/xml2js.bc.js"
find "$DEST/node_modules/web-streams-polyfill/dist" -type f ! -name 'polyfill.js' ! -name 'polyfill.mjs' -delete 2>/dev/null || true
find "$DEST" -type d -name prebuilds -prune -exec rm -rf {} +
find "$DEST" -name '*.bare' -delete
if ! grep -q "parts\[0\] === 'configure'" "$DEST/main.js" || ! grep -q "parts\[0\] === 'metadata'" "$DEST/main.js"; then
  echo "synced engine is missing /configure or /metadata; refusing to package a stale daemon" >&2
  exit 1
fi
# Dependency and patch changes must invalidate the device's installed bundle too.
( cd "$SRC" && sha256sum main.js protocol.js document-store.js package.json package-lock.json scripts/patch-webtorrent.mjs scripts/patch-native-addons.mjs ) > "$DEST/bundle.rev"
sha256sum "$ROOT/scripts/sync-engine.sh" | cut -d ' ' -f1 >> "$DEST/bundle.rev"
rm -rf -- "$FINAL_DEST"
mv -- "$DEST" "$FINAL_DEST"
trap - EXIT
echo "synced engine -> $FINAL_DEST (host .node binaries stripped; Android jniLibs provide uTP/WebRTC)"
