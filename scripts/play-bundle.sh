#!/usr/bin/env bash
# Builds the Google Play App Bundle, signed with the release key (the same key
# F-Droid and GitHub use; see docs/PLAY_STORE.md). Output: artifacts/*.aab
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
set -a
# shellcheck disable=SC1090
. "$HOME/.config/webtor/credentials.env"
set +a
NAME="$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' "$ROOT/android/app/build.gradle.kts")"
cd "$ROOT/android"
./gradlew -q clean :app:bundleRelease
OUT="$ROOT/artifacts/torrent-player-$NAME.aab"
cp app/build/outputs/bundle/release/app-release.aab "$OUT"
keytool -printcert -jarfile "$OUT" | grep -m1 'SHA256:'
echo "Built $OUT"
