# Builds the Windows app and packs it as a portable zip and an Inno Setup installer:
#   <OutDir>\TorrentPlayer-<Version>-windows-x64-portable.zip
#   <OutDir>\TorrentPlayer-<Version>-windows-x64-setup.exe
#
# Run from the repo root in a shell with the MSVC x64 environment (vcvars64 or
# ilammy/msvc-dev-cmd) and Qt's bin directory on PATH, after building the engine:
#   cargo build --release --locked --manifest-path engine-rs/Cargo.toml -p wt-engine --bin engine-host
#   pwsh desktop/packaging/windows/build.ps1 -Version 2.6.0 -OutDir dist
# Needs gh (GH_TOKEN) and 7z to fetch libmpv from shinchiro/mpv-winbuild-cmake,
# and Inno Setup 6 (ISCC.exe).
param(
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $OutDir
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$Root = (Resolve-Path "$PSScriptRoot\..\..\..").Path
New-Item -ItemType Directory -Force $OutDir | Out-Null
$Out = (Resolve-Path $OutDir).Path
$Work = Join-Path $Root 'desktop\build\windows'
if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null

# ---- libmpv: latest shinchiro dev package, plus an MSVC import library ----------
$Mpv = Join-Path $Work 'mpv'
New-Item -ItemType Directory -Force "$Mpv\lib", "$Work\mpv-download" | Out-Null
gh release download -R shinchiro/mpv-winbuild-cmake --pattern 'mpv-dev-x86_64-2*.7z' -D "$Work\mpv-download"
$archive = Get-ChildItem "$Work\mpv-download\*.7z" | Select-Object -First 1
Write-Host "libmpv: $($archive.Name)"
7z x -y "-o$Mpv" $archive.FullName | Out-Null
if (-not (Test-Path "$Mpv\include\mpv\client.h")) { throw "no include/mpv/client.h in $($archive.Name)" }
$dll = Get-ChildItem -Recurse "$Mpv\libmpv*.dll" | Select-Object -First 1
if (-not $dll) { throw "no libmpv dll in $($archive.Name)" }
$def = Get-ChildItem -Recurse "$Mpv\*.def" | Select-Object -First 1
if (-not $def) {
    # shinchiro mpv-dev no longer ships a .def; build one from the DLL for MSVC lib.exe.
    $defPath = Join-Path $Mpv 'lib\mpv.def'
    $names = [System.Collections.Generic.List[string]]::new()
    foreach ($line in & dumpbin /nologo /exports $dll.FullName) {
        if ($line -match '^\s+\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]+\s+(\S+)\s*$') {
            $names.Add($Matches[1])
        }
    }
    if ($names.Count -eq 0) { throw "dumpbin found no exports in $($dll.Name)" }
    (@("LIBRARY $($dll.Name)", 'EXPORTS') + $names) | Set-Content -Encoding ascii $defPath
    $def = Get-Item $defPath
    Write-Host "generated $($names.Count) exports -> $($def.Name)"
}
lib /nologo "/def:$($def.FullName)" "/name:$($dll.Name)" "/out:$Mpv\lib\mpv.lib" /machine:x64
Get-ChildItem -Recurse "$Mpv\libmpv.dll.a" | Copy-Item -Destination "$Mpv\lib"

# ---- the Qt app -------------------------------------------------------------------
$Build = Join-Path $Work 'app'
New-Item -ItemType Directory -Force $Build | Out-Null
Set-Content -Encoding ascii "$Build\app_version.h" "#define APP_VERSION `"$Version`""
Push-Location $Build
qmake "$Root\desktop\torrentplayer.pro" "MPV_PREFIX=$($Mpv -replace '\\','/')" `
    "INCLUDEPATH+=$($Build -replace '\\','/')" 'CONFIG-=debug_and_release' 'CONFIG+=release'
nmake /nologo
Pop-Location
$exe = Get-ChildItem -Recurse "$Build\torrentplayer.exe" | Select-Object -First 1
if (-not $exe) { throw 'torrentplayer.exe was not built' }

# ---- deploy folder: app, Qt, MSVC runtime, libmpv, engine, GeoIP -------------------
$Stage = Join-Path $Work 'TorrentPlayer'
New-Item -ItemType Directory -Force $Stage | Out-Null
Copy-Item $exe.FullName $Stage
windeployqt --release --no-compiler-runtime --qmldir "$Root\desktop\src" "$Stage\torrentplayer.exe"
$crt = Get-ChildItem -Directory "$env:VCToolsRedistDir\x64\Microsoft.VC*.CRT" | Select-Object -First 1
if (-not $crt) { throw "no MSVC runtime under VCToolsRedistDir ($env:VCToolsRedistDir)" }
Copy-Item "$($crt.FullName)\*.dll" $Stage
Copy-Item $dll.FullName $Stage
Copy-Item "$Root\engine-rs\target\release\engine-host.exe" $Stage
Copy-Item "$Root\android\app\src\main\assets\geoip-country-*.mmdb" $Stage
Copy-Item "$Root\desktop\LICENSE" "$Stage\LICENSE.txt"

# Smoke test with a bare PATH, so a DLL missing from the folder fails here
# (windeployqt ships only the windows platform plugin; --version shows no window).
$savedPath = $env:PATH
try {
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    $p = Start-Process -FilePath "$Stage\torrentplayer.exe" -ArgumentList '--version' -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput "$Work\version.txt"
    Write-Host "torrentplayer.exe --version: exit $($p.ExitCode): $(Get-Content -Raw "$Work\version.txt")"
    if ($p.ExitCode -ne 0) { throw "torrentplayer.exe --version failed (exit $($p.ExitCode)); a DLL is likely missing" }
} finally {
    $env:PATH = $savedPath
}

# ---- portable zip (one TorrentPlayer folder inside) ----------------------------------
$zip = Join-Path $Out "TorrentPlayer-$Version-windows-x64-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $Stage -DestinationPath $zip -CompressionLevel Optimal

# ---- installer ---------------------------------------------------------------------------
$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) {
    choco install innosetup -y --no-progress
    $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
}
& $iscc /Qp "/DAppVersion=$Version" "/DSourceDir=$Stage" "/DOutputDir=$Out" "$PSScriptRoot\torrentplayer.iss"

foreach ($f in "TorrentPlayer-$Version-windows-x64-setup.exe", "TorrentPlayer-$Version-windows-x64-portable.zip") {
    if (-not (Test-Path (Join-Path $Out $f))) { throw "missing $f" }
    Write-Host "Built $Out\$f"
}
