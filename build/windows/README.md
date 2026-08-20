# Windows packaging

Run the entire x64 Windows release build from the repository root:

```powershell
.\build\windows\build-release.ps1
```

Outputs are written to:

```text
dist\windows\
```

The script builds and signs `cursdel.exe`, then creates:

- Inno Setup `.exe`
- WiX `.msi`
- signed `.msix`
- portable `.zip`
- portable `.7z`

## Required local tools

- Rust / Cargo
- WiX Toolset (`wix.exe` in PATH)
- Inno Setup (`ISCC.exe`; default path is auto-detected)
- Windows SDK (`makeappx.exe` and `signtool.exe` in PATH)
- 7-Zip at `C:\Program Files\7-Zip\7z.exe`

## Test signing certificate

The local build uses the already-imported code-signing certificate in:

```text
Cert:\CurrentUser\My
```

It looks for a valid certificate with:

```text
Subject = CN=RePassCloud
HasPrivateKey = True
Code Signing EKU
```

You can override selection with:

```powershell
.\build\windows\build-release.ps1 -CertificateThumbprint "THUMBPRINT"
```

The PFX in `certs\` is not required for local builds once imported. Keep the PFX out of Git.

## Installer usage

### Inno Setup EXE

Interactive:

```powershell
.\CurseDelete-v2.0.0-win-x64-setup.exe
```

Silent:

```powershell
.\CurseDelete-v2.0.0-win-x64-setup.exe /SILENT
```

Silent with custom path and deployment enrollment:

```powershell
.\CurseDelete-v2.0.0-win-x64-setup.exe `
  /SILENT `
  /installdir="D:\Apps\CurseDelete" `
  /deploymentkey="abcd1234"
```

Inno Setup natively uses `/SILENT` or `/VERYSILENT`; `/s` is not used.

### MSI

Interactive:

```powershell
msiexec.exe /i .\CurseDelete-v2.0.0-win-x64.msi
```

Silent:

```powershell
msiexec.exe /i .\CurseDelete-v2.0.0-win-x64.msi /qn
```

Silent with custom path and deployment enrollment:

```powershell
msiexec.exe /i .\CurseDelete-v2.0.0-win-x64.msi `
  /qn `
  INSTALLFOLDER="D:\Apps\CurseDelete" `
  DEPLOYMENTKEY="abcd1234"
```

### MSIX

MSIX installs into a Windows-managed package location and exposes `cursdel.exe`
through an application execution alias.

MSIX cannot implement the MSI/Inno arbitrary `/deploymentkey` post-install
workflow. Enroll after installation:

```powershell
cursdel.exe license enroll --deploymentkey=abcd1234
```

## JSON ACL note

The MSI and Inno installers run:

```text
icacls "<install-dir>\*.json" /grant BUILTIN\Users:(M)
```

after optional enrollment, so JSON files that exist at that time receive
machine-wide Users Modify permission.

Windows ACL inheritance cannot automatically apply a different ACL based on a
future file's extension. If CurseDelete creates additional JSON files later,
the application should set that ACL when creating them, or mutable machine
state should be moved to `%ProgramData%\RePassCloud\CurseDelete`.
