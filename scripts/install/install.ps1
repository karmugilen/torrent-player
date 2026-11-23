# One-line installer for Torrent Player on Windows.
# Usage:
#   irm https://raw.githubusercontent.com/karmugilen/torrent-player/karmugilen/rust-engine/scripts/install/install.ps1 | iex
#
# Overrides (for tests / mirrors):
#   $env:TORRENTPLAYER_VERSION = '2.6.0'
#   $env:TORRENTPLAYER_RELEASE_BASE = 'https://github.com/karmugilen/torrent-player/releases/download'
#   $env:TORRENTPLAYER_API_BASE = 'https://api.github.com/repos/karmugilen/torrent-player/releases'

$ErrorActionPreference = 'Stop'

$Repo = if ($env:TORRENTPLAYER_REPO) { $env:TORRENTPLAYER_REPO } else { 'karmugilen/torrent-player' }
$FallbackVersion = if ($env:TORRENTPLAYER_FALLBACK_VERSION) { $env:TORRENTPLAYER_FALLBACK_VERSION } else { '2.6.0' }
$ApiBase = if ($env:TORRENTPLAYER_API_BASE) { $env:TORRENTPLAYER_API_BASE } else { "https://api.github.com/repos/$Repo/releases" }
$ReleaseBase = if ($env:TORRENTPLAYER_RELEASE_BASE) { $env:TORRENTPLAYER_RELEASE_BASE.TrimEnd('/') } else { "https://github.com/$Repo/releases/download" }

function Get-AssetUrl {
    param([string]$Version, [string]$Name)
    return "$ReleaseBase/v$Version/$Name"
}

function Test-DesktopSums {
    param([string]$SumsText)
    return [bool]($SumsText -match 'TorrentPlayer-.*-windows-x64-setup\.exe')
}

function Resolve-Version {
    if ($env:TORRENTPLAYER_VERSION) {
        return $env:TORRENTPLAYER_VERSION.TrimStart('v')
    }

    if ($ReleaseBase -notmatch '^https://github\.com/') {
        return $FallbackVersion
    }

    try {
        $headers = @{
            'User-Agent' = 'TorrentPlayer-Install'
            'Accept'     = 'application/vnd.github+json'
        }
        $releases = Invoke-RestMethod -Uri $ApiBase -Headers $headers
        foreach ($rel in $releases) {
            $ver = $rel.tag_name.TrimStart('v')
            $sumsUrl = Get-AssetUrl -Version $ver -Name 'SHA256SUMS-desktop.txt'
            try {
                $sums = (Invoke-WebRequest -Uri $sumsUrl -Headers $headers -UseBasicParsing).Content
                # GitHub serves release assets as application/octet-stream, which Windows PowerShell 5.1 returns as bytes.
                if ($sums -is [byte[]]) { $sums = [System.Text.Encoding]::UTF8.GetString($sums) }
                if (Test-DesktopSums -SumsText $sums) {
                    return $ver
                }
            } catch {
                continue
            }
        }
    } catch {
        Write-Host "Could not query GitHub releases; falling back to v$FallbackVersion"
    }
    return $FallbackVersion
}

function Get-Sha256FromSums {
    param([string]$SumsText, [string]$FileName)
    foreach ($line in ($SumsText -split "`n")) {
        $trim = $line.Trim()
        if (-not $trim) { continue }
        $parts = $trim -split '\s+', 2
        if ($parts.Count -ge 2) {
            $hash = $parts[0]
            $name = $parts[1].Trim().TrimStart('*')
            if ($name -eq $FileName) {
                return $hash.ToLowerInvariant()
            }
        }
    }
    throw "No SHA256 entry for $FileName in SHA256SUMS-desktop.txt"
}

function Get-FileSha256 {
    param([string]$Path)
    $hash = Get-FileHash -Path $Path -Algorithm SHA256
    return $hash.Hash.ToLowerInvariant()
}

$Version = Resolve-Version
Write-Host "Torrent Player desktop $Version (Windows x64)"

$SetupName = "TorrentPlayer-$Version-windows-x64-setup.exe"
$SumsName = 'SHA256SUMS-desktop.txt'
$Work = Join-Path ([System.IO.Path]::GetTempPath()) ("torrentplayer-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Work | Out-Null

try {
    $sumsPath = Join-Path $Work $SumsName
    $setupPath = Join-Path $Work $SetupName

    Write-Host "Fetching $SumsName…"
    Invoke-WebRequest -Uri (Get-AssetUrl -Version $Version -Name $SumsName) -OutFile $sumsPath -UseBasicParsing
    $sumsText = Get-Content -Path $sumsPath -Raw

    Write-Host "Downloading $SetupName…"
    Invoke-WebRequest -Uri (Get-AssetUrl -Version $Version -Name $SetupName) -OutFile $setupPath -UseBasicParsing

    $expected = Get-Sha256FromSums -SumsText $sumsText -FileName $SetupName
    $actual = Get-FileSha256 -Path $setupPath
    if ($expected -ne $actual) {
        throw "SHA256 mismatch for $SetupName (expected $expected, got $actual)"
    }
    Write-Host "OK  $SetupName"

    Write-Host "Running per-user silent installer…"
    $proc = Start-Process -FilePath $setupPath -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES' -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        throw "Installer exited with code $($proc.ExitCode)"
    }

    $candidates = @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Torrent Player'),
        (Join-Path $env:LOCALAPPDATA 'Programs\TorrentPlayer'),
        (Join-Path $env:LOCALAPPDATA 'Torrent Player'),
        (Join-Path $env:APPDATA 'Torrent Player')
    )
    $installed = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($installed) {
        Write-Host "Installed to $installed"
    } else {
        Write-Host "Installer finished. Look under $env:LOCALAPPDATA\Programs for Torrent Player."
    }
    Write-Host 'Done.'
}
finally {
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
