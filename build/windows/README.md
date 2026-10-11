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
.\CurseDelete-v2.1.0-win-x64-setup.exe
```

Silent:

```powershell
.\CurseDelete-v2.1.0-win-x64-setup.exe /SILENT
```

Silent with custom path and a licence (see [Licensing at install time](#licensing-at-install-time)):

```powershell
.\CurseDelete-v2.1.0-win-x64-setup.exe `
  /SILENT `
  /installdir="D:\Apps\CurseDelete" `
  /LICENSEFILE="C:\Deploy\cursedelete.license"
```

Inno Setup natively uses `/SILENT` or `/VERYSILENT`; `/s` is not used.

### MSI

Interactive:

```powershell
msiexec.exe /i .\CurseDelete-v2.1.0-win-x64.msi
```

Silent:

```powershell
msiexec.exe /i .\CurseDelete-v2.1.0-win-x64.msi /qn
```

Silent with custom path and a licence:

```powershell
msiexec.exe /i .\CurseDelete-v2.1.0-win-x64.msi `
  /qn `
  INSTALLFOLDER="D:\Apps\CurseDelete" `
  ACTIVATIONKEY="XXXX-XXXX"
```

### MSIX

MSIX installs into a Windows-managed package location and exposes `cursdel.exe`
through an application execution alias.

MSIX cannot run post-install commands. License after installation from an
elevated prompt:

```powershell
cursdel.exe license install --system C:\Deploy\cursedelete.license
# or
cursdel.exe license activate --system XXXX-XXXX
```

## Licensing at install time

| Inno Setup | MSI property | Runs (as admin/SYSTEM) | Use for |
|---|---|---|---|
| `/LICENSEFILE="<path>"` | `LICENSEFILE="<absolute path>"` | `cursdel license install --system <path>` | Community, Education, Enterprise licence files; offline, no device data |
| `/ACTIVATIONKEY="<key>"` | `ACTIVATIONKEY="<key>"` | `cursdel license activate --system <key>` | Online activation: Business (registers this machine) or online provisioning of other editions |

Both write to the machine-wide licence store,
`%ProgramData%\RePassCloud\CurseDelete\v2\`, so every user on the machine
gets the licence. See [docs/LICENSING.md](../../docs/LICENSING.md).

- The licence file is validated before anything is written. A licensing
  problem never fails the installation (MSI `Return="ignore"`), and
  CurseDelete then runs as Community. Verify afterwards with
  `cursdel license status`.
- `ACTIVATIONKEY` needs the licensing server URL baked into the release
  build (`CURSDEL_LICENSE_SERVER_URL` set at compile time), because the
  installer's process environment can't supply one. The MSI hides the key
  from action logging (`HideTarget`). Inno includes `[Run]` parameters in
  its log only when run with `/LOG`.
- Nothing is written into the installation directory. A machine-wide
  Business activation's client secret (`credentials.json`) is restricted to
  SYSTEM and Administrators, so `license deactivate --system` and
  `license renew --system` need an elevated prompt.
- Administrators can also copy a licence file into that directory by hand
  (`*.license` or `*.json`); CurseDelete picks it up automatically.
