#!/bin/sh
# Packs desktop/build/torrentplayer and desktop/build/engine-host into
# <outdir>/TorrentPlayer-<version>-linux-x86_64.AppImage with linuxdeploy and
# linuxdeploy-plugin-qt (downloaded to $LINUXDEPLOY_DIR, default /tmp/linuxdeploy).
#
#   desktop/packaging/linux/appimage.sh 2.6.0 dist
#
# Layout inside the AppImage (the app looks for these relative to its binary):
#   usr/bin/torrentplayer, usr/lib/torrentplayer/engine-host,
#   usr/share/torrentplayer/geoip-country-*.mmdb
# Needs rsvg-convert (librsvg). QMAKE picks the Qt to bundle (default: qmake6, else qmake, on PATH).
set -eu

VERSION="${1:?usage: appimage.sh <version> <outdir>}"
OUT="${2:?usage: appimage.sh <version> <outdir>}"
ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/../../.." && pwd)"
BUILD="$ROOT/desktop/build"
PKG="$ROOT/desktop/pkgbuild"
TOOLS="${LINUXDEPLOY_DIR:-/tmp/linuxdeploy}"
NAME="TorrentPlayer-$VERSION-linux-x86_64.AppImage"

for f in "$BUILD/torrentplayer" "$BUILD/engine-host"; do
  [ -x "$f" ] || { echo "appimage: $f is missing; build the app and engine first" >&2; exit 1; }
done

if [ -z "${QMAKE:-}" ]; then
  QMAKE="$(command -v qmake6 || command -v qmake || true)"
fi
[ -n "$QMAKE" ] || { echo "appimage: no qmake6 or qmake on PATH" >&2; exit 1; }
export QMAKE

mkdir -p "$TOOLS"
base=https://github.com/linuxdeploy
for tool in linuxdeploy/linuxdeploy-x86_64.AppImage linuxdeploy-plugin-qt/linuxdeploy-plugin-qt-x86_64.AppImage; do
  file="$TOOLS/${tool#*/}"
  if [ ! -x "$file" ]; then
    curl -fsSL -o "$file.part" "$base/${tool%%/*}/releases/download/continuous/${tool#*/}"
    chmod +x "$file.part"
    mv "$file.part" "$file"
  fi
done

# Scratch (AppDir, icons, the tools' own extraction) goes to $TMPDIR, about 1 GB.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/torrentplayer-appimage.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
export TMPDIR="$WORK"
APPDIR="$WORK/AppDir"
mkdir -p "$APPDIR/usr/lib/torrentplayer" "$APPDIR/usr/share/torrentplayer"
install -m755 "$BUILD/engine-host" "$APPDIR/usr/lib/torrentplayer/engine-host"
install -m644 "$ROOT"/android/app/src/main/assets/geoip-country-*.mmdb "$APPDIR/usr/share/torrentplayer/"
install -Dm644 "$ROOT/desktop/LICENSE" "$APPDIR/usr/share/licenses/torrentplayer/LICENSE"
# appimagetool makes .DirIcon from a PNG in the AppDir root; render one from the SVG.
mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"
rsvg-convert -w 256 -h 256 -o "$APPDIR/usr/share/icons/hicolor/256x256/apps/torrentplayer.png" "$PKG/torrentplayer.svg"
cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/torrentplayer.png" "$APPDIR/torrentplayer.png"

# Wayland (when this Qt has it) and offscreen (headless checks) next to xcb.
plugins="$("$QMAKE" -query QT_INSTALL_PLUGINS)/platforms"
extra=""
for p in libqwayland-egl.so libqwayland-generic.so libqoffscreen.so; do
  [ -f "$plugins/$p" ] && extra="${extra:+$extra;}$p"
done
export EXTRA_PLATFORM_PLUGINS="$extra"

export QML_SOURCES_PATHS="$ROOT/desktop/src"
export LDAI_OUTPUT="$NAME"
export LINUXDEPLOY_OUTPUT_VERSION="$VERSION"
# The tools are AppImages themselves; extract-and-run avoids needing FUSE.
export APPIMAGE_EXTRACT_AND_RUN=1
export PATH="$TOOLS:$PATH"

mkdir -p "$OUT"
OUT="$(CDPATH='' cd -- "$OUT" && pwd)"
cd "$OUT"
rm -f "$NAME"
"$TOOLS/linuxdeploy-x86_64.AppImage" --appdir "$APPDIR" \
  -e "$BUILD/torrentplayer" \
  -d "$PKG/torrentplayer.desktop" \
  -i "$PKG/torrentplayer.svg" \
  --custom-apprun "$ROOT/desktop/packaging/linux/AppRun" \
  --plugin qt --output appimage
[ -s "$OUT/$NAME" ] || { echo "appimage: linuxdeploy did not write $NAME" >&2; exit 1; }
echo "Built $OUT/$NAME"
