#!/bin/sh
# One-line installer for Torrent Player desktop.
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/karmugilen/torrent-player/karmugilen/rust-engine/scripts/install/install.sh | sh
#   curl -fsSL … | sh -s -- --uninstall
#
# Offline / test overrides:
#   TORRENTPLAYER_VERSION=2.6.0
#   TORRENTPLAYER_RELEASE_BASE=file:///tmp/fake   # assets at $BASE/v$VERSION/<name>
#   TORRENTPLAYER_API_BASE=…                     # GitHub API releases list (optional)
#   TORRENTPLAYER_HOME=$HOME                     # install prefix root (tests)
set -eu

REPO="${TORRENTPLAYER_REPO:-karmugilen/torrent-player}"
FALLBACK_VERSION="${TORRENTPLAYER_FALLBACK_VERSION:-2.6.0}"
API_BASE="${TORRENTPLAYER_API_BASE:-https://api.github.com/repos/${REPO}/releases}"
RELEASE_BASE="${TORRENTPLAYER_RELEASE_BASE:-https://github.com/${REPO}/releases/download}"
HOME_DIR="${TORRENTPLAYER_HOME:-${HOME}}"

APP_NAME="Torrent Player"
BIN_NAME="torrentplayer"
DESKTOP_ID="torrentplayer"

die() {
  printf '%s\n' "$*" >&2
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

usage() {
  cat <<EOF
Install Torrent Player on Linux or macOS.

Usage: $0 [--uninstall] [--help]

  --uninstall   Remove a previous user install (AppImage / macOS app)
  --help        Show this help

Environment:
  TORRENTPLAYER_VERSION         Pin a release tag version (e.g. 2.6.0)
  TORRENTPLAYER_RELEASE_BASE    Asset base URL (…/download or file:///…);
                                files are fetched from \$BASE/v\$VERSION/<asset>
  TORRENTPLAYER_API_BASE        Releases API URL (default: GitHub)
  TORRENTPLAYER_HOME            Home directory for user installs (tests)
EOF
}

UNINSTALL=0
for arg in "$@"; do
  case "$arg" in
    --uninstall) UNINSTALL=1 ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      die "Unknown option: $arg (try --help)"
      ;;
  esac
done

OS=$(uname -s)
ARCH=$(uname -m)

# --- helpers -----------------------------------------------------------------

download() {
  # download <url> <dest>
  url=$1
  dest=$2
  if have curl; then
    curl -fsSL --proto '=https' --proto-redir '=https' -o "$dest" "$url" \
      || curl -fsSL -o "$dest" "$url"
  elif have wget; then
    wget -q -O "$dest" "$url"
  else
    die "Need curl or wget to download $url"
  fi
}

# Allow file:// and plain http (local test servers) without --proto=https.
download_asset() {
  url=$1
  dest=$2
  case "$url" in
    https://*)
      download "$url" "$dest"
      ;;
    *)
      if have curl; then
        curl -fsSL -o "$dest" "$url"
      elif have wget; then
        wget -q -O "$dest" "$url"
      else
        die "Need curl or wget"
      fi
      ;;
  esac
}

asset_url() {
  version=$1
  name=$2
  printf '%s/v%s/%s' "${RELEASE_BASE%/}" "$version" "$name"
}

tmpdir_create() {
  if have mktemp; then
    mktemp -d "${TMPDIR:-/tmp}/torrentplayer-install.XXXXXX"
  else
    d="${TMPDIR:-/tmp}/torrentplayer-install.$$"
    mkdir -m 700 "$d"
    printf '%s\n' "$d"
  fi
}

# Returns 0 if SUMS file mentions a desktop asset we care about.
sums_has_desktop_assets() {
  sums_file=$1
  grep -E \
    'TorrentPlayer-.*-(linux-x86_64\.AppImage|windows-x64-setup\.exe|macos-(arm64|x64)\.dmg)|torrentplayer-.*-x86_64\.pkg\.tar\.zst' \
    "$sums_file" >/dev/null 2>&1
}

