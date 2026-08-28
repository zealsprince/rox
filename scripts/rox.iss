; Windows installer, compiled by the release workflow with
;   ISCC.exe /DVersion=<x.y.z> scripts\rox.iss
; Source and output paths are relative to this file, so it also builds from a
; local checkout the same way. The installer is unsigned (no Windows signing
; certificate yet), so SmartScreen shows the unknown-publisher warning, same
; as the bare zip.

#ifndef Version
  #error Pass the workspace version with /DVersion=<x.y.z>
#endif

[Setup]
; Stable install identity: future installers with the same AppId upgrade in
; place instead of installing beside the old copy. Never change it.
AppId={{7E4C9B2A-5D31-4F8E-A6C0-93B7D1F52E84}
AppName=rox
AppVersion={#Version}
AppPublisher=Andrew Lake
AppPublisherURL=https://rox.music
; Per-user by default: the in-app updater only self-applies when the install
; folder takes writes (can_update in startup/updater.rs), and Program Files
; doesn't without elevation. lowest resolves {autopf} to {localappdata}\
; Programs, which keeps self-update working; the dialog still offers an
; all-users install, which falls back to notify-only updates.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\rox
DefaultGroupName=rox
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
OutputDir=..
OutputBaseFilename=rox-v{#Version}-windows-x86_64-setup
SetupIconFile=..\crates\rox\assets\app\rox.ico
UninstallDisplayIcon={app}\rox.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\rox.exe"; DestDir: "{app}"; Flags: ignoreversion
; Beside the app binary, where the MCP settings page says it is.
Source: "..\target\release\rox-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\rox"; Filename: "{app}\rox.exe"
Name: "{autodesktop}\rox"; Filename: "{app}\rox.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\rox.exe"; Description: "{cm:LaunchProgram,rox}"; Flags: nowait postinstall skipifsilent
