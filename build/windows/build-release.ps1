#requires -Version 5.1
<#
.SYNOPSIS
    Builds, signs and packages CurseDelete for x64 Windows.

.DESCRIPTION
    This file is intentionally self-contained. It generates the temporary
    WiX, Inno Setup and MSIX source files itself so stale packaging templates
    cannot break the build.

    Outputs:
      - CurseDelete-v<VERSION>-win-x64-setup.exe
      - CurseDelete-v<VERSION>-win-x64.msi
      - CurseDelete-v<VERSION>-win-x64.msix
      - CurseDelete-v<VERSION>-win-x64-portable.zip
      - CurseDelete-v<VERSION>-win-x64-portable.7z
      - SHA256SUMS.txt
#>

[CmdletBinding()]
param(
    [string]$ProjectRoot,
    [string]$CertificateThumbprint,
    [switch]$SkipClean,
    [switch]$SkipRustBuild,
    [switch]$SkipSigning
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# =============================================================================
# Paths
# =============================================================================

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

if (-not $ProjectRoot) {
    # build\windows\build-release.ps1 -> repo root is two levels up
    $ProjectRoot = [IO.Path]::GetFullPath((Join-Path $ScriptDir '..\..'))
}
else {
    $ProjectRoot = [IO.Path]::GetFullPath($ProjectRoot)
}

$CargoToml = Join-Path $ProjectRoot 'Cargo.toml'
$CargoLock = Join-Path $ProjectRoot 'Cargo.lock'
$Readme    = Join-Path $ProjectRoot 'README.md'
$Changelog = Join-Path $ProjectRoot 'CHANGELOG.md'
$License   = Join-Path $ProjectRoot 'LICENSE'
$IconIco   = Join-Path $ProjectRoot 'icons\cursedelete-shredder-icon.ico'
$IconPng   = Join-Path $ProjectRoot 'icons\cursedelete-shredder-icon-master.png'

$RustBinary = Join-Path $ProjectRoot 'target\release\cursdel.exe'

$OutputDir   = Join-Path $ProjectRoot 'dist\windows'
$WorkDir     = Join-Path $OutputDir '.work'
$PayloadDir  = Join-Path $WorkDir 'payload'
$WixWorkDir  = Join-Path $WorkDir 'wix'
$InnoWorkDir = Join-Path $WorkDir 'inno'
$MsixWorkDir = Join-Path $WorkDir 'msix'

# =============================================================================
# Fixed local tool paths
# =============================================================================

$Cargo        = Join-Path -Path $env:USERPROFILE -ChildPath '.cargo\bin\cargo.exe'
$Wix          = Join-Path -Path $env:USERPROFILE -ChildPath '.dotnet\tools\wix.exe'
$InnoCompiler = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
$MakeAppx     = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\makeappx.exe'
$SignTool     = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe'
$SevenZip     = 'C:\Program Files\7-Zip\7z.exe'

# =============================================================================
# Product constants
# =============================================================================

$ProductName       = 'CurseDelete'
$Publisher          = 'RePassCloud'
$BinaryName         = 'cursdel.exe'
$CargoPackage       = 'cursdel-cli'
$DefaultInstallDir  = 'C:\Program Files\RePassCloud\CurseDelete'
$MsixIdentityName   = 'RePassCloud.CurseDelete'
$MsixPublisher      = 'CN=RePassCloud'

# Stable MSI identifiers. Do not regenerate these between releases.
$MsiUpgradeCode           = 'A690B423-3227-47F0-B898-734A1606B704'
$ComponentGuidExe         = '5DCDB0F8-A4B4-49D8-BBB4-5D4D4170D37E'
$ComponentGuidReadme      = 'F251042F-37C4-4DAE-9005-D28557340DF3'
$ComponentGuidChangelog   = 'D115EC62-CCFA-426E-882C-C9019A12F78B'
$ComponentGuidLicense     = '947DAF96-4634-41F8-9B52-B6027DC5D91D'
$ComponentGuidEnvironment = '370AFA18-BB44-47E3-94BA-C5665836884C'

# =============================================================================
# Helpers
# =============================================================================

function Write-Step {
    param([Parameter(Mandatory)][string]$Message)
    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Require-File {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Description
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Description not found: $Path"
    }
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter(Mandatory)][string[]]$ArgumentList,
        [string]$WorkingDirectory = $ProjectRoot
    )

    Write-Host "    $FilePath $($ArgumentList -join ' ')" -ForegroundColor DarkGray

    Push-Location $WorkingDirectory
    try {
        & $FilePath @ArgumentList
        $exitCode = $LASTEXITCODE

        if ($exitCode -ne 0) {
            throw "Command failed with exit code $exitCode`: $FilePath $($ArgumentList -join ' ')"
        }
    }
    finally {
        Pop-Location
    }
}

