#requires -RunAsAdministrator
#requires -Version 5.1

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ============================================================================
# Configuration
# ============================================================================

$ProjectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$CertDir     = Join-Path $ProjectRoot 'certs'

$Subject      = 'CN=RePassCloud'
$FriendlyName = 'CurseDelete Development Code Signing'

# INTERNAL TESTING ONLY.
# Do not reuse this password for anything real.
$PfxPasswordPlainText = 'CurseDelete-Test-Only-2026!'

$PfxPath = Join-Path $CertDir 'cursedelete-dev-signing.pfx'
$CerPath = Join-Path $CertDir 'cursedelete-dev-signing.cer'

# ============================================================================
# Helpers
# ============================================================================

function Write-Step {
    param([string]$Message)

    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Remove-ExistingCertificate {
    param(
        [string]$StorePath,
        [string]$Subject,
        [string]$FriendlyName
    )

    Get-ChildItem $StorePath -ErrorAction SilentlyContinue |
        Where-Object {
            $_.Subject -eq $Subject -or
            $_.FriendlyName -eq $FriendlyName
        } |
        ForEach-Object {
            Write-Host "    Removing existing certificate: $($_.Thumbprint)"
            Remove-Item -LiteralPath $_.PSPath -Force
        }
}

# ============================================================================
# Create certs directory
# ============================================================================

Write-Step 'Creating certificate directory'

New-Item `
    -ItemType Directory `
    -Path $CertDir `
    -Force |
    Out-Null

# Remove old exported files so we know exactly what was created this run.
Remove-Item $PfxPath -Force -ErrorAction SilentlyContinue
Remove-Item $CerPath -Force -ErrorAction SilentlyContinue

# ============================================================================
# Remove old development certificates
# ============================================================================

Write-Step 'Removing previous CurseDelete test certificates'

Remove-ExistingCertificate `
    -StorePath 'Cert:\CurrentUser\My' `
    -Subject $Subject `
    -FriendlyName $FriendlyName

Remove-ExistingCertificate `
    -StorePath 'Cert:\LocalMachine\TrustedPeople' `
    -Subject $Subject `
    -FriendlyName $FriendlyName

# ============================================================================
# Create code-signing certificate
# ============================================================================

Write-Step 'Creating self-signed code-signing certificate'

$cert = New-SelfSignedCertificate `
    -Type Custom `
    -Subject $Subject `
    -FriendlyName $FriendlyName `
    -CertStoreLocation 'Cert:\CurrentUser\My' `
    -KeyAlgorithm RSA `
    -KeyLength 3072 `
    -HashAlgorithm SHA256 `
    -KeyExportPolicy Exportable `
    -KeyUsage DigitalSignature `
    -NotAfter (Get-Date).AddYears(5) `
    -TextExtension @(
        '2.5.29.37={text}1.3.6.1.5.5.7.3.3',
        '2.5.29.19={text}'
    )

if (-not $cert) {
    throw 'Certificate creation failed.'
}

Write-Host "    Subject:    $($cert.Subject)"
Write-Host "    Thumbprint: $($cert.Thumbprint)"
Write-Host "    Expires:    $($cert.NotAfter)"

# ============================================================================
# Export PFX
# ============================================================================

Write-Step 'Exporting PFX with private key'

$securePassword = ConvertTo-SecureString `
    $PfxPasswordPlainText `
    -AsPlainText `
    -Force

Export-PfxCertificate `
    -Cert $cert `
    -FilePath $PfxPath `
    -Password $securePassword `
    -Force |
    Out-Null

# ============================================================================
# Export public CER
# ============================================================================

Write-Step 'Exporting public certificate'

Export-Certificate `
    -Cert $cert `
    -FilePath $CerPath `
    -Force |
    Out-Null

# ============================================================================
# Import public certificate into TrustedPeople
# ============================================================================

Write-Step 'Trusting certificate in LocalMachine\TrustedPeople'

$trustedCert = Import-Certificate `
    -FilePath $CerPath `
    -CertStoreLocation 'Cert:\LocalMachine\TrustedPeople'

if (-not $trustedCert) {
    throw 'Failed to import certificate into LocalMachine\TrustedPeople.'
}

# ============================================================================
# Verify private certificate
# ============================================================================

Write-Step 'Verifying private certificate'

$privateCert = Get-ChildItem 'Cert:\CurrentUser\My' |
    Where-Object {
        $_.Thumbprint -eq $cert.Thumbprint
    } |
    Select-Object -First 1

if (-not $privateCert) {
    throw 'Certificate not found in CurrentUser\My.'
}

if (-not $privateCert.HasPrivateKey) {
    throw 'Certificate exists but does not contain a private key.'
}

$hasCodeSigningEku =
    $privateCert.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3'

if (-not $hasCodeSigningEku) {
    throw 'Certificate does not contain the Code Signing EKU.'
}

# ============================================================================
# Verify trusted certificate
# ============================================================================

Write-Step 'Verifying trusted certificate'

$trusted = Get-ChildItem 'Cert:\LocalMachine\TrustedPeople' |
    Where-Object {
        $_.Thumbprint -eq $cert.Thumbprint
    } |
    Select-Object -First 1

if (-not $trusted) {
    throw 'Certificate not found in LocalMachine\TrustedPeople.'
}

# ============================================================================
# Verify exported files
# ============================================================================

Write-Step 'Verifying exported certificate files'

if (-not (Test-Path -LiteralPath $PfxPath)) {
    throw "PFX was not created: $PfxPath"
}

if (-not (Test-Path -LiteralPath $CerPath)) {
    throw "CER was not created: $CerPath"
}

# ============================================================================
# Final output
# ============================================================================

Write-Step 'Certificate setup complete'

Write-Host @"

Certificate:
  Subject:     $($cert.Subject)
  Thumbprint:  $($cert.Thumbprint)
  Friendly:    $FriendlyName
  Expires:     $($cert.NotAfter)

Certificate stores:
  Private key:
    Cert:\CurrentUser\My\$($cert.Thumbprint)

  Trusted public certificate:
    Cert:\LocalMachine\TrustedPeople\$($cert.Thumbprint)

Exported files:
  $PfxPath
  $CerPath

PFX password:
  $PfxPasswordPlainText

Your build-release.ps1 should now be able to automatically find:
  Subject = CN=RePassCloud
  EKU     = Code Signing
  Private key present

"@ -ForegroundColor Green