fetch_sums() {
  version=$1
  dest=$2
  download_asset "$(asset_url "$version" "SHA256SUMS-desktop.txt")" "$dest"
}

# Pick VERSION: env pin, else latest GitHub release that has desktop sums, else fallback.
resolve_version() {
  if [ -n "${TORRENTPLAYER_VERSION:-}" ]; then
    printf '%s\n' "$TORRENTPLAYER_VERSION"
    return
  fi

  # When using a non-GitHub base without a pin, use the fallback tag.
  case "$RELEASE_BASE" in
    https://github.com/*) ;;
    *)
      printf '%s\n' "$FALLBACK_VERSION"
      return
      ;;
  esac

  tmp=$(tmpdir_create)
  api_json="$tmp/releases.json"
  if ! download "$API_BASE" "$api_json" 2>/dev/null; then
    rm -rf "$tmp"
    printf '%s\n' "$FALLBACK_VERSION"
    return
  fi

  # Walk recent releases for SHA256SUMS-desktop.txt (no jq required).
  tags=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$api_json" | head -n 30)
  found=""
  for tag in $tags; do
    ver=${tag#v}
    sums="$tmp/sums-$ver.txt"
    if fetch_sums "$ver" "$sums" 2>/dev/null && sums_has_desktop_assets "$sums"; then
      found=$ver
      break
    fi
  done
  rm -rf "$tmp"

  if [ -n "$found" ]; then
    printf '%s\n' "$found"
  else
    printf '%s\n' "$FALLBACK_VERSION"
  fi
}

verify_sha256() {
  # verify_sha256 <sums_file> <file> <expected_basename>
  sums=$1
  file=$2
  base=$3
  line=$(grep -E "[[:space:]]${base}\$" "$sums" | head -n 1) || true
  if [ -z "$line" ]; then
    die "No SHA256 entry for $base in SHA256SUMS-desktop.txt"
  fi
  expect=$(printf '%s\n' "$line" | awk '{print $1}')
  if have sha256sum; then
    got=$(sha256sum "$file" | awk '{print $1}')
  elif have shasum; then
    got=$(shasum -a 256 "$file" | awk '{print $1}')
  else
    die "Need sha256sum or shasum to verify downloads"
  fi
  if [ "$got" != "$expect" ]; then
    die "SHA256 mismatch for $base (expected $expect, got $got)"
  fi
  printf 'OK  %s\n' "$base"
}

# --- uninstall ---------------------------------------------------------------

uninstall_linux_user() {
  removed=0
  bin="${HOME_DIR}/.local/bin/${BIN_NAME}"
  desktop="${HOME_DIR}/.local/share/applications/${DESKTOP_ID}.desktop"
  icon="${HOME_DIR}/.local/share/icons/hicolor/scalable/apps/${DESKTOP_ID}.svg"
  for f in "$bin" "$desktop" "$icon"; do
    if [ -e "$f" ] || [ -L "$f" ]; then
      rm -f "$f"
      printf 'Removed %s\n' "$f"
      removed=1
    fi
  done
  if have update-desktop-database; then
    update-desktop-database "${HOME_DIR}/.local/share/applications" 2>/dev/null || true
  fi
  if [ "$removed" -eq 0 ]; then
    printf 'No user AppImage install found under %s\n' "$HOME_DIR"
  fi
}

uninstall_macos() {
  removed=0
  for dir in "/Applications/${APP_NAME}.app" "${HOME_DIR}/Applications/${APP_NAME}.app"; do
    if [ -d "$dir" ]; then
      rm -rf "$dir"
      printf 'Removed %s\n' "$dir"
      removed=1
    fi
  done
  if [ "$removed" -eq 0 ]; then
    printf 'No %s.app found in /Applications or ~/Applications\n' "$APP_NAME"
  fi
}

uninstall_arch_hint() {
  if have pacman && pacman -Q torrentplayer >/dev/null 2>&1; then
    printf 'System package torrentplayer is installed. Remove with:\n  sudo pacman -R torrentplayer\n'
  fi
  if have pacman && pacman -Q torrentplayer-bin >/dev/null 2>&1; then
    printf 'AUR package torrentplayer-bin is installed. Remove with:\n  sudo pacman -R torrentplayer-bin\n'
  fi
}

