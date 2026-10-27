#!/usr/bin/env bash
# Resource governor for the Rust port workers.
# - Keeps at least 10% of RAM available: pauses (SIGSTOP) the largest build
#   processes below that, resumes them (SIGCONT) once 15% is available again.
# - Never touches agent CLIs, the editor, or anything outside the build tools.
# - Exits (alerting the coordinator) only when disk free drops below 30 GB or
#   /tmp below 2 GB, which need a human decision.
set -u
LOG=/tmp/wt-specs/governor.log
STOPPED=/tmp/wt-specs/governor.stopped   # one PID per line, oldest first
: > "$STOPPED"
total=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
low=$(( total / 10 ))        # pause below this (MiB)
high=$(( total * 15 / 100 )) # resume above this (MiB)
BUILD_RE='^(rustc|rust-lld|ld\.lld|lld|cc1|cc1plus|clang|clang-[0-9]+|cc|c\+\+|ld|build-script-b)'

log() { echo "$(date +%T) $*" >> "$LOG"; }

resume_all() {
    while read -r pid; do kill -CONT "$pid" 2>/dev/null; done < "$STOPPED"
    : > "$STOPPED"
}
trap 'resume_all; log "governor exit, all resumed"; exit 0' INT TERM EXIT

log "start: total=${total}M pause<${low}M resume>${high}M"
while true; do
    avail=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
    disk=$(df -BG --output=avail /home/kar | tail -1 | tr -dc 0-9)
    tmp=$(df -BM --output=avail /tmp | tail -1 | tr -dc 0-9)
    nstop=$(grep -c . "$STOPPED")
    echo "$(date +%T) disk=${disk}G tmp=${tmp}M mem=${avail}M paused=${nstop}" > /tmp/wt-specs/guard.last

    if [ "$avail" -lt "$low" ]; then
        # Pause the largest running build process not already paused.
        pid=$(ps -eo pid=,stat=,rss=,comm= --sort=-rss \
            | awk -v re="$BUILD_RE" '$2 !~ /T/ && $4 ~ re {print $1; exit}')
        if [ -n "${pid:-}" ]; then
            kill -STOP "$pid" 2>/dev/null && echo "$pid" >> "$STOPPED" \
                && log "pause pid=$pid ($(ps -o comm=,rss= -p "$pid" | tr -s ' ')) mem=${avail}M"
        fi
        sleep 2
        continue
    fi

    if [ "$nstop" -gt 0 ] && [ "$avail" -gt "$high" ]; then
        # Resume the oldest paused process first.
        pid=$(head -1 "$STOPPED"); sed -i '1d' "$STOPPED"
        kill -CONT "$pid" 2>/dev/null && log "resume pid=$pid mem=${avail}M"
        sleep 3
        continue
    fi

    if [ "$disk" -lt 30 ] || [ "$tmp" -lt 2048 ]; then
        echo "LIMIT disk=${disk}G tmp=${tmp}M mem=${avail}M"
        du -sh /home/kar/.cache/wt-targets/* /home/kar/.cache/wt-tmp/* 2>/dev/null | sort -h | tail -12
        exit 0
    fi
    sleep 5
done
