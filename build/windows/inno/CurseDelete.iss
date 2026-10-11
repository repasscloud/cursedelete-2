; CurseDelete x64 installer
; Rendered by build-release.ps1. Tokens __...__ are replaced in the work directory.

[Setup]
AppId={{A690B423-3227-47F0-B898-734A1606B704}
AppName=CurseDelete
AppVersion=__VERSION__
AppPublisher=RePassCloud
AppPublisherURL=__REPOSITORY__
AppSupportURL=__REPOSITORY__
AppUpdatesURL=__REPOSITORY__
AppCopyright=Copyright (c) RePassCloud
DefaultDirName={code:GetInstallDir}
DefaultGroupName=CurseDelete
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=__OUTPUT_DIR__
OutputBaseFilename=CurseDelete-v__VERSION__-win-x64-setup
SetupIconFile=__ICON_FILE__
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no
UninstallDisplayName=CurseDelete
VersionInfoCompany=RePassCloud
VersionInfoDescription=CurseDelete installer
VersionInfoProductName=CurseDelete
VersionInfoProductVersion=__VERSION__

[Dirs]
; Program Files already grants Users Read & Execute by default.
; This makes that requirement explicit without granting write access to the app directory.
Name: "{app}"; Permissions: users-readexec

[Files]
Source: "__PAYLOAD_DIR__\cursdel.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "__PAYLOAD_DIR__\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "__PAYLOAD_DIR__\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "__PAYLOAD_DIR__\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Run]
; Optional machine-wide licensing after installation:
;   /LICENSEFILE="C:\Deploy\cursedelete.license"  install a signed licence file (offline)
;   /ACTIVATIONKEY="XXXX-XXXX"                     activate online (needs the build's licensing server URL)
; A licence problem never fails the install; CurseDelete then runs as Community.
Filename: "{app}\cursdel.exe"; \
    Parameters: "license install --system ""{code:GetLicenseFile}"""; \
    WorkingDir: "{app}"; \
    Flags: runhidden waituntilterminated; \
    Check: HasLicenseFile

Filename: "{app}\cursdel.exe"; \
    Parameters: "license activate --system ""{code:GetActivationKey}"""; \
    WorkingDir: "{app}"; \
    Flags: runhidden waituntilterminated; \
    Check: HasActivationKey

[Code]
const
  DefaultInstallPath = 'C:\Program Files\RePassCloud\CurseDelete';

function GetNamedParameter(const Name: String): String;
var
  I: Integer;
  Arg: String;
  Prefix: String;
begin
  Result := '';
  Prefix := '/' + LowerCase(Name) + '=';

  for I := 1 to ParamCount do
  begin
    Arg := ParamStr(I);
    if Pos(Prefix, LowerCase(Arg)) = 1 then
    begin
      Result := Copy(Arg, Length(Prefix) + 1, MaxInt);

      if (Length(Result) >= 2) and
         (Result[1] = '"') and
         (Result[Length(Result)] = '"') then
        Result := Copy(Result, 2, Length(Result) - 2);

      Exit;
    end;
  end;
end;

function GetInstallDir(Param: String): String;
begin
  Result := GetNamedParameter('installdir');
  if Result = '' then
    Result := DefaultInstallPath;
end;

function GetLicenseFile(Param: String): String;
begin
  Result := GetNamedParameter('licensefile');
  if Result <> '' then
    Result := ExpandFileName(Result);
end;

function HasLicenseFile: Boolean;
begin
  Result := GetLicenseFile('') <> '';
end;

function GetActivationKey(Param: String): String;
begin
  Result := GetNamedParameter('activationkey');
end;

function HasActivationKey: Boolean;
begin
  Result := GetActivationKey('') <> '';
end;

function NormalizePathEntry(const Value: String): String;
begin
  Result := RemoveBackslashUnlessRoot(Trim(Value));
end;

function PathContains(const PathValue, Entry: String): Boolean;
var
  Parts: TArrayOfString;
  I: Integer;
  NormalizedEntry: String;
begin
  Result := False;
  NormalizedEntry := LowerCase(NormalizePathEntry(Entry));
  Parts := SplitString(PathValue, ';');

  for I := 0 to GetArrayLength(Parts) - 1 do
  begin
    if LowerCase(NormalizePathEntry(Parts[I])) = NormalizedEntry then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

procedure AddToMachinePath(const Entry: String);
var
  CurrentPath: String;
  NewPath: String;
begin
  if not RegQueryStringValue(
      HKLM64,
      'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
      'Path',
      CurrentPath) then
    CurrentPath := '';

  if not PathContains(CurrentPath, Entry) then
  begin
    if (CurrentPath <> '') and (CurrentPath[Length(CurrentPath)] <> ';') then
      NewPath := CurrentPath + ';' + Entry
    else
      NewPath := CurrentPath + Entry;

    RegWriteExpandStringValue(
      HKLM64,
      'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
      'Path',
      NewPath);
  end;
end;

procedure RemoveFromMachinePath(const Entry: String);
var
  CurrentPath: String;
  Parts: TArrayOfString;
  I: Integer;
  NewPath: String;
  NormalizedEntry: String;
begin
  if not RegQueryStringValue(
      HKLM64,
      'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
      'Path',
      CurrentPath) then
    Exit;

  NormalizedEntry := LowerCase(NormalizePathEntry(Entry));
  Parts := SplitString(CurrentPath, ';');
  NewPath := '';

  for I := 0 to GetArrayLength(Parts) - 1 do
  begin
    if (Trim(Parts[I]) <> '') and
       (LowerCase(NormalizePathEntry(Parts[I])) <> NormalizedEntry) then
    begin
      if NewPath <> '' then
        NewPath := NewPath + ';';
      NewPath := NewPath + Parts[I];
    end;
  end;

  RegWriteExpandStringValue(
    HKLM64,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path',
    NewPath);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddToMachinePath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveFromMachinePath(ExpandConstant('{app}'));
end;
