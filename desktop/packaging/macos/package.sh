#!/usr/bin/env bash
# Builds the macOS app bundle and dmg:
#   <outdir>/TorrentPlayer-<version>-macos-<arch>.dmg   (arch: arm64 or x64)
#
# Run from anywhere after building the engine:
#   cargo build --release --locked --manifest-path engine-rs/Cargo.toml -p wt-engine --bin engine-host
#   desktop/packaging/macos/package.sh 2.6.0 arm64 dist
# Needs Qt 6 (qmake, macdeployqt on PATH) and Homebrew mpv, dylibbundler, librsvg.
# The bundle is ad-hoc signed only (no Apple Developer ID), so Gatekeeper asks once.
set -euo pipefail

VERSION="${1:?usage: package.sh <version> <arm64|x64> <outdir>}"
ARCH="${2:?usage: package.sh <version> <arm64|x64> <outdir>}"
OUT="${3:?usage: package.sh <version> <arm64|x64> <outdir>}"
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BREW="$(brew --prefix)"
MPV="$(brew --prefix mpv)"
BUILD="$ROOT/desktop/build/macos"
DMG_NAME="TorrentPlayer-$VERSION-macos-$ARCH.dmg"

rm -rf "$BUILD"
mkdir -p "$BUILD" "$OUT"
OUT="$(cd "$OUT" && pwd)"

# ---- the Qt app ------------------------------------------------------------------
printf '#define APP_VERSION "%s"\n' "$VERSION" >"$BUILD/app_version.h"
(
  cd "$BUILD"
  qmake "$ROOT/desktop/torrentplayer.pro" "MPV_PREFIX=$MPV" "INCLUDEPATH+=$BUILD"
  make -j"$(sysctl -n hw.ncpu)"
)
APP="$BUILD/Torrent Player.app"
mv "$BUILD/torrentplayer.app" "$APP"
C="$APP/Contents"

# ---- engine, GeoIP, icon, Info.plist ------------------------------------------------
install -m755 "$ROOT/engine-rs/target/release/engine-host" "$C/MacOS/engine-host"
mkdir -p "$C/Resources"
cp "$ROOT"/android/app/src/main/assets/geoip-country-*.mmdb "$C/Resources/"
cp "$ROOT/desktop/LICENSE" "$C/Resources/LICENSE"

ICONSET="$BUILD/torrentplayer.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  rsvg-convert -w $s -h $s -o "$ICONSET/icon_${s}x${s}.png" "$ROOT/desktop/pkgbuild/torrentplayer.svg"
  rsvg-convert -w $((s * 2)) -h $((s * 2)) -o "$ICONSET/icon_${s}x${s}@2x.png" "$ROOT/desktop/pkgbuild/torrentplayer.svg"
done
iconutil -c icns -o "$C/Resources/torrentplayer.icns" "$ICONSET"

PLIST="$C/Info.plist"
pb() { /usr/libexec/PlistBuddy -c "$1" "$PLIST"; }
pbset() { pb "Set :$1 $2" 2>/dev/null || pb "Add :$1 string $2"; }
pbset CFBundleName "Torrent Player"
pbset CFBundleDisplayName "Torrent Player"
pbset CFBundleIdentifier app.torrentplayer.desktop
pbset CFBundleShortVersionString "$VERSION"
pbset CFBundleVersion "$VERSION"
pbset CFBundleIconFile torrentplayer.icns
pbset NSHumanReadableCopyright "MIT (app), MPL-2.0 (engine)"
pb "Delete :NSHighResolutionCapable" 2>/dev/null || true
pb "Add :NSHighResolutionCapable bool true"

# ---- Qt frameworks, QML, plugins, then libmpv and its Homebrew dependencies ---------
macdeployqt "$APP" -qmldir="$ROOT/desktop/src" -verbose=1

# macdeployqt always ships Qt SQL drivers. They link optional DB libs that are
# not on the runner (libiodbc, libpq, libmimerapi). dylibbundler then prompts
# for those paths; with no TTY it loops on "does not exist. Try again" until
# the job is killed (~90 min). We do not use Qt SQL — drop the drivers.
rm -rf "$C/PlugIns/sqldrivers"

