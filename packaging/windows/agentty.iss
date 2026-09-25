; Inno Setup script for Agentty-<version>-windows-x64-setup.exe (built by scripts/package-windows.ps1 -Installer).
;
; Installs for the current user only (no administrator rights) into %LOCALAPPDATA%\Programs\Agentty and registers the
; same things as install.ps1: Start menu shortcut (desktop shortcut optional), agentty:// links, "Open in Agentty" on
; folders and folder backgrounds, the install folder on the user PATH, and an entry in Settings > Apps.
;
; Command line (besides Inno Setup's own /SILENT, /VERYSILENT, /SUPPRESSMSGBOXES, ...):
;   /WAITPID=<pid>  wait (up to a minute) for that process to exit before installing - the in-app updater passes its own
;   /RELAUNCH       start Agentty when done, also in silent mode
;
; ISCC /DAppVersion=1.2.3 /DBinDir=<folder with agentty.exe, LICENSE, THIRD_PARTY_NOTICES.md> /O<out> agentty.iss

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=1.2.3
#endif
#ifndef BinDir
  #error Pass the folder with agentty.exe: ISCC /DBinDir=...
#endif

[Setup]
; Never change AppId: it identifies the installation for upgrades and uninstall.
AppId={{6F3C2B1E-8A4D-4C7B-9E2F-5A1D7C3B9E40}
AppName=Agentty
AppVersion={#AppVersion}
AppVerName=Agentty {#AppVersion}
AppPublisher=Agentty contributors
AppPublisherURL=https://www.agentty.run
AppSupportURL=https://github.com/empty-user77/Agentty/issues
AppUpdatesURL=https://github.com/empty-user77/Agentty/releases
AppCopyright=Apache-2.0
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Agentty
VersionInfoDescription=Agentty Setup
VersionInfoCompany=Agentty contributors
PrivilegesRequired=lowest
; Windows 10 1809: ConPTY, which every terminal pane needs.
MinVersion=10.0.17763
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
DefaultDirName={userpf}\Agentty
DisableDirPage=auto
DisableProgramGroupPage=yes
DisableReadyPage=yes
UsePreviousAppDir=yes
UninstallDisplayName=Agentty
UninstallDisplayIcon={app}\agentty.exe
SetupIconFile=agentty.ico
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ChangesEnvironment=yes
ChangesAssociations=yes
; Other Agentty processes (MCP servers started by agent sessions) may still run from the install folder. They are never
; closed: the old agentty.exe is renamed out of the way instead (see CurStepChanged).
CloseApplications=no
RestartApplications=no
OutputBaseFilename=Agentty-{#AppVersion}-windows-x64-setup

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
#if FileExists(CompilerPath + "Languages\Korean.isl")
Name: "ko"; MessagesFile: "compiler:Languages\Korean.isl"
#endif
#if FileExists(CompilerPath + "Languages\Japanese.isl")
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"
#endif

[CustomMessages]
; The same text in every language, like install.ps1's context menu entry.
OpenInAgentty=Open in Agentty

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\agentty.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#BinDir}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Agentty"; Filename: "{app}\agentty.exe"; WorkingDir: "{%USERPROFILE}"; Comment: "Agentty - multi-agent AI terminal"
Name: "{userdesktop}\Agentty"; Filename: "{app}\agentty.exe"; WorkingDir: "{%USERPROFILE}"; Tasks: desktopicon

[Registry]
; agentty:// links
Root: HKCU; Subkey: "Software\Classes\agentty"; ValueType: string; ValueName: ""; ValueData: "URL:Agentty"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\agentty"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\agentty\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\agentty.exe"",0"
Root: HKCU; Subkey: "Software\Classes\agentty\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\agentty.exe"" ""%1"""
; "Open in Agentty" on folders and inside folders
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Agentty"; ValueType: string; ValueName: ""; ValueData: "{cm:OpenInAgentty}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Agentty"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\agentty.exe"",0"
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Agentty\command"; ValueType: string; ValueName: ""; ValueData: """{app}\agentty.exe"" ""%V"""
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Agentty"; ValueType: string; ValueName: ""; ValueData: "{cm:OpenInAgentty}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Agentty"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\agentty.exe"",0"
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Agentty\command"; ValueType: string; ValueName: ""; ValueData: """{app}\agentty.exe"" ""%V"""

[UninstallDelete]
Type: files; Name: "{app}\agentty.exe.old-*"

[Run]
Filename: "{app}\agentty.exe"; Description: "{cm:LaunchProgram,Agentty}"; WorkingDir: "{%USERPROFILE}"; Flags: nowait postinstall skipifsilent; Check: not RelaunchRequested
Filename: "{app}\agentty.exe"; WorkingDir: "{%USERPROFILE}"; Flags: nowait; Check: RelaunchRequested

[Code]
const
  SYNCHRONIZE = $00100000;
  EnvironmentKey = 'Environment';
  LegacyUninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\Agentty';

var
  { agentty.exe of the previous version, renamed aside while installing; empty when there was none. }
  PreviousExe: String;
  Installed: Boolean;

function OpenProcess(DesiredAccess: DWORD; InheritHandle: BOOL; ProcessId: DWORD): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(Handle: THandle; Milliseconds: DWORD): DWORD;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

{ True when a bare /Name switch is on the command line. }
function HasSwitch(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/' + Name) = 0 then
      Result := True;
end;

function RelaunchRequested: Boolean;
begin
  Result := HasSwitch('RELAUNCH');
end;

{ The in-app updater starts Setup and then quits: wait for it so the new agentty.exe starts a fresh instance. }
procedure WaitForUpdater;
var
  Pid: Integer;
  Handle: THandle;
begin
  Pid := StrToIntDef(ExpandConstant('{param:WAITPID|0}'), 0);
  if Pid <= 0 then
    Exit;
  Handle := OpenProcess(SYNCHRONIZE, False, Pid);
  if Handle <> 0 then
  begin
    WaitForSingleObject(Handle, 60000);
    CloseHandle(Handle);
  end;
end;

function InitializeSetup: Boolean;
begin
  WaitForUpdater;
  Result := True;
end;

{ A running agentty.exe can't be overwritten but can be renamed, so the previous version is always renamed aside (not
  deleted): other Agentty processes keep running from it, and a failed install can put it back (DeinitializeSetup).
  DeletePreviousExecutables removes it once installed, unless it still runs. }
procedure MoveExecutableAside;
var
  Exe: String;
begin
  Exe := ExpandConstant('{app}\agentty.exe');
  if not FileExists(Exe) then
    Exit;
  PreviousExe := Exe + '.old-' + GetDateTimeString('yyyymmddhhnnss', #0, #0);
  if not RenameFile(Exe, PreviousExe) then
    PreviousExe := '';
end;

{ After a successful install: removes executables renamed aside by this or earlier installs, except those that still run
  (deleting them fails; a later install gets them). }
procedure DeletePreviousExecutables;
var
  Found: TFindRec;
  Dir: String;
begin
  Dir := ExpandConstant('{app}\');
  if FindFirst(Dir + 'agentty.exe.old-*', Found) then
  try
    repeat
      DeleteFile(Dir + Found.Name);
    until not FindNext(Found);
  finally
    FindClose(Found);
  end;
end;

{ Setup failed or was cancelled after the previous version was moved aside: restore it, and start it again when the
  updater asked for a relaunch, so a failed silent update never leaves the user without Agentty. }
procedure DeinitializeSetup;
var
  Exe: String;
  Code: Integer;
begin
  if Installed or (PreviousExe = '') then
    Exit;
  Exe := ExpandConstant('{app}\agentty.exe');
  if not FileExists(Exe) and RenameFile(PreviousExe, Exe) and RelaunchRequested then
    ExecAsOriginalUser(Exe, '', ExpandConstant('{%USERPROFILE}'), SW_SHOWNORMAL, ewNoWait, Code);
end;

function PathContains(const Paths, Dir: String): Boolean;
begin
  Result := Pos(';' + Lowercase(RemoveBackslashUnlessRoot(Dir)) + ';', ';' + Lowercase(Paths) + ';') > 0;
end;

procedure SetUserPath(Add: Boolean);
var
  Paths, Dir, Rest, Entry, Kept: String;
  Split: Integer;
begin
  Dir := RemoveBackslashUnlessRoot(ExpandConstant('{app}'));
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Paths) then
    Paths := '';
  if Add then
  begin
    if PathContains(Paths, Dir) then
      Exit;
    if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then
      Paths := Paths + ';';
    Paths := Paths + Dir;
  end
  else
  begin
    Kept := '';
    Rest := Paths;
    while Rest <> '' do
    begin
      Split := Pos(';', Rest);
      if Split = 0 then
      begin
        Entry := Rest;
        Rest := '';
      end
      else
      begin
        Entry := Copy(Rest, 1, Split - 1);
        Rest := Copy(Rest, Split + 1, Length(Rest));
      end;
      if (Entry <> '') and (CompareText(RemoveBackslashUnlessRoot(Entry), Dir) <> 0) then
      begin
        if Kept <> '' then
          Kept := Kept + ';';
        Kept := Kept + Entry;
      end;
    end;
    Paths := Kept;
  end;
  { REG_EXPAND_SZ keeps entries such as %USERPROFILE%\.local\bin working; ChangesEnvironment broadcasts the change. }
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    MoveExecutableAside;
  if CurStep = ssPostInstall then
  begin
    { The new files are in place: a later failure must not put the old agentty.exe back. }
    Installed := True;
    DeletePreviousExecutables;
    SetUserPath(True);
    { The zip's install.ps1 registers its own Settings > Apps entry for the same folder; this installer replaces it. }
    RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER, LegacyUninstallKey);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    SetUserPath(False);
end;
