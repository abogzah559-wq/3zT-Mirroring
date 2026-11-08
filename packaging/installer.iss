; 3zT Mirroring installer (Inno Setup 6.6 or newer, for the dark wizard).
; Built by packaging\build.ps1.
#ifndef AppVersion
  #define AppVersion "2.0.0"
#endif
#ifndef AppVersionNum
  #define AppVersionNum "2.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\MirroringService"
#endif
#ifndef OutputDir
  #define OutputDir "..\release"
#endif

; Apple Mobile Device Support (Apple's USB driver and service for iPhone and
; iPad). Downloaded from Apple during setup when it is missing, and checked
; against this SHA-256 before it runs.
#define AmdsUrl "https://swcdn.apple.com/content/downloads/20/49/047-76422/qcw2a7028lr8yp4rdkrfyzkvlhi9q1gg2g/AppleMobileDeviceSupport64.msi"
#define AmdsSha256 "b60533fb54e7bd81ffc52d99678a9cce04e58cb54a76281bd7b1309f20d360e9"

[Setup]
AppId={{9C3E2A71-4B6D-4F0E-8D2A-3E7B5C1F6A20}
AppName=3zT Mirroring
AppVersion={#AppVersion}
AppVerName=3zT Mirroring {#AppVersion}
AppPublisher=3zT
AppPublisherURL=https://github.com/abogzah559-wq/3zT-Mirroring
AppSupportURL=https://github.com/abogzah559-wq/3zT-Mirroring/issues
AppUpdatesURL=https://github.com/abogzah559-wq/3zT-Mirroring/releases
AppCopyright=Copyright (C) 2026 3zT and Zenvy Zenshiro
DefaultDirName={autopf}\3zT Mirroring
DefaultGroupName=3zT Mirroring
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\3zTMirroring.exe
UninstallDisplayName=3zT Mirroring
OutputDir={#OutputDir}
OutputBaseFilename=3zT-Mirroring-v{#AppVersion}-Setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern dark
WizardImageFile=..\assets\wizard-large.bmp
WizardSmallImageFile=..\assets\wizard-small.bmp
WizardImageBackColor=#18181b
WizardSmallImageBackColor=#18181b
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Apple's driver and the firewall rule need administrator rights.
PrivilegesRequired=admin
MinVersion=10.0.17763
CloseApplications=yes
SetupLogging=yes
VersionInfoCompany=3zT
VersionInfoDescription=3zT Mirroring Setup
VersionInfoVersion={#AppVersionNum}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "firewall"; Description: "Allow 3zT Mirroring on the local network (needed for Wi-Fi mirroring)"; GroupDescription: "Network:"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Dirs]
Name: "{localappdata}\3zT Mirroring\logs"

[Icons]
Name: "{group}\3zT Mirroring"; Filename: "{app}\3zTMirroring.exe"
Name: "{group}\Uninstall 3zT Mirroring"; Filename: "{uninstallexe}"
Name: "{autodesktop}\3zT Mirroring"; Filename: "{app}\3zTMirroring.exe"; Tasks: desktopicon

[Run]
; Wi-Fi mirroring (AirPlay): the iPhone/iPad must reach the service. Inbound allow, local subnet only.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""3zT Mirroring"""; Flags: runhidden waituntilterminated; Tasks: firewall
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""3zT Mirroring"" dir=in action=allow program=""{app}\MirroringService.exe"" enable=yes profile=any remoteip=localsubnet"; Flags: runhidden waituntilterminated; Tasks: firewall
Filename: "{app}\3zTMirroring.exe"; Description: "{cm:LaunchProgram,3zT Mirroring}"; Flags: nowait postinstall skipifsilent
; In-app update: 3zT Mirroring runs this Setup with /SILENT /RELAUNCH=1 and
; quits; start the new version when the silent install is done.
Filename: "{app}\3zTMirroring.exe"; Flags: nowait runasoriginaluser; Check: RelaunchAfterUpdate

[UninstallRun]
; Stop only the processes started from this install folder.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -Command ""Get-Process 3zTMirroring,MirroringService -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -like '{app}\*' } | Stop-Process -Force"""; Flags: runhidden; RunOnceId: "StopApp"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""3zT Mirroring"""; Flags: runhidden waituntilterminated; RunOnceId: "DelFirewall"
; Studio mode signs its USB driver package with a one-time certificate; take
; those certificates out of the machine stores again.
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -Command ""foreach ($s in 'Root','TrustedPublisher','My') {{ Get-ChildItem Cert:\LocalMachine\$s -ErrorAction SilentlyContinue | Where-Object {{ $_.Subject -like 'CN=3zT Mirroring USB Direct*' } | Remove-Item -Force -ErrorAction SilentlyContinue }"""; Flags: runhidden; RunOnceId: "DelCerts"

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\3zT Mirroring"
Type: filesandordirs; Name: "{userappdata}\3zT Mirroring"
Type: filesandordirs; Name: "{commonappdata}\3zT Mirroring"

[Registry]
; "Start with Windows" (Settings) writes this value itself; remove it on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "3zT Mirroring"; Flags: dontcreatekey uninsdeletevalue

[Code]
var
  DownloadPage: TDownloadWizardPage;
  AmdsResult: String;
  AmdsRestart: Boolean;

function AppleServiceInstalled(): Boolean;
var
  ImagePath: String;
begin
  Result := RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Services\Apple Mobile Device Service', 'ImagePath', ImagePath);
end;

function RelaunchAfterUpdate(): Boolean;
begin
  Result := WizardSilent() and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;

procedure InitializeWizard();
begin
  DownloadPage := CreateDownloadPage(SetupMessage(msgWizardPreparing), 'Downloading Apple Mobile Device Support from Apple...', nil);
end;

// Apple Mobile Device Support: downloaded from Apple (checked against its
// SHA-256) when Ready is confirmed, installed silently after the app files.
// Without it iPhone/iPad work only over Wi-Fi (AirPlay).
function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  // In-app updates run silently: they never fetch Apple's driver (a first
  // install already offered it).
  if (CurPageID <> wpReady) or AppleServiceInstalled() or RelaunchAfterUpdate() then
    Exit;
  DownloadPage.Clear;
  DownloadPage.Add('{#AmdsUrl}', 'AppleMobileDeviceSupport64.msi', '{#AmdsSha256}');
  DownloadPage.Show;
  try
    try
      DownloadPage.Download;
    except
      AmdsResult := 'Apple Mobile Device Support could not be downloaded (' + GetExceptionMessage + ').';
      Log(AmdsResult);
    end;
  finally
    DownloadPage.Hide;
  end;
end;

procedure InstallAppleSupport();
var
  Msi: String;
  Code: Integer;
begin
  Msi := ExpandConstant('{tmp}\AppleMobileDeviceSupport64.msi');
  if AppleServiceInstalled() or not FileExists(Msi) then
    Exit;
  WizardForm.StatusLabel.Caption := 'Installing Apple Mobile Device Support...';
  if Exec(ExpandConstant('{sys}\msiexec.exe'), '/i "' + Msi + '" /qn /norestart', '', SW_HIDE, ewWaitUntilTerminated, Code) and ((Code = 0) or (Code = 3010)) then
  begin
    Log('Apple Mobile Device Support installed');
    AmdsRestart := Code = 3010;
  end
  else
  begin
    AmdsResult := 'Apple Mobile Device Support could not be installed (code ' + IntToStr(Code) + ').';
    Log(AmdsResult);
  end;
end;

// Clean update: install-manifest.txt lists the top-level files and folders
// the previous version installed in {app}. Remove exactly those before the
// new files are copied, so nothing a newer version no longer ships stays.
procedure RemovePreviousFiles();
var
  Lines: TArrayOfString;
  I: Integer;
  Name, Path: String;
begin
  if not LoadStringsFromFile(ExpandConstant('{app}\install-manifest.txt'), Lines) then
    Exit;
  for I := 0 to GetArrayLength(Lines) - 1 do
  begin
    Name := Trim(Lines[I]);
    if (Name = '') or (Name = '.') or (Pos('..', Name) > 0) or (Pos('\', Name) > 0) or (Pos('/', Name) > 0) or (Pos(':', Name) > 0) then
      Continue;
    if CompareText(Copy(Name, 1, 5), 'unins') = 0 then
      Continue;
    Path := ExpandConstant('{app}\') + Name;
    if DirExists(Path) then
      DelTree(Path, True, True, True)
    else if FileExists(Path) then
      DeleteFile(Path);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    RemovePreviousFiles();
  if CurStep = ssPostInstall then
    InstallAppleSupport();
  if CurStep = ssDone then
    FileCopy(ExpandConstant('{log}'), ExpandConstant('{localappdata}\3zT Mirroring\logs\setup.log'), False);
end;

function NeedRestart(): Boolean;
begin
  Result := AmdsRestart;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpFinished) and (AmdsResult <> '') then
    WizardForm.FinishedLabel.Caption := WizardForm.FinishedLabel.Caption + #13#10#13#10 + AmdsResult +
      ' iPhone/iPad over the cable needs it; Wi-Fi mirroring works without it. Install "Apple Devices" from the Microsoft Store to add it later.';
end;
