; Inno Setup 6 script for Rustle. Build with:  scripts\package.ps1
; or manually:  ISCC /DAppVersion=0.1.0 installer\rustle.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B7C0A3E2-5C1D-4C8E-9A61-3D1B2F5E7A10}
AppName=Rustle
AppVersion={#AppVersion}
AppVerName=Rustle {#AppVersion}
AppPublisher=Rustle contributors
DefaultDirName={autopf}\Rustle
DefaultGroupName=Rustle
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=Rustle-{#AppVersion}-setup
SetupIconFile=..\crates\rustle-app\assets\icon.ico
UninstallDisplayIcon={app}\rustle.exe
LicenseFile=..\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Open with Rustle"" to the folder context menu"

[Files]
Source: "..\target\release\rustle.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Rustle"; Filename: "{app}\rustle.exe"
Name: "{autodesktop}\Rustle"; Filename: "{app}\rustle.exe"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\Directory\shell\Rustle"; ValueType: string; ValueData: "Open with Rustle"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\shell\Rustle"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\rustle.exe"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\Rustle\command"; ValueType: string; ValueData: """{app}\rustle.exe"" ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Rustle"; ValueType: string; ValueData: "Open with Rustle"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Rustle"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\rustle.exe"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\Background\shell\Rustle\command"; ValueType: string; ValueData: """{app}\rustle.exe"" ""%V"""; Tasks: contextmenu

[Run]
Filename: "{app}\rustle.exe"; Description: "Launch Rustle"; Flags: nowait postinstall skipifsilent

[Code]
function GitInstalled(): Boolean;
var
  p: String;
begin
  Result := RegQueryStringValue(HKLM, 'SOFTWARE\GitForWindows', 'InstallPath', p)
         or RegQueryStringValue(HKCU, 'SOFTWARE\GitForWindows', 'InstallPath', p)
         or (FileSearch('git.exe', GetEnv('PATH')) <> '');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and (not GitInstalled()) then
    SuppressibleMsgBox('Rustle needs Git for Windows, which was not found on this PC.' + #13#10 +
      'Download it from https://git-scm.com/download/win before using Rustle.',
      mbInformation, MB_OK, IDOK);
end;
