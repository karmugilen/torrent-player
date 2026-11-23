# Builds the Windows app and packs it as a portable zip and an Inno Setup installer:
#   <OutDir>\TorrentPlayer-<Version>-windows-x64-portable.zip
#   <OutDir>\TorrentPlayer-<Version>-windows-x64-setup.exe
#
# Run from the repo root in a shell with the MSVC x64 environment (vcvars64 or
# ilammy/msvc-dev-cmd) and Qt's bin directory on PATH, after building the engine:
#   cargo build --release --locked --manifest-path engine-rs/Cargo.toml -p wt-engine --bin engine-host
#   pwsh desktop/packaging/windows/build.ps1 -Version 2.6.0 -OutDir dist
# Needs gh (GH_TOKEN) and 7z to fetch libmpv from shinchiro/mpv-winbuild-cmake,
# cmake to build vulkan-1.dll (libmpv hard-imports it), and Inno Setup 6 (ISCC.exe).
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

# DLLs Windows always resolves from System32 (and friends). Anything else that
# dumpbin lists must sit next to the exe or the loader returns STATUS_DLL_NOT_FOUND.
$SystemDll = [System.Collections.Generic.HashSet[string]]::new(
    [string[]]@(
        'advapi32.dll', 'avicap32.dll', 'avrt.dll', 'bcrypt.dll', 'bcryptprimitives.dll',
        'cfgmgr32.dll', 'comctl32.dll', 'comdlg32.dll', 'crypt32.dll', 'd2d1.dll',
        'd3d11.dll', 'd3d12.dll', 'd3d9.dll', 'dbghelp.dll', 'dwmapi.dll', 'dwrite.dll',
        'dxgi.dll', 'gdi32.dll', 'imm32.dll', 'iphlpapi.dll', 'kernel32.dll', 'mpr.dll',
        'msvcrt.dll', 'netapi32.dll', 'normaliz.dll', 'ntdll.dll', 'odbc32.dll',
        'ole32.dll', 'oleaut32.dll', 'opengl32.dll', 'powrprof.dll', 'propsys.dll',
        'psapi.dll', 'rpcrt4.dll', 'secur32.dll', 'setupapi.dll', 'shcore.dll',
        'shell32.dll', 'shlwapi.dll', 'user32.dll', 'userenv.dll', 'usp10.dll',
        'uxtheme.dll', 'version.dll', 'winhttp.dll', 'winmm.dll', 'winspool.drv',
        'wldap32.dll', 'ws2_32.dll', 'wtsapi32.dll'
    ),
    [StringComparer]::OrdinalIgnoreCase
)

function Get-PeDependents([string] $Path) {
    $names = [System.Collections.Generic.List[string]]::new()
    foreach ($line in & dumpbin /nologo /dependents $Path) {
        if ($line -match '^\s+(\S+\.dll)\s*$') { $names.Add($Matches[1]) }
    }
    return $names
}

function Write-MissingDependents([string] $StageDir, [string[]] $Binaries) {
    $missing = [System.Collections.Generic.List[string]]::new()
    foreach ($binary in $Binaries) {
        if (-not (Test-Path $binary)) {
            Write-Host "dumpbin: missing file $binary"
            continue
        }
        Write-Host "dumpbin /dependents $(Split-Path -Leaf $binary):"
        $deps = Get-PeDependents $binary
        foreach ($dep in $deps) {
            Write-Host "  $dep"
            if ($SystemDll.Contains($dep)) { continue }
            if ($dep -like 'api-ms-win-*.dll') { continue }
            if ($dep -like 'ext-ms-*.dll') { continue }
            $beside = Join-Path $StageDir $dep
            $inSystem = Join-Path $env:SystemRoot "System32\$dep"
            if (-not (Test-Path $beside) -and -not (Test-Path $inSystem)) {
                $missing.Add("$(Split-Path -Leaf $binary) -> $dep")
            }
        }
    }
    if ($missing.Count -gt 0) {
        Write-Host "Missing DLLs (not beside the exe and not in System32):"
        $missing | ForEach-Object { Write-Host "  $_" }
    }
    return $missing
}

