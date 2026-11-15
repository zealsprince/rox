; Windows installer, compiled by the release workflow with
;   ISCC.exe /DVersion=<x.y.z> scripts\rox.iss
; Source and output paths are relative to this file, so it also builds from a
; local checkout the same way. Nothing here signs: the release workflow signs
; the binaries before this runs and the installer after, so a local build is
; unsigned and SmartScreen shows the unknown-publisher warning for it.

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
; Tells Explorer to reread the file types below once setup finishes.
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\rox.exe"; DestDir: "{app}"; Flags: ignoreversion
; Beside the app binary, where the MCP settings page says it is.
Source: "..\target\release\rox-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; The same keys startup/file_types.rs heals under HKCU at launch, since a
; self-updated install never runs this script again. Keep the two in step.
Root: HKA; Subkey: "Software\Classes\.roxw"; ValueType: string; ValueName: ""; ValueData: "rox.workspace"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.workspace"; ValueType: string; ValueName: ""; ValueData: "rox workspace"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.workspace\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\rox.exe,0"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.workspace\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rox.exe"" ""%1"""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.roxp"; ValueType: string; ValueName: ""; ValueData: "rox.plugin"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.plugin"; ValueType: string; ValueName: ""; ValueData: "rox plugin"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.plugin\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\rox.exe,0"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.plugin\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rox.exe"" ""%1"""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.roxo"; ValueType: string; ValueName: ""; ValueData: "rox.object"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.object"; ValueType: string; ValueName: ""; ValueData: "rox object"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.object\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\rox.exe,0"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox.object\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rox.exe"" ""%1"""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox"; ValueType: string; ValueName: ""; ValueData: "URL:rox"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\rox\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\rox.exe"" ""%1"""; Flags: uninsdeletekey

[Icons]
Name: "{autoprograms}\rox"; Filename: "{app}\rox.exe"
Name: "{autodesktop}\rox"; Filename: "{app}\rox.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\rox.exe"; Description: "{cm:LaunchProgram,rox}"; Flags: nowait postinstall skipifsilent
