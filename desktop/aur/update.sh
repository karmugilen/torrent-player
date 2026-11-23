#!/bin/sh
# Fill real sha256sums for torrentplayer-bin and regenerate .SRCINFO.
# Usage: desktop/aur/update.sh 2.6.0
#
# Downloads:
#   https://github.com/karmugilen/torrent-player/releases/download/v$V/torrentplayer-$V-1-x86_64.pkg.tar.zst
set -eu

REPO="${TORRENTPLAYER_REPO:-karmugilen/torrent-player}"
RELEASE_BASE="${TORRENTPLAYER_RELEASE_BASE:-https://github.com/${REPO}/releases/download}"

usage() {
  printf 'Usage: %s <version>\n' "$0" >&2
  printf 'Example: %s 2.6.0\n' "$0" >&2
  exit 1
}

[ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ] && usage
[ -n "${1:-}" ] || usage

VERSION=${1#v}
ROOT=$(CDPATH='' cd -- "$(dirname "$0")" && pwd)
PKGDIR="${ROOT}/torrentplayer-bin"
PKGBUILD="${PKGDIR}/PKGBUILD"
ASSET="torrentplayer-${VERSION}-1-x86_64.pkg.tar.zst"
URL="${RELEASE_BASE%/}/v${VERSION}/${ASSET}"

[ -f "$PKGBUILD" ] || {
  printf 'Missing %s\n' "$PKGBUILD" >&2
  exit 1
}

have() { command -v "$1" >/dev/null 2>&1; }

if ! have curl && ! have wget; then
  printf 'Need curl or wget\n' >&2
  exit 1
fi
if ! have sha256sum && ! have shasum; then
  printf 'Need sha256sum or shasum\n' >&2
  exit 1
fi
if ! have makepkg; then
  printf 'Need makepkg to regenerate .SRCINFO\n' >&2
  exit 1
fi

WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/torrentplayer-aur-update.XXXXXX")
cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT INT TERM

dest="${WORKDIR}/${ASSET}"
printf 'Downloading %s…\n' "$URL"
case "$URL" in
  https://*)
    if have curl; then
      curl -fsSL --proto '=https' --proto-redir '=https' -o "$dest" "$URL" \
        || curl -fsSL -o "$dest" "$URL"
    else
      wget -q -O "$dest" "$URL"
    fi
    ;;
  *)
    if have curl; then curl -fsSL -o "$dest" "$URL"; else wget -q -O "$dest" "$URL"; fi
    ;;
esac

if have sha256sum; then
  SUM=$(sha256sum "$dest" | awk '{print $1}')
else
  SUM=$(shasum -a 256 "$dest" | awk '{print $1}')
fi
printf 'sha256  %s\n' "$SUM"

# Update pkgver and sha256sums in PKGBUILD (portable sed).
tmp="${WORKDIR}/PKGBUILD.new"
# shellcheck disable=SC2016
awk -v ver="$VERSION" -v sum="$SUM" '
  BEGIN { done_ver=0; done_sum=0 }
  /^pkgver=/ && !done_ver {
    print "pkgver=" ver
    done_ver=1
    next
  }
  /^sha256sums=/ && !done_sum {
    print "sha256sums=('\''" sum "'\'')"
    done_sum=1
    next
  }
  { print }
  END {
    if (!done_ver) exit 2
    if (!done_sum) exit 3
  }
' "$PKGBUILD" >"$tmp"
mv "$tmp" "$PKGBUILD"

printf 'Regenerating .SRCINFO…\n'
(
  cd "$PKGDIR"
  makepkg --printsrcinfo >.SRCINFO
)

printf 'Updated %s to %s\n' "$PKGBUILD" "$VERSION"
printf 'Next: follow desktop/aur/README.md to publish to the AUR (do not push from CI).\n'
