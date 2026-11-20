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
#
# Downloads persist in $CACHE (default ~/.cache/webtor-release): srclib clones,
# apt archives, NDK, Gradle, rustup and cargo. Every one is pinned (commits,
# checksums), so a warm run gives the same APK as a cold one; only the
# downloading is skipped. Delete the folder to start cold.
#
# Writes $WORK/plan: "recipe: same" when checkupdates can add this version by
# copying the last build block, "recipe: changed" when fdroiddata needs a merge
# request with $WORK/fdroiddata/metadata/webtor.app.yml.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMMIT="$(git -C "$ROOT" rev-parse "${1:-HEAD}")"
GRADLE="$(git -C "$ROOT" show "$COMMIT:android/app/build.gradle.kts")"
NAME="$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' <<<"$GRADLE")"
CODE="$(sed -n 's/^ *versionCode = \([0-9]*\)$/\1/p' <<<"$GRADLE")"
# F-Droid names NDKs by release (r28c); Gradle pins the full version.
NDKVER="$(sed -n 's/^ *ndkVersion = "\(.*\)"$/\1/p' <<<"$GRADLE")"
case "$NDKVER" in
    26.1.10909125) NDK=r26b ;;
    28.2.13676358) NDK=r28c ;;
    *) echo "Unknown ndkVersion $NDKVER: add its F-Droid name here" >&2; exit 1 ;;
esac
CACHE="${CACHE:-$HOME/.cache/webtor-release}"
WORK="${WORK:-$CACHE/work}"
IMAGE=registry.gitlab.com/fdroid/fdroidserver:buildserver-trixie
DOCKER="${DOCKER:-sudo docker}"
SDK="${ANDROID_HOME:-$HOME/Android/Sdk}"
APKSIGNER="$(ls -d "$SDK"/build-tools/* | sort -V | tail -1)/apksigner"
OUT="$ROOT/artifacts/torrent-player-$NAME.apk"

git ls-remote https://github.com/karmugilen/torrent-player.git | grep -q "^$COMMIT" ||
    git -C "$ROOT" branch -r --contains "$COMMIT" | grep -q origin/ ||
    { echo "Push $COMMIT to GitHub first" >&2; exit 1; }

echo "Building $NAME ($CODE) from $COMMIT"
mkdir -p "$WORK" "$CACHE"/{apt,ndk,gradle,cargo,rustup,home-build}
$DOCKER pull -q "$IMAGE"
# The Gradle cache mount hides the image's gradle.properties (no daemon, no
# JDK auto-download, network timeouts): copy it in from the image each run.
# Files written by root in the container would block the reset below.
$DOCKER run --rm -v "$CACHE:/c" "$IMAGE" sh -c \
    "cp /home/vagrant/.gradle/gradle.properties /c/gradle/ && chown -R $(id -u):$(id -g) /c"
if [ -d "$WORK/fdroiddata/.git" ]; then
    git -C "$WORK/fdroiddata" fetch -q --depth 1 origin master
    git -C "$WORK/fdroiddata" reset -q --hard FETCH_HEAD
    git -C "$WORK/fdroiddata" clean -q -ffdx
else
    rm -rf "$WORK/fdroiddata"
    git clone -q --depth 1 https://gitlab.com/fdroid/fdroiddata.git "$WORK/fdroiddata"