# macdeployqt copies most non-Qt dylibs; whatever still points into Homebrew is
# bundled here with dylibbundler.
brew_refs() {
  find "$C/MacOS" "$C/Frameworks" "$C/PlugIns" -type f \( -perm -111 -o -name '*.dylib' \) -print0 2>/dev/null |
    while IFS= read -r -d '' f; do
      file -b "$f" | grep -q Mach-O || continue
      if otool -L "$f" | tail -n +2 | awk '{print $1}' | grep -qE "^($BREW|/opt/homebrew|/usr/local)/"; then
        printf '%s\n' "$f"
      fi
    done
}
# (bash 3.2 on macOS has no mapfile)
todo=()
while IFS= read -r f; do todo+=("$f"); done < <(brew_refs)
if [ "${#todo[@]}" -gt 0 ]; then
  args=()
  for f in "${todo[@]}"; do args+=(-x "$f"); done
  # Feed "quit" so a missing optional dylib fails the step instead of hanging
  # on dylibbundler's interactive "where is this library?" prompt.
  printf 'quit\n' | dylibbundler -of -cd -b "${args[@]}" -d "$C/Frameworks" \
    -p @executable_path/../Frameworks/ -s "$BREW/lib" -s "$MPV/lib"
fi
# Some Homebrew dylibs (brotli on arm64) load their siblings as @rpath/<name> with an
# rpath of @loader_path/../lib, which brew_refs cannot see. Copy every such sibling
# into Frameworks and give each bundled dylib an @loader_path rpath so it resolves there.
fix_rpath_deps() {
  local changed=1 f dep name src d
  while [ "$changed" = 1 ]; do
    changed=0
    for f in "$C"/Frameworks/*.dylib "$C/MacOS/torrentplayer"; do
      [ -f "$f" ] || continue
      for dep in $(otool -L "$f" | tail -n +2 | awk '{print $1}' | grep '^@rpath/' | grep -v '\.framework/' || true); do
        name=${dep#@rpath/}
        [ -e "$C/Frameworks/$name" ] && continue
        src=""
        for d in "$BREW/lib" "$BREW"/opt/*/lib "$MPV/lib"; do
          if [ -e "$d/$name" ]; then src="$d/$name"; break; fi
        done
        if [ -z "$src" ]; then
          echo "package.sh: $f needs $dep, not found in Homebrew" >&2
          exit 1
        fi
        echo "package.sh: bundling $name for $(basename "$f")"
        cp -L "$src" "$C/Frameworks/$name"
        chmod u+w "$C/Frameworks/$name"
        install_name_tool -id "@rpath/$name" "$C/Frameworks/$name"
        changed=1
      done
    done
  done
  for f in "$C"/Frameworks/*.dylib; do
    if ! otool -l "$f" | grep -A2 LC_RPATH | grep -q 'path @loader_path ('; then
      install_name_tool -add_rpath @loader_path "$f"
    fi
  done
}
fix_rpath_deps

left=()
while IFS= read -r f; do left+=("$f"); done < <(brew_refs)
if [ "${#left[@]}" -gt 0 ]; then
  echo "package.sh: these still load libraries from Homebrew:" >&2
  for f in "${left[@]}"; do echo "  $f" >&2; otool -L "$f" | grep -E "($BREW|/opt/homebrew|/usr/local)/" >&2; done
  exit 1
fi

# ---- sign (ad hoc), check, smoke test -----------------------------------------------
codesign --force --deep -s - "$APP"
codesign --verify --deep --strict "$APP"
"$C/MacOS/torrentplayer" --version

# ---- dmg with an Applications link ------------------------------------------------------
STAGE="$BUILD/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$OUT/$DMG_NAME"
hdiutil create -volname "Torrent Player" -srcfolder "$STAGE" -ov -format UDZO "$OUT/$DMG_NAME"
echo "Built $OUT/$DMG_NAME"
