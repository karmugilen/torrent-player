#!/usr/bin/env bash
# Install the user systemd timer that runs scripts/desktop-sync.sh daily.
# Does not start a sync now; only enables the timer.
#
#   scripts/desktop-sync-install.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
SRC="$ROOT/desktop/sync"

mkdir -p "$UNIT_DIR"

sed "s|@REPO@|$ROOT|g" "$SRC/torrentplayer-sync.service" >"$UNIT_DIR/torrentplayer-sync.service"
install -m644 "$SRC/torrentplayer-sync.timer" "$UNIT_DIR/torrentplayer-sync.timer"

systemctl --user daemon-reload
systemctl --user enable --now torrentplayer-sync.timer

echo "installed and enabled torrentplayer-sync.timer"
systemctl --user list-timers torrentplayer-sync.timer --no-pager