fi
rm -f "$WORK/plan" "$WORK"/*.apk

# Add this version as a copy of the recipe's last build, as checkupdates would.
# When the build steps change, the repo's webtor.app.yml carries this version's
# block and that block is used instead.
python3 - "$WORK/fdroiddata/metadata/webtor.app.yml" "$NAME" "$CODE" "$COMMIT" "$ROOT/webtor.app.yml" "$NDK" "$WORK/plan" <<'PY'
import re, sys
path, name, code, commit, ours, ndk, plan = sys.argv[1:]
text = open(path).read()
if re.search(rf"^    versionCode: {code}$", text, re.M):
    open(plan, "w").write("recipe: already has this version\n")
    sys.exit(0)
def blocks(t):
    return list(re.finditer(r"^  - versionName: .*?(?=^\S|^  - )", t, re.M | re.S))
builds = blocks(text)
last = builds[-1]
block = last.group(0).rstrip("\n") + "\n"
for b in blocks(open(ours).read()):
    if re.search(rf"^    versionCode: {code}$", b.group(0), re.M):
        block = b.group(0).rstrip("\n") + "\n"
        print(f"Using the build steps for {code} from webtor.app.yml")
block = re.sub(r"^  - versionName: .*$", f"  - versionName: {name}", block, flags=re.M)
block = re.sub(r"^    versionCode: .*$", f"    versionCode: {code}", block, flags=re.M)
block = re.sub(r"^    commit: .*$", f"    commit: {commit}", block, flags=re.M)
block = re.sub(r"^    ndk: .*$", f"    ndk: {ndk}", block, flags=re.M)
# Same build steps as the last block (only version and commit differ) means
# F-Droid's checkupdates adds this version by itself; otherwise open an MR.
def steps(b):
    return re.sub(r"^(  - versionName|    versionCode|    commit): .*\n", "", b, flags=re.M).strip()
same = steps(block) == steps(last.group(0))
open(plan, "w").write("recipe: same\n" if same else "recipe: changed\n")
text = text[:last.end()].rstrip("\n") + "\n\n" + block + "\n" + text[last.end():].lstrip("\n")
text = re.sub(r"^CurrentVersion: .*$", f"CurrentVersion: {name}", text, flags=re.M)
text = re.sub(r"^CurrentVersionCode: .*$", f"CurrentVersionCode: {code}", text, flags=re.M)
open(path, "w").write(text)
PY

# The cached build dir keeps srclib clones, but fdroid reuses an app clone it
# finds there without fetching, so a new commit is missing ("unable to read
# tree"). Clone the app fresh every run; it is small.
rm -rf "$CACHE/home-build/webtor.app" "$CACHE/home-build/.fdroidvcs-webtor.app"

# git:// (port 9418) is blocked on some networks, and Savannah's https is often
# too overloaded to clone from, so take the freetype srclib from freedesktop's
# GitLab mirror (same history). DOCKER_RUN_ARGS adds more docker run options.
# shellcheck disable=SC2086
$DOCKER run --rm ${DOCKER_RUN_ARGS:-} \
    -e GIT_CONFIG_COUNT=1 \
    -e GIT_CONFIG_KEY_0=url.https://gitlab.freedesktop.org/freetype/freetype.git.insteadOf \
    -e GIT_CONFIG_VALUE_0=git://git.sv.nongnu.org/freetype/freetype2.git \
    -v "$CACHE/apt:/var/cache/apt/archives" \
    -v "$CACHE/ndk:/opt/android-sdk/ndk" \
    -v "$CACHE/gradle:/home/vagrant/.gradle" \
    -v "$CACHE/cargo:/home/vagrant/.cargo" \
    -v "$CACHE/rustup:/home/vagrant/.rustup" \
    -v "$CACHE/home-build:/home/vagrant/build" \
    -v "$WORK/fdroiddata:/builds/fdroiddata" \
    -v "$ROOT/scripts/fdroid-ci-build.sh:/run.sh:ro" \
    "$IMAGE" bash /run.sh "webtor.app:$CODE" >"$WORK/build.log" 2>&1 ||
    { tail -40 "$WORK/build.log"; echo "F-Droid build failed; log: $WORK/build.log" >&2; exit 1; }

$DOCKER run --rm -v "$CACHE:/c" "$IMAGE" chown -R "$(id -u):$(id -g)" /c

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
if [ -d "$WORK/fdroidserver/.git" ]; then
    git -C "$WORK/fdroidserver" pull -q --ff-only
else
    git clone -q --depth 1 https://gitlab.com/fdroid/fdroidserver.git "$WORK/fdroidserver"
fi
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
cat "$WORK/plan"