function Reset-Directory {
    param([Parameter(Mandatory)][string]$Path)

    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }

    New-Item -ItemType Directory -Path $Path -Force | Out-Null
}

function Ensure-Directory {
    param([Parameter(Mandatory)][string]$Path)
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
}

function Get-WorkspaceMetadata {
    param([Parameter(Mandatory)][string]$Path)

    $text = Get-Content -LiteralPath $Path -Raw

    function Get-WorkspaceValue {
        param(
            [Parameter(Mandatory)][string]$Text,
            [Parameter(Mandatory)][string]$Name
        )

        $pattern =
            '(?ms)^\[workspace\.package\]\s.*?^' +
            [regex]::Escape($Name) +
            '\s*=\s*"(?<value>[^"]+)"'

        $match = [regex]::Match($Text, $pattern)

        if ($match.Success) {
            return $match.Groups['value'].Value
        }

        return $null
    }

    $version = Get-WorkspaceValue -Text $text -Name 'version'

    if (-not $version) {
        throw "Unable to read [workspace.package] version from $Path"
    }

    [pscustomobject]@{
        Version    = $version
        Repository = Get-WorkspaceValue -Text $text -Name 'repository'
        Homepage   = Get-WorkspaceValue -Text $text -Name 'homepage'
        License    = Get-WorkspaceValue -Text $text -Name 'license'
    }
}

function Convert-ToMsixVersion {
    param([Parameter(Mandatory)][string]$Version)

    $parts = $Version.Split('.')

    if ($parts.Count -gt 4) {
        throw "Version '$Version' cannot be represented as an MSIX version."
    }

    $result = @()

    foreach ($part in $parts) {
        if ($part -notmatch '^(?<number>\d+)') {
            throw "MSIX version components must start with digits: $Version"
        }

        $number = [int]$Matches['number']

        if ($number -lt 0 -or $number -gt 65535) {
            throw "MSIX version component must be 0-65535: $number"
        }

        $result += $number
    }

    while ($result.Count -lt 4) {
        $result += 0
    }

    return ($result -join '.')
}

function Get-CodeSigningCertificate {
    param([string]$Thumbprint)

    $certificates = @(
        Get-ChildItem Cert:\CurrentUser\My |
            Where-Object {
                $_.HasPrivateKey -and
                $_.NotBefore -le (Get-Date) -and
                $_.NotAfter -gt (Get-Date) -and
                ($_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3')
            }
    )

    if ($Thumbprint) {
        $normalizedThumbprint = ($Thumbprint -replace '\s', '').ToUpperInvariant()

        $certificate = $certificates |
            Where-Object {
                $_.Thumbprint.ToUpperInvariant() -eq $normalizedThumbprint
            } |
            Select-Object -First 1

        if (-not $certificate) {
            throw "No valid CurrentUser\My code-signing certificate was found with thumbprint '$Thumbprint'."
        }

        return $certificate
    }

    $matches = @(
        $certificates |
            Where-Object { $_.Subject -eq $MsixPublisher } |
            Sort-Object NotAfter -Descending
    )

    if ($matches.Count -eq 0) {
        throw @"
No valid code-signing certificate was found in Cert:\CurrentUser\My.

Required:
  Subject:     $MsixPublisher
  EKU:         Code Signing
  Private key: Yes
  Valid now:   Yes

Import the test PFX first, or pass -CertificateThumbprint.
"@
    }

    if ($matches.Count -gt 1) {
        Write-Host '    Multiple matching certificates found; using the one expiring latest.' -ForegroundColor Yellow
    }

    return $matches[0]
}

function Sign-File {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)]$Certificate
    )

    Require-File -Path $Path -Description 'File to sign'

    Invoke-Checked -FilePath $SignTool -ArgumentList @(
        'sign',
        '/fd', 'SHA256',
        '/sha1', $Certificate.Thumbprint,
        '/s', 'My',
        $Path
    )

    Invoke-Checked -FilePath $SignTool -ArgumentList @(
        'verify',
        '/pa',
        '/v',
        $Path
    )
}

