#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cache_dir="${XDG_CACHE_HOME:-${HOME}/.cache}/torrent-player"
local_url="http://127.0.0.1:18181"
mkdir -p "$cache_dir"
pid_file="$cache_dir/discovery.pid"
bin_file="$cache_dir/discovery"
running() {
  [[ -f "$pid_file" ]] || return 1
  local daemon_pid
  read -r daemon_pid < "$pid_file"
  [[ "$daemon_pid" =~ ^[0-9]+$ ]] || return 1
  [[ "$(readlink "/proc/$daemon_pid/exe" 2>/dev/null || true)" == "$bin_file" ]] && kill -0 "$daemon_pid" 2>/dev/null
}
case "${1:-start}" in
  start)
    if running; then echo "$local_url"; exit 0; fi
    # Fast dump/catalog path: Rust mmap index + HTTP (rustdump tag). Falls back
    # to pure Go when the staticlib is missing.
    rust_lib="$repo_dir/engine-rust/discovery/target/release/libwebtor_discovery.a"
    build_tags="nosqlite,noboltdb"
    if command -v cargo >/dev/null; then
      (cd "$repo_dir/engine-rust/discovery" && cargo build --release)
      if [[ -f "$rust_lib" ]]; then
        build_tags="nosqlite,noboltdb,rustdump"
      fi
    fi
    (cd "$repo_dir/engine-go" && go build -tags="$build_tags" -o "$bin_file" ./cmd/discovery)
    discovery_env=()
    if [[ -v TORRENT_ATLAS_FEEDS ]]; then
      discovery_env+=("--setenv=TORRENT_ATLAS_FEEDS=$TORRENT_ATLAS_FEEDS")
    fi
    if command -v systemd-run > /dev/null && systemd-run --user --quiet --collect \
      --unit=torrent-player-discovery \
      "${discovery_env[@]}" \
      --property=Restart=on-failure --property=RestartSec=3 \
      "--property=StandardOutput=append:$cache_dir/discovery.log" \
      --property=StandardError=inherit \
      "$bin_file" --assets "$repo_dir/android/app/src/main/assets" --cache "$cache_dir/discovery.json"; then
      systemctl --user show torrent-player-discovery.service --property=MainPID --value > "$pid_file"
    else
      nohup setsid "$bin_file" --assets "$repo_dir/android/app/src/main/assets" --cache "$cache_dir/discovery.json" < /dev/null > "$cache_dir/discovery.log" 2>&1 &
      echo "$!" > "$pid_file"
    fi
    # Large discovery.json can take tens of seconds to load before /api/state answers.
    for attempt in {1..90}; do
      if curl --silent --fail --max-time 5 "$local_url/api/state?limit=1" > /dev/null; then echo "$local_url"; exit 0; fi
      sleep 1
    done
    echo "Dashboard has not become ready. See $cache_dir/discovery.log" >&2; exit 1
    ;;
  stop)
    systemctl --user stop torrent-player-discovery.service 2>/dev/null || true
    if running; then read -r daemon_pid < "$pid_file"; kill -TERM "$daemon_pid"; fi
    rm -f "$pid_file"
    echo "Discovery stopped."
    ;;
  status)
    if running; then echo "$local_url"; else echo "Discovery is not running."; fi
    ;;
  *) echo "Usage: $0 [start|stop|status]" >&2; exit 2 ;;
esac
