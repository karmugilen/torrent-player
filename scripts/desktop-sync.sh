#!/usr/bin/env bash
# Find Android commits not yet ported to the desktop app and run a headless
# Claude session in a fresh worktree to port them. Never merges, never pushes.
#
#   scripts/desktop-sync.sh [--dry-run] [--base <branch>]
#
# Marker: desktop/.android-synced (last ported origin/karmugilen/rust-engine SHA).
# Default --base: karmugilen/desktop
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ANDROID_BRANCH=karmugilen/rust-engine
MARKER="$ROOT/desktop/.android-synced"
PROMPT_TEMPLATE="$ROOT/desktop/SYNC_PROMPT.md"
DRY=0
BASE=karmugilen/desktop

usage() {
  echo "usage: scripts/desktop-sync.sh [--dry-run] [--base <branch>]" >&2
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1; shift ;;
    --base)
      [ $# -ge 2 ] || usage
      BASE=$2
      shift 2
      ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

# Paths that never need a desktop port when they are the only files in a commit.
is_ignored_path() {
  local p=$1
  case "$p" in
    artifacts/*|fastlane/*|docs/images/*|desktop/*) return 0 ;;
    *screenshot*|*Screenshot*|*screenshots*) return 0 ;;
    # Store listings live under fastlane/; release scripts:
    scripts/ship.sh|scripts/release.sh|scripts/fdroid-ci-build.sh|scripts/fdroid-release.sh|\
    scripts/play-bundle.sh|scripts/play-upload.py|scripts/phone-build.sh|scripts/install-apk.sh|\
    scripts/update-rust-notices.py|scripts/known-test-failures.txt) return 0 ;;
    *.png|*.jpg|*.jpeg|*.webp) return 0 ;;
    *) return 1 ;;
  esac
}

# Classify a commit: ignore | engine | port
# - ignore: every path is ignored
# - engine: non-ignored paths are only under engine-rs/
# - port: anything else (android app, shared/, top-level scripts that matter, …)
classify_commit() {
  local sha=$1
  local paths path has_non_ignored=0 has_port=0
  paths=$(git diff-tree --no-commit-id --name-only -r "$sha")
  [ -n "$paths" ] || { echo ignore; return; }
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    if is_ignored_path "$path"; then
      continue
    fi
    has_non_ignored=1
    case "$path" in
      engine-rs/*) ;;
      *) has_port=1 ;;
    esac
  done <<<"$paths"
  if [ "$has_non_ignored" -eq 0 ]; then
    echo ignore
  elif [ "$has_port" -eq 0 ]; then
    echo engine
  else
    echo port
  fi
}

git fetch -q origin "$ANDROID_BRANCH"

ANDROID_TIP=$(git rev-parse "origin/$ANDROID_BRANCH")

if [ ! -f "$MARKER" ]; then
  printf '%s\n' "$ANDROID_TIP" >"$MARKER"
  echo "created $MARKER at $ANDROID_TIP"
fi

LAST=$(tr -d '[:space:]' <"$MARKER")
[ -n "$LAST" ] || { echo "desktop-sync: empty marker $MARKER" >&2; exit 1; }
git cat-file -e "$LAST^{commit}" 2>/dev/null || {
  echo "desktop-sync: marker $LAST is not a commit; reset it" >&2
  exit 1
}

PORT_SHAS=()
ENGINE_SHAS=()
while IFS= read -r sha; do
  [ -n "$sha" ] || continue
  case "$(classify_commit "$sha")" in
    ignore) ;;
    engine) ENGINE_SHAS+=("$sha") ;;
    port) PORT_SHAS+=("$sha") ;;
  esac
done < <(git rev-list --reverse "${LAST}..origin/${ANDROID_BRANCH}")

format_commit_list() {
  local sha
  if [ $# -eq 0 ]; then
    echo "(none)"
    return
  fi
  for sha in "$@"; do
    git log -1 --format='%h %s' "$sha"
  done
}

if [ "$DRY" -eq 1 ]; then
  echo "marker: $LAST"
  echo "android tip: $ANDROID_TIP"
  echo "base: $BASE"
  echo
  echo "commits to port:"
  format_commit_list "${PORT_SHAS[@]+"${PORT_SHAS[@]}"}"
  echo
  echo "engine-only (rebuild, no port):"
  format_commit_list "${ENGINE_SHAS[@]+"${ENGINE_SHAS[@]}"}"
  if [ ${#PORT_SHAS[@]} -eq 0 ]; then
    echo
    echo "nothing to port"
  fi
  exit 0
fi

if [ ${#PORT_SHAS[@]} -eq 0 ]; then
  exit 0
fi

DATE=$(date +%Y-%m-%d)
BRANCH="desktop-sync/$DATE"
# If today's branch already exists, suffix with time.
if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
  DATE=$(date +%Y-%m-%d-%H%M%S)
  BRANCH="desktop-sync/$DATE"
fi

WT_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}/torrentplayer-desktop-sync"
WT="$WT_ROOT/$DATE"
mkdir -p "$WT_ROOT"
if [ -e "$WT" ]; then
  WT="$WT_ROOT/${DATE}-$$"
fi

# Resolve base to a local ref (branch or commit).
if git show-ref --verify --quiet "refs/heads/$BASE"; then
  BASE_REF=$BASE
elif git show-ref --verify --quiet "refs/remotes/origin/$BASE"; then
  BASE_REF="origin/$BASE"
else
  echo "desktop-sync: base branch $BASE not found" >&2
  exit 1
fi

git worktree add -b "$BRANCH" "$WT" "$BASE_REF"

REPORT_DIR="$WT/desktop/sync-reports"
mkdir -p "$REPORT_DIR"

PORT_COMMITS=$(format_commit_list "${PORT_SHAS[@]}")
ENGINE_COMMITS=$(format_commit_list "${ENGINE_SHAS[@]+"${ENGINE_SHAS[@]}"}")

PORT_DIFFS=""
for sha in "${PORT_SHAS[@]}"; do
  PORT_DIFFS+=$(printf '\n### %s\n\n' "$(git log -1 --format='%h %s' "$sha")")
  PORT_DIFFS+=$(git show --stat --format=fuller "$sha")
  PORT_DIFFS+=$'\n'
  PORT_DIFFS+=$(git show --format= "$sha")
  PORT_DIFFS+=$'\n'
done

# Escape nothing special beyond the placeholders; body is markdown for Claude.
PROMPT=$(
  sed \
    -e "s|{{DATE}}|$DATE|g" \
    -e "s|{{ANDROID_TIP}}|$ANDROID_TIP|g" \
    -e "s|{{BASE}}|$BASE|g" \
    "$PROMPT_TEMPLATE"
)
# Insert multi-line sections without sed (content may contain & \ etc.).
PROMPT=${PROMPT//\{\{PORT_COMMITS\}\}/$PORT_COMMITS}
PROMPT=${PROMPT//\{\{ENGINE_COMMITS\}\}/$ENGINE_COMMITS}
PROMPT=${PROMPT//\{\{PORT_DIFFS\}\}/$PORT_DIFFS}

PROMPT_FILE="$WT/desktop/sync-reports/_prompt-$DATE.md"
printf '%s\n' "$PROMPT" >"$PROMPT_FILE"

if ! command -v claude >/dev/null 2>&1; then
  echo "desktop-sync: claude not found on PATH; worktree left at $WT on $BRANCH" >&2
  notify-send "Desktop sync ready for review: $BRANCH" "claude missing; worktree at $WT" 2>/dev/null || true
  exit 1
fi

(
  cd "$WT"
  # Headless print mode: one shot, no interactive TUI. Pass a path so large
  # diffs do not blow the process argument limit.
  claude -p --dangerously-skip-permissions \
    "Read the file ${PROMPT_FILE#"$WT"/} and follow it exactly as your task prompt."
) || {
  status=$?
  echo "desktop-sync: claude exited $status; worktree at $WT on $BRANCH" >&2
  notify-send "Desktop sync ready for review: $BRANCH" "claude failed ($status); review $WT" 2>/dev/null || true
  exit "$status"
}

notify-send "Desktop sync ready for review: $BRANCH" "Worktree: $WT" 2>/dev/null || true
echo "desktop-sync: branch $BRANCH ready for review at $WT"
