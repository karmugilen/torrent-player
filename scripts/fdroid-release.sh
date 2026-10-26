#!/usr/bin/env bash
# Builds a pushed release commit exactly as F-Droid does (buildserver image,
# fdroiddata recipe), signs it with the release key and checks the signer.
# The result goes in artifacts/ and is what the GitHub release must carry:
# F-Droid rebuilds the tag, copies this APK's signature onto its own build
# and publishes it only if the two match.
#
# Usage: scripts/fdroid-release.sh [commit]   (default HEAD; must be on GitHub)
# Needs docker (DOCKER="sudo docker" by default), the Android SDK build-tools
# and ~/.config/webtor/credentials.env (see docs/SIGNING.md).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMMIT="$(git -C "$ROOT" rev-parse "${1:-HEAD}")"
GRADLE="$(git -C "$ROOT" show "$COMMIT:android/app/build.gradle.kts")"
NAME="$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' <<<"$GRADLE")"
CODE="$(sed -n 's/^ *versionCode = \([0-9]*\)$/\1/p' <<<"$GRADLE")"
WORK="${WORK:-/tmp/fdroid-release}"
IMAGE=registry.gitlab.com/fdroid/fdroidserver:buildserver-trixie
DOCKER="${DOCKER:-sudo docker}"
SDK="${ANDROID_HOME:-$HOME/Android/Sdk}"
APKSIGNER="$(ls -d "$SDK"/build-tools/* | sort -V | tail -1)/apksigner"
OUT="$ROOT/artifacts/torrent-player-$NAME.apk"

git ls-remote https://github.com/karmugilen/torrent-player.git | grep -q "^$COMMIT" ||
    git -C "$ROOT" branch -r --contains "$COMMIT" | grep -q origin/ ||
    { echo "Push $COMMIT to GitHub first" >&2; exit 1; }

echo "Building $NAME ($CODE) from $COMMIT"
rm -rf "$WORK"
mkdir -p "$WORK"
git clone -q --depth 1 https://gitlab.com/fdroid/fdroiddata.git "$WORK/fdroiddata"

# Add this version as a copy of the recipe's last build, as checkupdates would.
python3 - "$WORK/fdroiddata/metadata/webtor.app.yml" "$NAME" "$CODE" "$COMMIT" <<'PY'
import re, sys
path, name, code, commit = sys.argv[1:]
text = open(path).read()
if re.search(rf"^    versionCode: {code}$", text, re.M):
    sys.exit(0)
builds = list(re.finditer(r"^  - versionName: .*?(?=^\S|^  - )", text, re.M | re.S))
last = builds[-1]
block = last.group(0).rstrip("\n") + "\n"
block = re.sub(r"^  - versionName: .*$", f"  - versionName: {name}", block, flags=re.M)
block = re.sub(r"^    versionCode: .*$", f"    versionCode: {code}", block, flags=re.M)
block = re.sub(r"^    commit: .*$", f"    commit: {commit}", block, flags=re.M)
text = text[:last.end()].rstrip("\n") + "\n\n" + block + "\n" + text[last.end():].lstrip("\n")
text = re.sub(r"^CurrentVersion: .*$", f"CurrentVersion: {name}", text, flags=re.M)
text = re.sub(r"^CurrentVersionCode: .*$", f"CurrentVersionCode: {code}", text, flags=re.M)
open(path, "w").write(text)
PY

$DOCKER pull -q "$IMAGE"
$DOCKER run --rm \
    -v "$WORK/fdroiddata:/builds/fdroiddata" \
    -v "$ROOT/scripts/fdroid-ci-build.sh:/run.sh:ro" \
    "$IMAGE" bash /run.sh "webtor.app:$CODE" >"$WORK/build.log" 2>&1 ||
    { tail -40 "$WORK/build.log"; echo "F-Droid build failed; log: $WORK/build.log" >&2; exit 1; }

UNSIGNED="$WORK/fdroiddata/tmp/webtor.app_$CODE.apk"
[ -f "$UNSIGNED" ] || { echo "No $UNSIGNED; log: $WORK/build.log" >&2; exit 1; }

set -a
# shellcheck disable=SC1090
. "$HOME/.config/webtor/credentials.env"
set +a
# --alignment-preserved keeps every zip byte of F-Droid's build, so F-Droid
# can copy this signature onto its own build of the tag.
"$APKSIGNER" sign --alignment-preserved --ks "$WEBTOR_KEYSTORE" --ks-key-alias "$WEBTOR_KEY_ALIAS" \
    --ks-pass env:WEBTOR_STORE_PASSWORD --key-pass env:WEBTOR_KEY_PASSWORD \
    --out "$OUT" "$UNSIGNED"
rm -f "$OUT.idsig"

WANT="$(sed -n 's/^AllowedAPKSigningKeys: *//p' "$WORK/fdroiddata/metadata/webtor.app.yml")"
GOT="$("$APKSIGNER" verify --print-certs "$OUT" | sed -n 's/^Signer #1 certificate SHA-256 digest: //p')"
[ "$GOT" = "$WANT" ] || { echo "Signer $GOT is not F-Droid's allowed key $WANT" >&2; exit 1; }

cp "$UNSIGNED" "$WORK/webtor.app_$CODE.unsigned.apk"

# The check F-Droid runs before publishing: copy our signature onto its build.
git clone -q --depth 1 https://gitlab.com/fdroid/fdroidserver.git "$WORK/fdroidserver"
cp "$OUT" "$WORK/signed.apk"
$DOCKER run --rm -v "$WORK:/w" "$IMAGE" bash -c '
    mkdir /tmp/v && cd /tmp/v && PYTHONPATH=/w/fdroidserver python3 -c "
import sys
from fdroidserver import common
common.config = {\"sdk_path\": \"/opt/android-sdk\"}
common.fill_config_defaults(common.config)
sys.exit(common.verify_apks(\"/w/signed.apk\", sys.argv[1], \"/tmp/v\"))
" "/w/webtor.app_'"$CODE"'.unsigned.apk"' ||
    { echo "F-Droid would reject $OUT: its signature does not fit F-Droid's build" >&2; exit 1; }
echo "F-Droid verification: OK"
echo "Signed $OUT"
echo "sha256 $(sha256sum "$OUT" | cut -d' ' -f1)"
