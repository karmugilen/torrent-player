#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/engine"
DEST="$ROOT/android/app/src/main/assets/nodejs-project"
rm -rf "$DEST"
mkdir -p "$DEST"
cp "$SRC/package.json" "$SRC/main.js" "$SRC/protocol.js" "$SRC/document-store.js" "$DEST/"
if [ ! -d "$SRC/node_modules/webtorrent" ]; then
  echo "engine/node_modules missing; run npm ci --omit=optional --omit=dev in engine/" >&2
  exit 1
fi
node "$SRC/scripts/patch-webtorrent.mjs"
cp -a "$SRC/node_modules" "$DEST/node_modules"
# assets cannot contain files named like this on some packagers
find "$DEST" -name '.bin' -type d -prune -exec rm -rf {} +
# Desktop npm may install x86_64 node-datachannel. Android Node 18 cannot load it.
# v1 has no WebRTC peers — stub the polyfill and drop the native addon.
rm -rf "$DEST/node_modules/node-datachannel"
mkdir -p "$DEST/node_modules/node-datachannel/dist/esm/lib" \
         "$DEST/node_modules/webrtc-polyfill"
cat > "$DEST/node_modules/node-datachannel/package.json" << 'EOF'
{ "name": "node-datachannel", "version": "0.0.0-stub", "type": "module" }
EOF
cat > "$DEST/node_modules/node-datachannel/dist/esm/lib/node-datachannel.mjs" << 'EOF'
export default {}
export const initLogger = () => {}
EOF
cat > "$DEST/node_modules/webrtc-polyfill/index.js" << 'EOF'
export class RTCPeerConnection {}
export class RTCSessionDescription {}
export class RTCIceCandidate {}
export class RTCIceTransport {}
export class RTCDataChannel {}
export class RTCSctpTransport {}
export class RTCDtlsTransport {}
export class RTCCertificate {}
export default {}
EOF
find "$DEST" -name '*.node' -delete
# Drop docs, types, maps, tests, and browser bundles. Runtime JS stays.
find "$DEST" -type f \( \
  -name '*.md' -o -name '*.markdown' -o -name '*.map' -o -name '*.ts' \
  -o -name '*.d.ts' -o -name '*.yml' -o -name '*.yaml' -o -name '*.bc.js' \
  -o -name '.npmignore' -o -name '*.tsbuildinfo' -o -name 'LICENSE*' \
  -o -name 'CHANGELOG*' -o -name 'AUTHORS*' -o -name 'Makefile' \
\) -delete
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
( cd "$SRC" && sha256sum main.js protocol.js document-store.js ) > "$DEST/bundle.rev"
echo "synced engine -> $DEST (native addons stripped)"