function Copy-Payload {
    Reset-Directory $PayloadDir

    Copy-Item -LiteralPath $RustBinary -Destination (Join-Path $PayloadDir $BinaryName)
    Copy-Item -LiteralPath $Readme -Destination (Join-Path $PayloadDir 'README.md')
    Copy-Item -LiteralPath $Changelog -Destination (Join-Path $PayloadDir 'CHANGELOG.md')
    Copy-Item -LiteralPath $License -Destination (Join-Path $PayloadDir 'LICENSE')
}

function New-SquarePng {
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination,
        [Parameter(Mandatory)][int]$Size
    )

    Add-Type -AssemblyName System.Drawing

    $sourceImage = [System.Drawing.Image]::FromFile($Source)

    try {
        $bitmap = New-Object System.Drawing.Bitmap($Size, $Size)

        try {
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)

            try {
                $graphics.Clear([System.Drawing.Color]::Transparent)
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

                $ratio = [Math]::Min(
                    $Size / [double]$sourceImage.Width,
                    $Size / [double]$sourceImage.Height
                )

                $width  = [int]($sourceImage.Width * $ratio)
                $height = [int]($sourceImage.Height * $ratio)
                $x      = [int](($Size - $width) / 2)
                $y      = [int](($Size - $height) / 2)

                $graphics.DrawImage($sourceImage, $x, $y, $width, $height)
            }
            finally {
                $graphics.Dispose()
            }

            $bitmap.Save($Destination, [System.Drawing.Imaging.ImageFormat]::Png)
        }
        finally {
            $bitmap.Dispose()
        }
    }
    finally {
        $sourceImage.Dispose()
    }
}

function Write-WixSource {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Version
    )

    $payload = [System.Security.SecurityElement]::Escape($PayloadDir)

    $xml = @"
