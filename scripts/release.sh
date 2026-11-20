#!/usr/bin/env bash
# One command for a release: checks, F-Droid build + signing, then publishing.
#
#   scripts/release.sh            build, sign, verify and print the plan
#   scripts/release.sh --publish  also create the GitHub release and, only when
#                                 the build steps changed, the fdroiddata MR
#
# Before: raise versionCode/versionName, add
# fastlane/metadata/android/en-US/changelogs/<versionCode>.txt, commit, push.
# The planner does the smallest safe work: when the recipe block is unchanged
# F-Droid's checkupdates picks up the tag by itself (no MR); when anything in
# it differs (NDK, srclibs, toolchain, steps) it opens an MR. Never replace a
# published APK; fix problems in a new version.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PUBLISH=0
[ "${1:-}" = --publish ] && PUBLISH=1
REPO=karmugilen/torrent-player
CACHE="${CACHE:-$HOME/.cache/webtor-release}"
WORK="${WORK:-$CACHE/work}"
cd "$ROOT"

COMMIT="$(git rev-parse HEAD)"
GRADLE=android/app/build.gradle.kts
NAME="$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' "$GRADLE")"
CODE="$(sed -n 's/^ *versionCode = \([0-9]*\)$/\1/p' "$GRADLE")"
NOTES="fastlane/metadata/android/en-US/changelogs/$CODE.txt"
TAG="v$NAME"
APK="artifacts/torrent-player-$NAME.apk"
fail() { echo "release: $*" >&2; exit 1; }

# 1. Checks that cost nothing.
[ -z "$(git status --porcelain --untracked-files=no)" ] || fail "uncommitted changes"
[ -f "$NOTES" ] || fail "missing $NOTES"
[ "$(wc -c <"$NOTES")" -le 500 ] || fail "$NOTES is over 500 characters"
git ls-remote --heads origin | grep -q "^$COMMIT" || fail "push $COMMIT to GitHub first"
! git ls-remote --tags origin | grep -q "refs/tags/$TAG$" || fail "$TAG already exists"
echo "Release $NAME ($CODE) from $COMMIT"

# 2. F-Droid build in its buildserver image, signed and verified.
start=$SECONDS
"$ROOT/scripts/fdroid-release.sh" "$COMMIT"
echo "Build took $(( (SECONDS - start) / 60 )) min $(( (SECONDS - start) % 60 )) s"

# 3. Google Play and Android 15+ need every native library 16 KB aligned.
BT="$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/build-tools/* | sort -V | tail -1)"
"$BT/zipalign" -c -P 16 4 "$APK" >/dev/null || fail "$APK is not 16 KB aligned"
SHA="$(sha256sum "$APK" | cut -d' ' -f1)"
PLAN="$(cat "$WORK/plan")"
echo "Plan: GitHub release $TAG; $PLAN"

if [ $PUBLISH = 0 ]; then
    echo "Checked only. Run scripts/release.sh --publish to publish."
    exit 0
fi

# 4. GitHub release; the tag is created with the APK so it never exists alone.
{ cat "$NOTES"; printf '\n\n---\nSHA-256 `%s`\n' "$SHA"; } >"$WORK/notes.md"
gh release create "$TAG" "$APK" -R "$REPO" --target "$COMMIT" --title "Torrent Player $NAME" \
    --notes-file "$WORK/notes.md" --latest
curl -sfL "https://github.com/$REPO/releases/download/$TAG/$(basename "$APK")" |
    sha256sum | grep -q "^$SHA" || fail "uploaded APK does not match $SHA"
echo "GitHub release $TAG: OK"

# 5. fdroiddata MR only when the build block changed.
if [ "$PLAN" = "recipe: changed" ]; then
    FD="$WORK/fdroiddata"
    BRANCH="webtor.app-$NAME"
    git -C "$FD" remote get-url fork >/dev/null 2>&1 ||
        git -C "$FD" remote add fork https://gitlab.com/karmugilrc/fdroiddata.git
    git -C "$FD" checkout -q -B "$BRANCH"
    git -C "$FD" add metadata/webtor.app.yml
    git -C "$FD" commit -qm "Update Torrent Player to $NAME ($CODE)"
    # The fork rejects shallow pushes whose base it does not have.
    git -C "$FD" fetch -q origin master --shallow-since="$(date -d '-10 days' +%F)"
    git -C "$FD" push -q -u fork "$BRANCH"
    glab mr create -R fdroid/fdroiddata --head karmugilrc/fdroiddata \
        --source-branch "$BRANCH" --target-branch master \
        --title "Update Torrent Player to $NAME ($CODE)" --allow-collaboration --yes \
        --description "Torrent Player $NAME ($CODE). The build steps differ from the previous block, so checkupdates alone would copy the old block. Built with the fdroiddata CI job in \`fdroidserver:buildserver-trixie\`; \`verify_apks\` accepts the GitHub release APK (sha256 \`$SHA\`), signed with the key in \`AllowedAPKSigningKeys\`."
else
    echo "No fdroiddata MR needed: checkupdates picks up $TAG ($PLAN)"
fi