# ---- libmpv: latest shinchiro dev package, plus an MSVC import library ----------
$Mpv = Join-Path $Work 'mpv'
New-Item -ItemType Directory -Force "$Mpv\lib", "$Work\mpv-download" | Out-Null
gh release download -R shinchiro/mpv-winbuild-cmake --pattern 'mpv-dev-x86_64-2*.7z' -D "$Work\mpv-download"
# Prefer the baseline x86_64 build over x86_64-v3 (needs newer CPUs).
$archive = Get-ChildItem "$Work\mpv-download\mpv-dev-x86_64-2*.7z" |
    Where-Object { $_.Name -notmatch 'x86_64-v3-' } |
    Select-Object -First 1
if (-not $archive) { throw "no mpv-dev-x86_64 (non-v3) archive downloaded" }
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

# Matching player package: companion DLLs (currently d3dcompiler_43.dll) that
# libmpv/mpv expect beside the binary. Same release name with "mpv-dev-" dropped.
$runtimeName = $archive.Name -replace '^mpv-dev-', 'mpv-'
$runtimeDir = Join-Path $Work 'mpv-runtime'
New-Item -ItemType Directory -Force $runtimeDir, "$Work\mpv-download" | Out-Null
gh release download -R shinchiro/mpv-winbuild-cmake --pattern $runtimeName -D "$Work\mpv-download"
$runtimeArchive = Get-ChildItem "$Work\mpv-download\$runtimeName" | Select-Object -First 1
if (-not $runtimeArchive) { throw "no matching runtime package $runtimeName" }
Write-Host "mpv runtime: $($runtimeArchive.Name)"
7z x -y "-o$runtimeDir" $runtimeArchive.FullName | Out-Null
$runtimeDlls = @(Get-ChildItem -Recurse "$runtimeDir\*.dll")
Write-Host "runtime DLLs: $($runtimeDlls.Name -join ', ')"