<?xml version="1.0" encoding="utf-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Package
      Name="$ProductName"
      Manufacturer="$Publisher"
      Version="$Version"
      Language="1033"
      Scope="perMachine"
      UpgradeCode="$MsiUpgradeCode">

    <SummaryInformation
        Description="CurseDelete command-line file deletion utility"
        Manufacturer="$Publisher" />

    <MajorUpgrade
        DowngradeErrorMessage="A newer version of $ProductName is already installed." />

    <MediaTemplate EmbedCab="yes" />

    <!-- Optional public property supplied on the msiexec command line. -->
    <Property Id="DEPLOYMENTKEY" Hidden="yes" Secure="yes" />

    <StandardDirectory Id="ProgramFiles64Folder">
      <Directory Id="RePassCloudFolder" Name="RePassCloud">
        <Directory Id="INSTALLFOLDER" Name="CurseDelete">

          <!--
            One file per component. This deliberately avoids WiX automatic-GUID
            restrictions for multi-file components.
          -->
          <Component Id="CursdelComponent" Guid="$ComponentGuidExe">
            <File
                Id="CursdelExe"
                Source="$payload\cursdel.exe"
                KeyPath="yes" />
          </Component>

          <Component Id="ReadmeComponent" Guid="$ComponentGuidReadme">
            <File
                Id="ReadmeFile"
                Source="$payload\README.md"
                KeyPath="yes" />
          </Component>

          <Component Id="ChangelogComponent" Guid="$ComponentGuidChangelog">
            <File
                Id="ChangelogFile"
                Source="$payload\CHANGELOG.md"
                KeyPath="yes" />
          </Component>

          <Component Id="LicenseComponent" Guid="$ComponentGuidLicense">
            <File
                Id="LicenseFile"
                Source="$payload\LICENSE"
                KeyPath="yes" />
          </Component>

          <!--
            Separate component for machine PATH. The registry value is the
            component key path; the environment entry is removed on uninstall.
          -->
          <Component Id="EnvironmentComponent" Guid="$ComponentGuidEnvironment">
            <RegistryValue
                Root="HKLM"
                Key="Software\RePassCloud\CurseDelete"
                Name="InstallPath"
                Type="string"
                Value="[INSTALLFOLDER]"
                KeyPath="yes" />

            <Environment
                Id="AddInstallFolderToMachinePath"
                Name="PATH"
                Action="set"
                Part="last"
                System="yes"
                Permanent="no"
                Value="[INSTALLFOLDER]" />
          </Component>

        </Directory>
      </Directory>
    </StandardDirectory>

    <Feature Id="MainFeature" Title="$ProductName" Level="1">
      <ComponentRef Id="CursdelComponent" />
      <ComponentRef Id="ReadmeComponent" />
      <ComponentRef Id="ChangelogComponent" />
      <ComponentRef Id="LicenseComponent" />
      <ComponentRef Id="EnvironmentComponent" />
    </Feature>

    <!--
      Optional post-install deployment enrollment.

      Example:
        msiexec /i CurseDelete.msi /qn DEPLOYMENTKEY="abcd1234"

      Deferred, non-impersonated execution means this runs elevated after the
      application files have been installed.
    -->
    <SetProperty
        Id="EnrollDeploymentKey"
        Value="&quot;[#CursdelExe]&quot; license enroll --deploymentkey=&quot;[DEPLOYMENTKEY]&quot;"
        Before="EnrollDeploymentKey"
        Sequence="execute" />

    <CustomAction
        Id="EnrollDeploymentKey"
        BinaryRef="Wix4UtilCA_`$(sys.BUILDARCHSHORT)"
        DllEntry="WixSilentExec"
        Execute="deferred"
        Impersonate="no"
        Return="check"
        HideTarget="yes" />

    <!-- Grant BUILTIN\Users Modify to JSON files that exist after enrollment. -->
    <SetProperty
        Id="SetJsonAcl"
        Value="&quot;[%ComSpec]&quot; /d /s /c if exist &quot;[INSTALLFOLDER]*.json&quot; icacls &quot;[INSTALLFOLDER]*.json&quot; /grant *S-1-5-32-545:(M) /C"
        Before="SetJsonAcl"
        Sequence="execute" />

    <CustomAction
        Id="SetJsonAcl"
        BinaryRef="Wix4UtilCA_`$(sys.BUILDARCHSHORT)"
        DllEntry="WixSilentExec"
        Execute="deferred"
        Impersonate="no"
        Return="check" />

    <InstallExecuteSequence>
      <Custom
          Action="EnrollDeploymentKey"
          After="InstallFiles"
          Condition="NOT Installed AND DEPLOYMENTKEY &lt;&gt; &quot;&quot;" />

      <Custom
          Action="SetJsonAcl"
          After="EnrollDeploymentKey"
          Condition="NOT Installed" />
    </InstallExecuteSequence>

  </Package>
</Wix>
"@

    Set-Content -LiteralPath $Path -Value $xml -Encoding UTF8
}

function Write-InnoSource {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Version,
        [string]$Repository
    )

    $escapedOutput  = $OutputDir
    $escapedPayload = $PayloadDir
    $escapedIcon    = $IconIco

    $iss = @"
; Generated by build-release.ps1. Do not edit this temporary file.

[Setup]
AppId={{A690B423-3227-47F0-B898-734A1606B704}
AppName=$ProductName
AppVersion=$Version
AppPublisher=$Publisher
AppPublisherURL=$Repository
AppSupportURL=$Repository
AppUpdatesURL=$Repository
DefaultDirName={code:GetInstallDir}
DefaultGroupName=$ProductName
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=$escapedOutput
OutputBaseFilename=CurseDelete-v$Version-win-x64-setup
SetupIconFile=$escapedIcon
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no
UninstallDisplayName=$ProductName
VersionInfoCompany=$Publisher
VersionInfoDescription=CurseDelete installer
VersionInfoProductName=$ProductName
VersionInfoProductVersion=$Version

[Files]
Source: "$escapedPayload\cursdel.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "$escapedPayload\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "$escapedPayload\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "$escapedPayload\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Run]
; Optional deployment enrollment after installation.
Filename: "{app}\cursdel.exe"; Parameters: "license enroll --deploymentkey=""{code:GetDeploymentKey}"""; WorkingDir: "{app}"; Flags: runhidden waituntilterminated; Check: HasDeploymentKey