do_uninstall() {
  case "$OS" in
    Linux)
      uninstall_linux_user
      uninstall_arch_hint
      ;;
    Darwin)
      uninstall_macos
      ;;
    *)
      die "Uninstall is supported on Linux and macOS (got $OS)"
      ;;
  esac
}

# --- Linux -------------------------------------------------------------------

install_arch_pkg() {
  version=$1
  work=$2
  sums=$3
  pkg="torrentplayer-${version}-1-x86_64.pkg.tar.zst"
  url=$(asset_url "$version" "$pkg")
  dest="$work/$pkg"
  printf 'Downloading %s…\n' "$pkg"
  download_asset "$url" "$dest"
  verify_sha256 "$sums" "$dest" "$pkg"
  printf 'Installing with pacman (sudo)…\n'
  if [ "$(id -u)" -eq 0 ]; then
    pacman -U --noconfirm "$dest"
  else
    sudo pacman -U --noconfirm "$dest"
  fi
  printf 'Installed system package torrentplayer %s\n' "$version"
  printf 'Tip: for AUR updates later, you can use: yay -S torrentplayer-bin\n'
}

install_linux_appimage() {
  version=$1
  work=$2
  sums=$3
  name="TorrentPlayer-${version}-linux-x86_64.AppImage"
  url=$(asset_url "$version" "$name")
  dest="$work/$name"
  printf 'Downloading %s…\n' "$name"
  download_asset "$url" "$dest"
  verify_sha256 "$sums" "$dest" "$name"

  bindir="${HOME_DIR}/.local/bin"
  appdir="${HOME_DIR}/.local/share/applications"
  icondir="${HOME_DIR}/.local/share/icons/hicolor/scalable/apps"
  mkdir -p "$bindir" "$appdir" "$icondir"

  install_path="${bindir}/${BIN_NAME}"
  mv "$dest" "$install_path"
  chmod +x "$install_path"

  icon_path="${icondir}/${DESKTOP_ID}.svg"
  cat >"$icon_path" <<'SVG'
<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 108 108" width="256" height="256">
  <rect width="108" height="108" rx="24" fill="#161513"/>
  <path d="M34 28 L34 80 L82 54 Z" fill="#E2B84A" stroke="#1A140C" stroke-width="3.2" stroke-linejoin="round"/>
</svg>
SVG

  desktop_path="${appdir}/${DESKTOP_ID}.desktop"
  cat >"$desktop_path" <<EOF
[Desktop Entry]
Type=Application
Name=Torrent Player
Comment=Stream and download torrents
Exec=${install_path} %U
Icon=${DESKTOP_ID}
Terminal=false
Categories=AudioVideo;Video;Player;Network;FileTransfer;P2P;
MimeType=x-scheme-handler/magnet;application/x-bittorrent;
StartupWMClass=torrentplayer
Keywords=torrent;magnet;stream;video;
EOF

  if have update-desktop-database; then
    update-desktop-database "$appdir" 2>/dev/null || true
  fi
  if have xdg-mime; then
    xdg-mime default "${DESKTOP_ID}.desktop" x-scheme-handler/magnet 2>/dev/null || true
  fi

  printf 'Installed AppImage to %s\n' "$install_path"
  printf 'Desktop entry: %s\n' "$desktop_path"
  printf 'If the AppImage fails to start, install libfuse2 (e.g. sudo apt install libfuse2).\n'
  case ":${PATH}:" in
    *":${bindir}:"*) ;;
    *)
      printf 'Add %s to your PATH if the launcher cannot find torrentplayer.\n' "$bindir"
      ;;
  esac
}

install_linux() {
  version=$1
  work=$2
  sums=$3

  case "$ARCH" in
    x86_64|amd64) ;;
    *)
      die "Linux desktop builds are x86_64 only (this machine is $ARCH)"
      ;;
  esac

  if have pacman; then
    install_arch_pkg "$version" "$work" "$sums"
  else
    install_linux_appimage "$version" "$work" "$sums"
  fi
}

