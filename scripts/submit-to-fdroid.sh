#!/usr/bin/env bash
set -euo pipefail

echo "=== F-Droid Automated GitLab Submission ==="

# Check glab authentication
if ! glab auth status >/dev/null 2>&1; then
  echo "Error: glab is not authenticated with GitLab."
  echo "Please authenticate using one of the following methods:"
  echo "  1) Run interactive login: glab auth login"
  echo "  2) Or export your GitLab Personal Access Token: export GITLAB_TOKEN=\"glpat-...\""
  echo ""
  echo "Required token scopes: api, read_repository, write_repository"
  exit 1
fi

USER_LOGIN="$(glab api user | python3 -c 'import sys, json; print(json.load(sys.stdin).get("username", ""))')"
if [ -z "$USER_LOGIN" ]; then
  echo "Could not fetch GitLab username."
  exit 1
fi
echo "Authenticated as GitLab user: $USER_LOGIN"

TEMP_DIR="$(mktemp -d /tmp/fdroiddata-submission.XXXXXX)"
trap 'rm -rf -- "$TEMP_DIR"' EXIT

echo "Forking fdroid/fdroiddata if not already forked..."
glab repo fork fdroid/fdroiddata --clone=false 2>/dev/null || true

echo "Cloning fork ($USER_LOGIN/fdroiddata)..."
git clone "https://gitlab.com/$USER_LOGIN/fdroiddata.git" "$TEMP_DIR"

cd "$TEMP_DIR"
git checkout -b add-webtor-app

echo "Adding metadata/webtor.app.yml..."
cp "/run/media/kar/Turbo/webtor/webtor.app.yml" "$TEMP_DIR/metadata/webtor.app.yml"

git add metadata/webtor.app.yml
git commit -m "Add webtor.app (Torrent Player)"

echo "Pushing branch add-webtor-app..."
git push -u origin add-webtor-app --force

echo "Creating Merge Request on fdroid/fdroiddata..."
MR_DESCRIPTION=$(cat <<'EOF'
### Application Details
- **Package Name**: `webtor.app`
- **Application Name**: Torrent Player
- **License**: MIT
- **Source Code**: https://github.com/karmugilen/torrent-player
- **Issue Tracker**: https://github.com/karmugilen/torrent-player/issues
- **Summary**: Stream and download torrents directly on Android with external media players

### Checklist
- [x] The upstream repository is public and licensed under a free software license.
- [x] The source code contains no non-free binary blobs or proprietary SDKs.
- [x] Does not contain proprietary tracking, analytics, or advertising libraries.
- [x] Application builds with standard Gradle and Android NDK r26b.
- [x] Fastlane metadata and screenshots are present in `fastlane/metadata/android/en-US/`.
- [x] Version matches release tag `v1.4.2` (`versionCode 19`).
- [x] I am the author / upstream maintainer of this project.
EOF
)

glab mr create \
  --repo fdroid/fdroiddata \
  --source-branch add-webtor-app \
  --target-branch master \
  --title "Add webtor.app (Torrent Player)" \
  --description "$MR_DESCRIPTION" \
  --yes

echo "=== Successfully submitted Merge Request to F-Droid! ==="