; Apply Users=Modify to JSON files created during enrollment.
Filename: "{cmd}"; Parameters: "/d /s /c if exist ""{app}\*.json"" icacls ""{app}\*.json"" /grant *S-1-5-32-545:(M) /C"; Flags: runhidden waituntilterminated; Check: HasJsonFiles

[Code]
const
  DefaultInstallPath = '$DefaultInstallDir';

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
      begin
        Result := Copy(Result, 2, Length(Result) - 2);
      end;

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

function GetDeploymentKey(Param: String): String;
begin
  Result := GetNamedParameter('deploymentkey');
end;

function HasDeploymentKey: Boolean;
begin
  Result := GetDeploymentKey('') <> '';
end;

function HasJsonFiles: Boolean;
var
  FindRec: TFindRec;
begin
  Result := FindFirst(ExpandConstant('{app}\*.json'), FindRec);

  if Result then
    FindClose(FindRec);
end;

function NormalizePathEntry(const Value: String): String;
begin
  Result := LowerCase(Trim(Value));

  while (Length(Result) > 3) and
        (Result[Length(Result)] = '\') do
  begin
    Delete(Result, Length(Result), 1);
  end;
end;

function PathContains(const PathValue, Entry: String): Boolean;
var
  Remaining: String;
  Item: String;
  SeparatorPos: Integer;
  NormalizedEntry: String;
begin
  Result := False;
  Remaining := PathValue;
  NormalizedEntry := NormalizePathEntry(Entry);

  while Remaining <> '' do
  begin
    SeparatorPos := Pos(';', Remaining);

    if SeparatorPos > 0 then
    begin
      Item := Copy(Remaining, 1, SeparatorPos - 1);
      Delete(Remaining, 1, SeparatorPos);
    end
    else
    begin
      Item := Remaining;
      Remaining := '';
    end;

    if NormalizePathEntry(Item) = NormalizedEntry then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

function RemovePathEntry(const PathValue, Entry: String): String;
var
  Remaining: String;
  Item: String;
  SeparatorPos: Integer;
  NormalizedEntry: String;
begin
  Result := '';
  Remaining := PathValue;
  NormalizedEntry := NormalizePathEntry(Entry);

  while Remaining <> '' do
  begin
    SeparatorPos := Pos(';', Remaining);

    if SeparatorPos > 0 then
    begin
      Item := Copy(Remaining, 1, SeparatorPos - 1);
      Delete(Remaining, 1, SeparatorPos);
    end
    else
    begin
      Item := Remaining;
      Remaining := '';
    end;

    if (Trim(Item) <> '') and
       (NormalizePathEntry(Item) <> NormalizedEntry) then
    begin
      if Result <> '' then
        Result := Result + ';';

      Result := Result + Item;
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
  begin
    CurrentPath := '';
  end;

  if not PathContains(CurrentPath, Entry) then
  begin
    if (CurrentPath <> '') and
       (CurrentPath[Length(CurrentPath)] <> ';') then
    begin
      NewPath := CurrentPath + ';' + Entry;
    end
    else
    begin
      NewPath := CurrentPath + Entry;
    end;

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
  NewPath: String;
begin
  if not RegQueryStringValue(
      HKLM64,
      'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
      'Path',
      CurrentPath) then
  begin
    Exit;
  end;

  NewPath := RemovePathEntry(CurrentPath, Entry);

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
"@

    Set-Content -LiteralPath $Path -Value $iss -Encoding UTF8
}

