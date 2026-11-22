#!/usr/bin/env bash
# One-shot release: version bump, tests, Play bundle, GitHub + F-Droid, Google Play.
#
#   scripts/ship.sh 2.7.0             do it all
#   scripts/ship.sh 2.7.0 --dry-run   everything local (tests, both builds' checks), publish nothing
#
# Before running, write (both in fastlane/metadata/android/en-US/changelogs/):
#   <code>.txt      short notes, 500 characters max: F-Droid and Google Play show these
#   <code>.md       optional long GitHub notes in the 2.6.0 style; the SHA-256 line is added
# <code> is the current versionCode + 1; the script prints it if the file is missing.
#
# Order: everything that can fail locally runs first, so nothing public happens
# until the tests pass and the Play bundle is checked (key, 16 KB pages, features).
# Then: push -> release.sh --publish (F-Droid build, GitHub release, fdroiddata MR
# only if the recipe changed) -> Google Play closed test (scripts/play-upload.py).
#
# Google Play upload needs a service account key at $PLAY_KEY
# (default ~/.config/webtor/play-service-account.json); see docs/PLAY_STORE.md.
# Without it the script stops after GitHub and prints the manual Play steps.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
NAME="${1:-}"
DRY=0; [ "${2:-}" = --dry-run ] && DRY=1
REPO=karmugilen/torrent-player
BRANCH=karmugilen/rust-engine
PLAY_PACKAGE=torrentplayer.app
PLAY_TRACK="${PLAY_TRACK:-alpha}"
PLAY_KEY="${PLAY_KEY:-$HOME/.config/webtor/play-service-account.json}"
SIGNER=b26e9a22c1a40c5f8bd2bb5ba9e8639f7835c1ca986e9607e240beeddbd9f71f
GRADLE=android/app/build.gradle.kts
KNOWN="$ROOT/scripts/known-test-failures.txt"
LOG="/tmp/ship-$NAME.log"

step() { printf '\n== %s\n' "$*"; }
fail() { printf '\nship: %s\n' "$*" >&2; exit 1; }

