# AUR package: `torrentplayer-bin`

Prebuilt desktop binaries for Arch Linux, packaged from the GitHub release
assets (`torrentplayer-<ver>-1-x86_64.pkg.tar.zst`).

This directory is the **source of truth** in the torrent-player repo. The live
AUR git repo is separate; publish by copying files there. **Never push to the
AUR from CI or from an automated agent.**

## Files

| File | Role |
| --- | --- |
| `torrentplayer-bin/PKGBUILD` | Package recipe |
| `torrentplayer-bin/.SRCINFO` | Generated metadata (`makepkg --printsrcinfo`) |
| `update.sh` | Bump `pkgver`, fill real `sha256sums`, regenerate `.SRCINFO` |

## Update for a new release

From the repo root (or this directory):

```sh
desktop/aur/update.sh 2.6.0
```

That downloads the Arch package from
`https://github.com/karmugilen/torrent-player/releases/download/v2.6.0/`,
writes the real SHA-256 into `PKGBUILD`, and refreshes `.SRCINFO`.
`sha256sums=('SKIP')` is not allowed; the committed placeholder must be
replaced before publishing.

## Publish to aur.archlinux.org

1. Create an [AUR account](https://aur.archlinux.org/register) if you do not
   have one, and add an SSH public key under **My Account → SSH Public Keys**.

2. Clone the empty (or existing) package repo over SSH:

   ```sh
   git clone ssh://aur@aur.archlinux.org/torrentplayer-bin.git
   cd torrentplayer-bin
   ```

3. Copy the package files from this tree:

   ```sh
   cp /path/to/torrent-player/desktop/aur/torrentplayer-bin/PKGBUILD .
   cp /path/to/torrent-player/desktop/aur/torrentplayer-bin/.SRCINFO .
   ```

4. Commit and push (AUR expects both `PKGBUILD` and `.SRCINFO`):

   ```sh
   git add PKGBUILD .SRCINFO
   git commit -m "torrentplayer-bin 2.6.0-1"
   git push
   ```

5. Check the package page:
   https://aur.archlinux.org/packages/torrentplayer-bin

Users install with:

```sh
yay -S torrentplayer-bin
# or
paru -S torrentplayer-bin
```

## Local smoke check

```sh
cd desktop/aur/torrentplayer-bin
makepkg --printsrcinfo > /tmp/srcinfo.check
diff -u .SRCINFO /tmp/srcinfo.check
namcap PKGBUILD
# After update.sh has real checksums and network is available:
# makepkg -f
```