function Write-MsixManifest {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Version
    )

    $manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
  xmlns:uap10="http://schemas.microsoft.com/appx/manifest/uap/windows10/10"
  xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  IgnorableNamespaces="uap uap3 uap10 desktop rescap">

  <Identity
      Name="$MsixIdentityName"
      Publisher="$MsixPublisher"
      Version="$Version"
      ProcessorArchitecture="x64" />

  <Properties>
    <DisplayName>$ProductName</DisplayName>
    <PublisherDisplayName>$Publisher</PublisherDisplayName>
    <Description>CurseDelete command-line file deletion utility</Description>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>

  <Resources>
    <Resource Language="en-us" />
  </Resources>

  <Dependencies>
    <TargetDeviceFamily
        Name="Windows.Desktop"
        MinVersion="10.0.19041.0"
        MaxVersionTested="10.0.26100.0" />
  </Dependencies>

  <Applications>
    <Application
        Id="CurseDelete"
        Executable="cursdel.exe"
        uap10:RuntimeBehavior="packagedClassicApp"
        uap10:TrustLevel="mediumIL">

      <uap:VisualElements
          DisplayName="$ProductName"
          Description="CurseDelete command-line file deletion utility"
          BackgroundColor="transparent"
          Square44x44Logo="Assets\Square44x44Logo.png"
          Square150x150Logo="Assets\Square150x150Logo.png" />

      <Extensions>
        <uap3:Extension
            Category="windows.appExecutionAlias"
            Executable="cursdel.exe"
            EntryPoint="Windows.FullTrustApplication">
          <uap3:AppExecutionAlias>
            <desktop:ExecutionAlias Alias="cursdel.exe" />
          </uap3:AppExecutionAlias>
        </uap3:Extension>
      </Extensions>

    </Application>
  </Applications>

  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>

</Package>
"@

    Set-Content -LiteralPath $Path -Value $manifest -Encoding UTF8
}

# =============================================================================
# Validate repository + tools
# =============================================================================

Write-Step 'Validating repository'

foreach ($item in @(
    @{ Path = $CargoToml; Description = 'Cargo.toml' },
    @{ Path = $CargoLock; Description = 'Cargo.lock' },
    @{ Path = $Readme; Description = 'README.md' },
    @{ Path = $Changelog; Description = 'CHANGELOG.md' },
    @{ Path = $License; Description = 'LICENSE' },
    @{ Path = $IconIco; Description = 'Windows icon' },
    @{ Path = $IconPng; Description = 'Master PNG icon' }
)) {
    Require-File -Path $item.Path -Description $item.Description
}

$metadata    = Get-WorkspaceMetadata -Path $CargoToml
$Version     = $metadata.Version
$MsixVersion = Convert-ToMsixVersion -Version $Version

Write-Host "    Project:      $ProjectRoot"
Write-Host "    Version:      $Version"
Write-Host "    MSIX version: $MsixVersion"
Write-Host "    Output:       $OutputDir"

Write-Step 'Validating build tools'

foreach ($tool in @(
    @{ Path = $Cargo; Description = 'Cargo' },
    @{ Path = $Wix; Description = 'WiX' },
    @{ Path = $InnoCompiler; Description = 'Inno Setup compiler' },
    @{ Path = $MakeAppx; Description = 'MakeAppx' },
    @{ Path = $SignTool; Description = 'SignTool' },
    @{ Path = $SevenZip; Description = '7-Zip' }
)) {
    Require-File -Path $tool.Path -Description $tool.Description
}

Write-Host "    cargo:    $Cargo"
Write-Host "    wix:      $Wix"
Write-Host "    ISCC:     $InnoCompiler"
Write-Host "    makeappx: $MakeAppx"
Write-Host "    signtool: $SignTool"
Write-Host "    7z:       $SevenZip"

# =============================================================================
# Prepare output
# =============================================================================

Write-Step 'Preparing output directories'

if (-not $SkipClean) {
    Reset-Directory $OutputDir
}
else {
    Ensure-Directory $OutputDir
}

Reset-Directory $WorkDir
Ensure-Directory $WixWorkDir
Ensure-Directory $InnoWorkDir
Ensure-Directory $MsixWorkDir

# =============================================================================
# Rust build
# =============================================================================

