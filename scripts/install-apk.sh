#!/usr/bin/env bash
# Install a saved build from artifacts/ on the connected phone, keeping its data.
#   scripts/install-apk.sh                 list saved builds
#   scripts/install-apk.sh seekbugs2       install by name or part of a name
set -euo pipefail

ARTIFACTS="$(cd "$(dirname "$0")/../artifacts" && pwd)"

if [ $# -eq 0 ]; then
    find "$ARTIFACTS" -maxdepth 1 -name '*.apk' -printf '%TY-%Tm-%Td %TH:%TM  %f\n' | sort
    exit 0
fi

mapfile -t matches < <(find "$ARTIFACTS" -maxdepth 1 -name "*$1*.apk" | sort)
if [ "${#matches[@]}" -ne 1 ]; then
    # An exact build name wins over longer names containing it.
    mapfile -t exact < <(find "$ARTIFACTS" -maxdepth 1 \( -name "$1" -o -name "webtor-2.3.0-$1-release.apk" \))
    if [ "${#exact[@]}" -ne 1 ]; then
        echo "Expected one build matching '$1', found ${#matches[@]}:" >&2
        printf '  %s\n' "${matches[@]##*/}" >&2
        exit 1
    fi
    matches=("${exact[0]}")
fi
apk="${matches[0]}"

if [ -f "$apk.sha256" ]; then
    (cd "$ARTIFACTS" && sha256sum --quiet -c "$apk.sha256")
fi

devices="$(timeout 10 adb devices | awk 'NR>1 && $2=="device"' | wc -l)"
if [ "$devices" -ne 1 ]; then
    echo "Expected one phone on adb, found $devices." >&2
    timeout 10 adb devices >&2
    exit 1
fi

timeout 180 adb install -r "$apk"
echo "Installed ${apk##*/}"