[[ "$NAME" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "usage: scripts/ship.sh <x.y.z> [--dry-run]"
OLD_CODE="$(sed -n 's/^ *versionCode = \([0-9]*\)$/\1/p' "$GRADLE")"
OLD_NAME="$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' "$GRADLE")"
CODE=$((OLD_CODE + 1))
NOTES="fastlane/metadata/android/en-US/changelogs/$CODE.txt"
LONG="fastlane/metadata/android/en-US/changelogs/$CODE.md"

# ---- 1. Checks that cost nothing --------------------------------------------
step "1/8 checks: $OLD_NAME ($OLD_CODE) -> $NAME ($CODE)"
[ "$(printf '%s\n%s\n' "$OLD_NAME" "$NAME" | sort -V | tail -1)" = "$NAME" ] && [ "$NAME" != "$OLD_NAME" ] ||
    fail "$NAME is not above $OLD_NAME"
[ -z "$(git status --porcelain --untracked-files=no)" ] || fail "commit or stash your changes first"
[ -f "$NOTES" ] || fail "write $NOTES first (short notes, 500 characters max)"
[ "$(wc -c <"$NOTES")" -le 500 ] || fail "$NOTES is $(wc -c <"$NOTES") characters, 500 max"
git fetch -q origin
git ls-remote --heads origin | grep -q "refs/heads/$BRANCH$" || fail "origin has no $BRANCH"
git merge-base --is-ancestor "origin/$BRANCH" HEAD || fail "origin/$BRANCH has commits you do not have: pull first"
git ls-remote --tags origin | grep -q "refs/tags/v$NAME$" && fail "tag v$NAME already exists"
[ "$(date +%u)" = 7 ] || echo "note: today is not Sunday (weekly releases go out on Sundays)"
if [ -f "$PLAY_KEY" ]; then
    python3 "$ROOT/scripts/play-upload.py" "$PLAY_KEY" "$PLAY_PACKAGE" --check | tail -1 || fail "Play key check failed"
else
    echo "note: no Play key at $PLAY_KEY; Play will be manual"
fi
echo "notes: $NOTES ($(wc -c <"$NOTES") chars)$([ -f "$LONG" ] && echo ", long GitHub notes: $LONG")"

# ---- 2. Version bump ---------------------------------------------------------
step "2/8 version bump"
sed -i "s/^\( *versionCode = \)$OLD_CODE$/\1$CODE/; s/^\( *versionName = \)\"$OLD_NAME\"$/\1\"$NAME\"/" "$GRADLE"
grep -qE "versionCode = $CODE$" "$GRADLE" && grep -q "versionName = \"$NAME\"" "$GRADLE" || fail "version bump failed"
COMMITTED=0
restore() { if [ $COMMITTED = 0 ]; then git checkout -q -- "$GRADLE"; echo "version bump undone (nothing committed)"; fi; }
trap restore EXIT

# Release toolchain (MISTAKES.md: rustup Rust, no sccache, no configuration cache).
set -a; . "$HOME/.config/webtor/credentials.env"; set +a
export PATH="$HOME/.cargo/bin:$PATH" RUSTC_WRAPPER=

# ---- 3. Unit tests: only the known failures may fail --------------------------
step "3/8 Kotlin unit tests (log: $LOG)"
( cd android && ./gradlew -q --no-configuration-cache :app:testDebugUnitTest -x :app:buildRustEngine ) >"$LOG" 2>&1 || true
FAILED="$(python3 - <<'EOF'
import glob, xml.etree.ElementTree as E
for f in sorted(glob.glob("android/app/build/test-results/testDebugUnitTest/*.xml")):
    for t in E.parse(f).getroot().iter("testcase"):
        if t.find("failure") is not None or t.find("error") is not None:
            print(f'{t.get("classname")}.{t.get("name")}')
EOF
)"
[ -n "$(ls android/app/build/test-results/testDebugUnitTest/*.xml 2>/dev/null)" ] || fail "tests did not run, see $LOG"
NEW="$(comm -23 <(printf '%s\n' "$FAILED" | sed '/^$/d' | sort) <(grep -v '^#' "$KNOWN" | sed '/^$/d' | sort))"
[ -z "$NEW" ] || fail "new test failures:
$NEW"
echo "tests OK ($(printf '%s\n' "$FAILED" | sed '/^$/d' | wc -l) known failures, none new)"

# ---- 4. Google Play bundle, checked before anything goes public ---------------
step "4/8 Play bundle"
( cd android && ./gradlew -q --no-configuration-cache clean :app:bundleRelease -Pwebtor.applicationId=$PLAY_PACKAGE ) >>"$LOG" 2>&1 ||
    fail "bundle build failed, see $LOG"
AAB="artifacts/torrent-player-$NAME.aab"
cp android/app/build/outputs/bundle/release/app-release.aab "$AAB"
SIG="$(keytool -printcert -jarfile "$AAB" | sed -n 's/.*SHA256: //p' | head -1 | tr -d ':' | tr 'A-F' 'a-f')"
[ "$SIG" = "$SIGNER" ] || fail "$AAB is signed with $SIG, expected the release key $SIGNER"
T="$(mktemp -d)"; unzip -q "$AAB" 'base/lib/*' -d "$T"
for so in "$T"/base/lib/*/*.so; do
    readelf -lW "$so" | awk '/LOAD/{print $NF}' | grep -qvE '^0x(4000|10000)$' && fail "$(basename "$so") is not 16 KB aligned"
done
rm -rf "$T"
FEAT="$(git diff "v$OLD_NAME" -- android/app/src/main/AndroidManifest.xml | grep -E '^\+.*<uses-(feature|permission)' || true)"
[ -z "$FEAT" ] || fail "new manifest features or permissions (Play may drop devices); check, then rerun:
$FEAT"
echo "bundle OK: $AAB, release key, 16 KB pages, no new features"

if [ $DRY = 1 ]; then
    step "dry run done: tests and Play bundle OK; nothing committed, pushed or published"
    exit 0
fi

# ---- 5. Commit and push --------------------------------------------------------
step "5/8 commit and push to $BRANCH"
git add "$GRADLE" "$NOTES"; [ -f "$LONG" ] && git add "$LONG"
git commit -q -m "Release $NAME ($CODE)"
COMMITTED=1
git push -q origin "HEAD:$BRANCH"
echo "pushed $(git rev-parse --short HEAD)"

# ---- 6. F-Droid build + GitHub release (+ fdroiddata MR if the recipe changed) --
step "6/8 F-Droid build and GitHub release (about 10 min)"
if ! "$ROOT/scripts/release.sh" --publish >>"$LOG" 2>&1; then
    tail -15 "$LOG"
    fail "release.sh failed (the release commit is pushed; fix, then run scripts/release.sh --publish and the steps after it)"
fi
grep -E '^(Signed|recipe|Build took|GitHub release|No fdroiddata)' "$LOG" | tail -5
APK_SHA="$(sha256sum "artifacts/torrent-player-$NAME.apk" | cut -d' ' -f1)"

# ---- 7. Long GitHub notes -------------------------------------------------------
step "7/8 GitHub notes"
if [ -f "$LONG" ]; then
    { cat "$LONG"; printf '\n\n---\nSHA-256 `%s`\n' "$APK_SHA"; } >"/tmp/ship-notes-$NAME.md"
    gh release edit "v$NAME" -R "$REPO" --notes-file "/tmp/ship-notes-$NAME.md" >/dev/null
    echo "long notes from $LONG"
else
    echo "short notes only (write $LONG next time for the detailed style)"
fi

# ---- 7b. Linux package (optional; never blocks the Android release) ------------
step "7b/8 Linux package (optional)"
LINUX_PKG=""
if ! command -v qmake6 >/dev/null 2>&1; then
    echo "note: qmake6 not found; skipping Linux package"
elif (
    cd "$ROOT/desktop/pkgbuild"
    makepkg -f --noconfirm
    PKG="torrentplayer-${NAME}-1-x86_64.pkg.tar.zst"
    [ -f "$PKG" ] || exit 1
    gh release upload "v$NAME" "$PKG" -R "$REPO" --clobber
); then
    LINUX_PKG="torrentplayer-${NAME}-1-x86_64.pkg.tar.zst"
    echo "Linux package attached: $LINUX_PKG"
else
    echo "note: Linux package step failed; Android release is unaffected"
fi

# ---- 8. Google Play closed test ------------------------------------------------
step "8/8 Google Play ($PLAY_TRACK)"
if [ -f "$PLAY_KEY" ]; then
    python3 "$ROOT/scripts/play-upload.py" "$PLAY_KEY" "$PLAY_PACKAGE" "$AAB" "$PLAY_TRACK" "$CODE ($NAME)" "$NOTES"
    PLAY="sent for review"
else
    PLAY="NOT uploaded (no $PLAY_KEY)"
    cat <<EOF
No Play key, so upload by hand (2 min):
  Play Console > Torrent Player > Test and release > Closed testing > Alpha >
  Create new release > Upload $AAB > paste $NOTES > Next > Save >
  Publishing overview > Submit for review (bring the tab to the front for the dialog).
EOF
fi

command -v mem >/dev/null && mem log "webtor: shipped $NAME ($CODE): GitHub v$NAME, F-Droid via tag, Play $PLAY" >/dev/null || true
cat <<EOF

== Done: $NAME ($CODE)
  GitHub   https://github.com/$REPO/releases/tag/v$NAME
  F-Droid  picks up v$NAME by itself (a few days)
  Play     $PLAY
  APK sha256 $APK_SHA
  Linux    ${LINUX_PKG:-not attached}
EOF