if (-not $SkipRustBuild) {
    Write-Step 'Building x64 Rust release binary'

    Invoke-Checked -FilePath $Cargo -ArgumentList @(
        'build',
        '--release',
        '--locked',
        '--package', $CargoPackage
    )
}

Require-File -Path $RustBinary -Description 'Rust release binary'

# =============================================================================
# Signing certificate + executable
# =============================================================================

$Certificate = $null

if (-not $SkipSigning) {
    Write-Step 'Selecting local test code-signing certificate'

    $Certificate = Get-CodeSigningCertificate -Thumbprint $CertificateThumbprint

    Write-Host "    Subject:    $($Certificate.Subject)"
    Write-Host "    Thumbprint: $($Certificate.Thumbprint)"
    Write-Host "    Expires:    $($Certificate.NotAfter)"

    Write-Step 'Signing cursdel.exe before packaging'
    Sign-File -Path $RustBinary -Certificate $Certificate
}

# =============================================================================
# Common payload
# =============================================================================

Write-Step 'Staging common payload'
Copy-Payload

# =============================================================================
# Portable ZIP + 7z
# =============================================================================

Write-Step 'Creating portable archives'

$PortableBase = "CurseDelete-v$Version-win-x64-portable"
$ZipPath      = Join-Path $OutputDir "$PortableBase.zip"
$SevenZipPath = Join-Path $OutputDir "$PortableBase.7z"

Compress-Archive `
    -Path (Join-Path $PayloadDir '*') `
    -DestinationPath $ZipPath `
    -CompressionLevel Optimal `
    -Force

Invoke-Checked `
    -FilePath $SevenZip `
    -WorkingDirectory $PayloadDir `
    -ArgumentList @(
        'a',
        '-t7z',
        '-mx=9',
        $SevenZipPath,
        '.\*'
    )

# =============================================================================
# WiX MSI
# =============================================================================

Write-Step 'Building WiX MSI'

# Use the global per-user extension cache consistently.
$extensionList = @(& $Wix extension list -g 2>$null)

if (
    $LASTEXITCODE -ne 0 -or
    (($extensionList -join "`n") -notmatch 'WixToolset\.Util\.wixext')
) {
    Invoke-Checked -FilePath $Wix -ArgumentList @(
        'extension',
        'add',
        '-g',
        'WixToolset.Util.wixext'
    )
}

$WixSource = Join-Path $WixWorkDir 'Package.wxs'
$MsiPath   = Join-Path $OutputDir "CurseDelete-v$Version-win-x64.msi"

Write-WixSource -Path $WixSource -Version $Version

Invoke-Checked `
    -FilePath $Wix `
    -WorkingDirectory $WixWorkDir `
    -ArgumentList @(
        'build',
        $WixSource,
        '-arch', 'x64',
        '-ext', 'WixToolset.Util.wixext',
        '-o', $MsiPath
    )

Require-File -Path $MsiPath -Description 'WiX MSI output'

if (-not $SkipSigning) {
    Write-Step 'Signing MSI'
    Sign-File -Path $MsiPath -Certificate $Certificate
}

# =============================================================================
# Inno Setup EXE
# =============================================================================

Write-Step 'Building Inno Setup EXE'

$InnoSource  = Join-Path $InnoWorkDir 'CurseDelete.iss'
$SetupExePath = Join-Path $OutputDir "CurseDelete-v$Version-win-x64-setup.exe"

Write-InnoSource `
    -Path $InnoSource `
    -Version $Version `
    -Repository $metadata.Repository

Invoke-Checked `
    -FilePath $InnoCompiler `
    -WorkingDirectory $InnoWorkDir `
    -ArgumentList @($InnoSource)

Require-File -Path $SetupExePath -Description 'Inno Setup output'

if (-not $SkipSigning) {
    Write-Step 'Signing Setup.exe'
    Sign-File -Path $SetupExePath -Certificate $Certificate
}

# =============================================================================
# MSIX
# =============================================================================

Write-Step 'Building MSIX'

Reset-Directory $MsixWorkDir

Copy-Item -LiteralPath (Join-Path $PayloadDir 'cursdel.exe') -Destination $MsixWorkDir
Copy-Item -LiteralPath (Join-Path $PayloadDir 'README.md') -Destination $MsixWorkDir
Copy-Item -LiteralPath (Join-Path $PayloadDir 'CHANGELOG.md') -Destination $MsixWorkDir
Copy-Item -LiteralPath (Join-Path $PayloadDir 'LICENSE') -Destination $MsixWorkDir

$AssetsDir = Join-Path $MsixWorkDir 'Assets'
Ensure-Directory $AssetsDir

New-SquarePng `
    -Source $IconPng `
    -Destination (Join-Path $AssetsDir 'Square44x44Logo.png') `
    -Size 44

