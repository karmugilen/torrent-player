; Torrent Player installer (Inno Setup 6). Per-user install, no admin needed.
; Silent: TorrentPlayer-<v>-windows-x64-setup.exe /VERYSILENT /SUPPRESSMSGBOXES
; Built by build.ps1:  ISCC /DAppVersion=2.6.0 /DSourceDir=<deploy folder> /DOutputDir=<dir> torrentplayer.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\build\windows\TorrentPlayer"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\build\windows"
#endif

[Setup]
; Fixed forever: Windows uses it to find this app for upgrades and uninstall.
AppId={{6F2B8E4C-3A71-4D9E-B5C2-8E1F0A7D4C93}
AppName=Torrent Player
AppVersion={#AppVersion}
AppVerName=Torrent Player {#AppVersion}
AppPublisher=Torrent Player
AppPublisherURL=https://github.com/karmugilen/torrent-player
AppSupportURL=https://github.com/karmugilen/torrent-player/issues
AppUpdatesURL=https://github.com/karmugilen/torrent-player/releases
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Torrent Player
DefaultGroupName=Torrent Player
DisableProgramGroupPage=yes
DisableDirPage=auto
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayName=Torrent Player
UninstallDisplayIcon={app}\torrentplayer.exe
CloseApplications=force
RestartApplications=no
OutputDir={#OutputDir}
OutputBaseFilename=TorrentPlayer-{#AppVersion}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "magnet"; Description: "Open magnet links with Torrent Player"; GroupDescription: "Links:"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Torrent Player"; Filename: "{app}\torrentplayer.exe"
Name: "{autodesktop}\Torrent Player"; Filename: "{app}\torrentplayer.exe"; Tasks: desktopicon

[Registry]
; Per-user magnet: handler (HKCU, so no admin); removed again on uninstall.
Root: HKCU; Subkey: "Software\Classes\magnet"; ValueType: string; ValueName: ""; ValueData: "URL:Magnet link"; Flags: uninsdeletekey; Tasks: magnet
Root: HKCU; Subkey: "Software\Classes\magnet"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Tasks: magnet
Root: HKCU; Subkey: "Software\Classes\magnet\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\torrentplayer.exe"",0"; Tasks: magnet
Root: HKCU; Subkey: "Software\Classes\magnet\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\torrentplayer.exe"" ""%1"""; Tasks: magnet

[Run]
Filename: "{app}\torrentplayer.exe"; Description: "{cm:LaunchProgram,Torrent Player}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; The app hides to the tray and runs engine-host; stop both before removing files.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /T /IM torrentplayer.exe"; Flags: runhidden; RunOnceId: "StopApp"
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM engine-host.exe"; Flags: runhidden; RunOnceId: "StopEngine"