# ---- vulkan-1.dll: libmpv hard-imports it (not delay-loaded) since ~20260808.
# GPU drivers normally install the loader; GitHub-hosted Windows runners and
# machines without a Vulkan ICD do not have one. Upstream mpv ships it too.
$VulkanSdk = '1.4.309.0'
$VulkanTag = "vulkan-sdk-$VulkanSdk"
$VkWork = Join-Path $Work 'vulkan'
New-Item -ItemType Directory -Force $VkWork | Out-Null
foreach ($repo in 'Vulkan-Headers', 'Vulkan-Loader') {
    $url = "https://github.com/KhronosGroup/$repo/archive/refs/tags/$VulkanTag.tar.gz"
    $tar = Join-Path $VkWork "$repo.tar.gz"
    Write-Host "fetch $repo $VulkanTag"
    Invoke-WebRequest -Uri $url -OutFile $tar
    tar -xzf $tar -C $VkWork
}
$headersSrc = Join-Path $VkWork "Vulkan-Headers-$VulkanTag"
$loaderSrc = Join-Path $VkWork "Vulkan-Loader-$VulkanTag"
$vkPrefix = Join-Path $VkWork 'prefix'
$hdrBuild = Join-Path $headersSrc '_build'
$loaderBuild = Join-Path $loaderSrc '_build'
cmake -S $headersSrc -B $hdrBuild -DCMAKE_BUILD_TYPE=Release "-DCMAKE_INSTALL_PREFIX=$vkPrefix"
cmake --build $hdrBuild --config Release
cmake --install $hdrBuild --config Release
cmake -S $loaderSrc -B $loaderBuild -DCMAKE_BUILD_TYPE=Release `
    "-DVULKAN_HEADERS_INSTALL_DIR=$vkPrefix" -DBUILD_TESTS=OFF -DUPDATE_DEPS=OFF
cmake --build $loaderBuild --config Release
$vulkanDll = Get-ChildItem -Recurse "$loaderBuild\vulkan-1.dll" | Select-Object -First 1
if (-not $vulkanDll) { throw "cmake did not produce vulkan-1.dll under $loaderBuild" }
Write-Host "vulkan-1.dll: $($vulkanDll.FullName)"

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

# ---- deploy folder: app, Qt, MSVC runtime, libmpv, vulkan, engine, GeoIP ------------
$Stage = Join-Path $Work 'TorrentPlayer'
New-Item -ItemType Directory -Force $Stage | Out-Null
Copy-Item $exe.FullName $Stage
windeployqt --release --no-compiler-runtime --qmldir "$Root\desktop\src" "$Stage\torrentplayer.exe"
$crt = Get-ChildItem -Directory "$env:VCToolsRedistDir\x64\Microsoft.VC*.CRT" | Select-Object -First 1
if (-not $crt) { throw "no MSVC runtime under VCToolsRedistDir ($env:VCToolsRedistDir)" }
Copy-Item "$($crt.FullName)\*.dll" $Stage
Copy-Item $dll.FullName $Stage
Copy-Item $vulkanDll.FullName $Stage
foreach ($rd in $runtimeDlls) {
    Copy-Item $rd.FullName $Stage -Force
}
Copy-Item "$Root\engine-rs\target\release\engine-host.exe" $Stage
Copy-Item "$Root\android\app\src\main\assets\geoip-country-*.mmdb" $Stage
Copy-Item "$Root\desktop\LICENSE" "$Stage\LICENSE.txt"

if (-not (Test-Path "$Stage\platforms\qwindows.dll")) {
    throw "windeployqt did not install platforms\qwindows.dll"
}
if (-not (Test-Path "$Stage\$($dll.Name)")) { throw "libmpv $($dll.Name) missing from stage" }
if (-not (Test-Path "$Stage\vulkan-1.dll")) { throw 'vulkan-1.dll missing from stage' }

$stagedExe = Join-Path $Stage 'torrentplayer.exe'
$stagedMpv = Join-Path $Stage $dll.Name
$missing = Write-MissingDependents $Stage @($stagedExe, $stagedMpv)
if ($missing.Count -gt 0) {
    throw "staged folder is missing DLL(s): $($missing -join '; ')"
}

# Smoke test with a bare PATH, so a DLL missing from the folder fails here
# (windeployqt ships only the windows platform plugin; --version shows no window).
$savedPath = $env:PATH
try {
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    $p = Start-Process -FilePath $stagedExe -ArgumentList '--version' -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput "$Work\version.txt" -RedirectStandardError "$Work\version.err.txt"
    $stdout = if (Test-Path "$Work\version.txt") { Get-Content -Raw "$Work\version.txt" } else { '' }
    $stderr = if (Test-Path "$Work\version.err.txt") { Get-Content -Raw "$Work\version.err.txt" } else { '' }
    Write-Host "torrentplayer.exe --version: exit $($p.ExitCode): $stdout"
    if ($stderr) { Write-Host "stderr: $stderr" }
    if ($p.ExitCode -ne 0) {
        # Diagnosis only: briefly put the stage (and the original PATH, which may
        # include Qt) back so dumpbin / a retry log can say what is still absent.
        $env:PATH = "$Stage;$savedPath"
        $hex = '{0:X8}' -f [uint32]([int64]$p.ExitCode -band 0xffffffff)
        Write-Host "Smoke test failed (exit $($p.ExitCode) / 0x$hex); re-checking dependents:"
        $missing = Write-MissingDependents $Stage @($stagedExe, $stagedMpv)
        $hint = if ($missing.Count -gt 0) { ($missing -join '; ') } else { 'see dumpbin output above (loader error 0xC0000135 = STATUS_DLL_NOT_FOUND)' }
        throw "torrentplayer.exe --version failed (exit $($p.ExitCode) / 0x$hex); missing DLL(s): $hint"
    }
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