New-SquarePng `
    -Source $IconPng `
    -Destination (Join-Path $AssetsDir 'Square150x150Logo.png') `
    -Size 150

New-SquarePng `
    -Source $IconPng `
    -Destination (Join-Path $AssetsDir 'StoreLogo.png') `
    -Size 50

$ManifestPath = Join-Path $MsixWorkDir 'AppxManifest.xml'
$MsixPath     = Join-Path $OutputDir "CurseDelete-v$Version-win-x64.msix"

Write-MsixManifest -Path $ManifestPath -Version $MsixVersion

Invoke-Checked `
    -FilePath $MakeAppx `
    -WorkingDirectory $MsixWorkDir `
    -ArgumentList @(
        'pack',
        '/o',
        '/d', $MsixWorkDir,
        '/p', $MsixPath
    )

Require-File -Path $MsixPath -Description 'MSIX output'

if (-not $SkipSigning) {
    Write-Step 'Signing MSIX'
    Sign-File -Path $MsixPath -Certificate $Certificate
}

# =============================================================================
# SHA-256 checksums
# =============================================================================

Write-Step 'Generating SHA-256 checksums'

$ArtifactFiles = @(
    $SetupExePath,
    $MsiPath,
    $MsixPath,
    $ZipPath,
    $SevenZipPath
)

$ChecksumPath = Join-Path $OutputDir 'SHA256SUMS.txt'

$checksumLines = foreach ($artifact in $ArtifactFiles) {
    Require-File -Path $artifact -Description 'Release artifact'

    $hash = Get-FileHash -LiteralPath $artifact -Algorithm SHA256

    '{0}  {1}' -f `
        $hash.Hash.ToLowerInvariant(), `
        (Split-Path -Leaf $artifact)
}

Set-Content `
    -LiteralPath $ChecksumPath `
    -Value $checksumLines `
    -Encoding ASCII

# =============================================================================
# Final report
# =============================================================================

Write-Step 'Build complete'

Get-ChildItem -LiteralPath $OutputDir -File |
    Sort-Object Name |
    Select-Object `
        Name,
        @{ N = 'SizeMB'; E = { [Math]::Round($_.Length / 1MB, 2) } } |
    Format-Table -AutoSize

Write-Host @"

Output:
  $OutputDir

Installer examples:

  Inno interactive:
    .\CurseDelete-v$Version-win-x64-setup.exe

  Inno silent:
    .\CurseDelete-v$Version-win-x64-setup.exe /SILENT

  Inno silent + deployment:
    .\CurseDelete-v$Version-win-x64-setup.exe /SILENT /deploymentkey="abcd1234"

  Inno custom directory:
    .\CurseDelete-v$Version-win-x64-setup.exe /installdir="D:\Apps\CurseDelete"

  MSI interactive:
    msiexec.exe /i "CurseDelete-v$Version-win-x64.msi"

  MSI silent:
    msiexec.exe /i "CurseDelete-v$Version-win-x64.msi" /qn

  MSI silent + deployment:
    msiexec.exe /i "CurseDelete-v$Version-win-x64.msi" /qn DEPLOYMENTKEY="abcd1234"

  MSI custom directory:
    msiexec.exe /i "CurseDelete-v$Version-win-x64.msi" /qn INSTALLFOLDER="D:\Apps\CurseDelete"

  MSIX:
    Add-AppxPackage ".\CurseDelete-v$Version-win-x64.msix"

  MSIX deployment enrollment (run after install):
    cursdel.exe license enroll --deploymentkey=abcd1234

"@ -ForegroundColor Green
