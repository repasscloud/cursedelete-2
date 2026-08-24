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
cursdel.exe license enroll --deployment-key=abcd1234
```

## Machine-wide license storage and ACLs

`cursdel license enroll` (whether invoked directly, via the MSI's
`DEPLOYMENTKEY` property, or via the Inno installer's `/deploymentkey`
switch) writes `license.json` and `activation.json` to
`%ProgramData%\CurseDelete-2\` -- **not** the install directory
(`{app}`/`INSTALLFOLDER`). `cursdel` applies its own permissions when
writing those files: `license.json` inherits `%ProgramData%`'s normal
Users-readable ACL (any local account running `cursdel` needs to read it),
while `activation.json` -- the bearer credential -- is explicitly
restricted to `BUILTIN\Administrators` and `SYSTEM` via `icacls` (see
`cursdel_license::store::save_machine_activation_credentials`). Earlier
revisions of this installer applied their own `icacls ... /grant
BUILTIN\Users:(M)` step targeting the install directory; that step never
actually matched where the JSON files are written and has been removed --
`cursdel` itself is now the single source of truth for these permissions.