# --- macOS -------------------------------------------------------------------

install_macos() {
  version=$1
  work=$2
  sums=$3

  case "$ARCH" in
    arm64|aarch64)
      suffix=macos-arm64
      ;;
    x86_64|amd64)
      suffix=macos-x64
      ;;
    *)
      die "Unsupported macOS arch: $ARCH"
      ;;
  esac

  name="TorrentPlayer-${version}-${suffix}.dmg"
  url=$(asset_url "$version" "$name")
  dest="$work/$name"
  printf 'Downloading %s…\n' "$name"
  download_asset "$url" "$dest"
  verify_sha256 "$sums" "$dest" "$name"

  if ! have hdiutil; then
    die "hdiutil is required to install the macOS DMG"
  fi

  attach_out="$work/attach.txt"
  hdiutil attach "$dest" -nobrowse -readonly -mountroot "$work" >"$attach_out"
  # Prefer mount point that contains the .app
  app_src=$(find "$work" -maxdepth 3 -type d -name "${APP_NAME}.app" 2>/dev/null | head -n 1 || true)
  if [ -z "$app_src" ]; then
    # Parse mount point from hdiutil output (last column)
    mnt=$(awk '/\/Volumes\//{print $NF}' "$attach_out" | tail -n 1)
    if [ -n "$mnt" ] && [ -d "$mnt/${APP_NAME}.app" ]; then
      app_src="$mnt/${APP_NAME}.app"
    fi
  fi
  if [ -z "$app_src" ]; then
    hdiutil detach "$(awk '/\/Volumes\//{print $NF}' "$attach_out" | tail -n 1)" 2>/dev/null || true
    die "Could not find ${APP_NAME}.app inside the DMG"
  fi

  target_root="/Applications"
  if [ ! -w "$target_root" ]; then
    target_root="${HOME_DIR}/Applications"
    mkdir -p "$target_root"
  fi
  target="${target_root}/${APP_NAME}.app"
  rm -rf "$target"
  printf 'Copying to %s…\n' "$target"
  cp -R "$app_src" "$target"

  # Detach the volume that holds the app
  vol=$(printf '%s\n' "$app_src" | sed -n 's|\(/Volumes/[^/]*\)/.*|\1|p')
  if [ -z "$vol" ]; then
    vol=$(awk '/\/Volumes\//{print $NF}' "$attach_out" | tail -n 1)
  fi
  # Also try mountroot-based detach
  if [ -n "$vol" ]; then
    hdiutil detach "$vol" 2>/dev/null || true
  fi
  # Detach anything still mounted under $work
  for m in "$work"/*; do
    if mount | grep -F " $m " >/dev/null 2>&1; then
      hdiutil detach "$m" 2>/dev/null || true
    fi
  done

  if have xattr; then
    xattr -dr com.apple.quarantine "$target" 2>/dev/null || true
  fi

  printf 'Installed %s\n' "$target"
}

# --- main --------------------------------------------------------------------

if [ "$UNINSTALL" -eq 1 ]; then
  do_uninstall
  exit 0
fi

case "$OS" in
  Linux|Darwin) ;;
  *)
    die "This installer supports Linux and macOS. On Windows use:\n  irm https://raw.githubusercontent.com/${REPO}/karmugilen/rust-engine/scripts/install/install.ps1 | iex"
    ;;
esac

VERSION=$(resolve_version)
printf 'Torrent Player desktop %s\n' "$VERSION"

WORK=$(tmpdir_create)
cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

SUMS="$WORK/SHA256SUMS-desktop.txt"
printf 'Fetching SHA256SUMS-desktop.txt…\n'
fetch_sums "$VERSION" "$SUMS"
sums_has_desktop_assets "$SUMS" || die "SHA256SUMS-desktop.txt for v${VERSION} has no desktop assets"

case "$OS" in
  Linux) install_linux "$VERSION" "$WORK" "$SUMS" ;;
  Darwin) install_macos "$VERSION" "$WORK" "$SUMS" ;;
esac

printf 'Done.\n'
