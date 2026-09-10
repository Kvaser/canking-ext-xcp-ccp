<#
.SYNOPSIS
    Signs a CanKing extension bundle (.ckext) file with a single combined signature.

.DESCRIPTION
    This script takes an unsigned .ckext bundle file, extracts it, creates a combined signature
    for payload+metadata using signtool (detached PKCS#7), and repackages everything into a signed bundle.

    The signature protects both the payload (.wasm or .tgz) and metadata (ck-ext-info.json) as a single unit,
    preventing tampering or extraction/repackaging of licensed extensions.

.PARAMETER InputBundle
    Path to the unsigned .ckext bundle file.

.PARAMETER OutputBundle
    Path where the signed bundle should be saved. If not specified, will overwrite the input bundle.

.PARAMETER ProductId
    Product ID to add to metadata. Optional.

.PARAMETER Ean
    EAN code to add to metadata. Optional.

.PARAMETER LicenseType
    License type to add to metadata. Valid values: Free, Commercial, OpenSource. Default: Commercial

.PARAMETER RequiresActivation
    Set to $true if the extension requires license activation. Default: $false

.PARAMETER CopyrightNotice
    Copyright notice to add to metadata. Default: "© [Year] Kvaser AB. All rights reserved."

.PARAMETER Eula
    End User License Agreement. Optional. Default: "See CanKing EULA."

.EXAMPLE
    .\Sign-CkextBundle.ps1 -InputBundle "myextension.ckext"

.EXAMPLE
    .\Sign-CkextBundle.ps1 -InputBundle "myextension.ckext" -OutputBundle "myextension-signed.ckext" -LicenseType Commercial -RequiresActivation $true
#>

param(
  [Parameter(Mandatory = $true)]
  [ValidateScript({ Test-Path $_ -PathType Container })]
  [string]$InputBundle,

  [Parameter(Mandatory = $false)]
  [string]$OutputBundle,

  [Parameter(Mandatory = $false)]
  [string]$ProductId,

  [Parameter(Mandatory = $false)]
  [string]$Ean,

  [Parameter(Mandatory = $false)]
  [ValidateSet("Free", "Commercial", "OpenSource")]
  [string]$LicenseType = "Commercial",

  [Parameter(Mandatory = $false)]
  [bool]$RequiresActivation = $false,

  [Parameter(Mandatory = $false)]
  [string]$CopyrightNotice,

  [Parameter(Mandatory = $false)]
  [string]$Eula = "See CanKing EULA."
)

# Function to get certificate subject name
<# function Get-CertificateSubject {
  try {
    # Try to find certificate in stores
    $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.SubjectName -eq "Kvaser AB" } | Select-Object -First 1
    if (-not $cert) {
      $cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object { $_.SubjectName -eq "Kvaser AB" } | Select-Object -First 1
    }

    if ($cert) {
      return $cert.Subject
    }
    else {
      Write-Warning "Could not find certificate with subject 'Kvaser AB' in certificate stores"
      return "CN=Unknown Publisher"
    }
  }
  catch {
    Write-Warning "Error reading certificate: $($_.Exception.Message)"
    return "CN=Unknown Publisher"
  }

  return "CN=Unknown Publisher"
} #>

# Function to sign a file with detached signature
function Sign-FileDetached {
  param(
    [string]$FilePath
  )

  Write-Host "Signing: $([System.IO.Path]::GetFileName($FilePath))" -ForegroundColor Cyan

  $timestampServers = @(
    "http://timestamp.digicert.com"
    #"http://timestamp.sectigo.com",
    #"http://timestamp.globalsign.com/scripts/timstamp.dll"
  )

  $baseArgs = @(
    "sign",
    "/s", "My",
    "/n", "Kvaser AB",
    "/fd", "SHA256",
    "/p7", (Split-Path $FilePath -Parent),
    "/p7co", "1.2.840.113549.1.7.2", # PKCS#7 SignedData
    "/p7ce", "DetachedSignedData"
  )

  foreach ($tsServer in $timestampServers) {
    $signArgs = $baseArgs + @("/tr", $tsServer) + @("/td", "SHA256") + $FilePath

    Write-Verbose "Executing: signtool $($signArgs -join ' ')"
    Write-Host "  Trying timestamp server: $tsServer" -ForegroundColor DarkGray

    $result = & signtool @signArgs 2>&1

    if ($LASTEXITCODE -eq 0) {
      Write-Host "  ? Signature created successfully" -ForegroundColor Green
      return
    }

    Write-Warning "Timestamp server failed ($tsServer): $result"
  }

  # Fallback: sign without a timestamp.
  #
  # Public TSAs (DigiCert, Sectigo, GlobalSign) are designed for Authenticode (PE files like .dll/.exe).
  # When signing arbitrary data with /p7ce DetachedSignedData they reject the request, even though the
  # same TSA works fine for .dll signing on the same machine.
  #
  # A timestamp is not strictly required for the signature to be cryptographically valid - it only
  # matters for long-term verification after the signing certificate expires. Since this is an internal
  # pipeline and we control the verifier, signing without a timestamp is acceptable.
  #
  # If a proper timestamp is needed in the future, options are:
  #   - Use a self-hosted TSA that supports general CMS/PKCS#7 (e.g. OpenSSL-based)
  #   - Switch to a manifest-based signing approach (hash files -> sign manifest -> verify hashes)
  Write-Warning "All timestamp servers failed (they do not support detached PKCS#7 for arbitrary data). Signing without timestamp..."

  $signArgs = $baseArgs + $FilePath
  Write-Verbose "Executing: signtool $($signArgs -join ' ')"
  $result = & signtool @signArgs 2>&1

  if ($LASTEXITCODE -eq 0) {
    Write-Host "  ? Signature created successfully (no timestamp)" -ForegroundColor Yellow
    return
  }

  Write-Error "Failed to sign $FilePath"
  Write-Error "$result"
  throw "Signing failed with exit code $LASTEXITCODE"
}

# Main script
try {
  Write-Host "CanKing Extension Bundle Signer" -ForegroundColor Yellow
  Write-Host "================================" -ForegroundColor Yellow
  Write-Host ""

  # Resolve input path
  $InputBundle = Resolve-Path $InputBundle

  # Find payload file
  # $payloadFile = Get-ChildItem -Path $tempDir -Include "*.wasm", "*.tgz" -File -Recurse | Select-Object -First 1
  $payloadFile = Get-ChildItem -Path $InputBundle -Include "*.wasm", "*.tgz" -File -Recurse | Select-Object -First 1
  if (-not $payloadFile) {
    throw "No payload file (.wasm or .tgz) found in bundle"
  }
  Write-Host "  Found payload: $($payloadFile.Name)" -ForegroundColor Gray

  # Find metadata file
  # $metadataFile = Get-ChildItem -Path $tempDir -Filter "ck-ext-info.json" -File -Recurse | Select-Object -First 1
  $metadataFile = Get-ChildItem -Path $InputBundle -Filter "ck-ext-info.json" -File -Recurse | Select-Object -First 1

  if (-not $metadataFile) {
    throw "No metadata file (ck-ext-info.json) found in bundle"
  }
  Write-Host "  Found metadata: $($metadataFile.Name)" -ForegroundColor Gray
  Write-Host ""

  # Enhance metadata with signing information
  Write-Host "Enhancing metadata..." -ForegroundColor Cyan

  # Read existing metadata
  $metadataJson = Get-Content $metadataFile.FullName -Raw | ConvertFrom-Json

  # Get certificate subject
  # $publisher = Get-CertificateSubject
  $publisher = "Kvaser AB"

  # Update signing-related fields
  $metadataJson | Add-Member -NotePropertyName "productId" -NotePropertyValue $ProductId -Force
  $metadataJson | Add-Member -NotePropertyName "ean" -NotePropertyValue $Ean -Force
  $metadataJson | Add-Member -NotePropertyName "licenseType" -NotePropertyValue $LicenseType -Force
  $metadataJson | Add-Member -NotePropertyName "publisher" -NotePropertyValue $publisher -Force
  $metadataJson | Add-Member -NotePropertyName "requiresActivation" -NotePropertyValue $RequiresActivation -Force
  $metadataJson | Add-Member -NotePropertyName "signedDate" -NotePropertyValue ([datetime]::UtcNow.ToString("o")) -Force

  if (-not [string]::IsNullOrEmpty($CopyrightNotice)) {
    $metadataJson | Add-Member -NotePropertyName "copyrightNotice" -NotePropertyValue $CopyrightNotice -Force
  }
  elseif ($LicenseType -eq "Commercial") {
    $year = (Get-Date).Year
    $metadataJson | Add-Member -NotePropertyName "copyrightNotice" -NotePropertyValue "© $year Kvaser AB. All rights reserved." -Force
  }

  if (-not [string]::IsNullOrEmpty($Eula)) {
    $metadataJson | Add-Member -NotePropertyName "eula" -NotePropertyValue $Eula -Force
  }

  # Write enhanced metadata back
  [System.IO.File]::WriteAllText($metadataFile.FullName, ($metadataJson | ConvertTo-Json -Depth 10), [System.Text.UTF8Encoding]::new($false))

  Write-Host "  License Type: $LicenseType" -ForegroundColor Gray
  Write-Host "  Publisher: $publisher" -ForegroundColor Gray
  Write-Host "  Requires Activation: $RequiresActivation" -ForegroundColor Gray
  Write-Host ""

  # Combine payload and metadata into a single file for signing
  Write-Host "Combining payload and metadata..." -ForegroundColor Cyan
  # $combinedPath = Join-Path $tempDir "bundle.dat"
  $combinedPath = Join-Path $InputBundle "bundle.dat"


  # Read both files and concatenate (payload first, then metadata - must match C# order)
  $payloadBytes = [System.IO.File]::ReadAllBytes($payloadFile.FullName)
  $metadataBytes = [System.IO.File]::ReadAllBytes($metadataFile.FullName)
  $combinedBytes = $payloadBytes + $metadataBytes
  [System.IO.File]::WriteAllBytes($combinedPath, $combinedBytes)

  Write-Host "  Combined size: $([math]::Round($combinedBytes.Length / 1KB, 2)) KB" -ForegroundColor Gray
  Write-Host ""

  # Sign the combined data
  Sign-FileDetached -FilePath $combinedPath

  # Rename signature to bundle.p7s
  $combinedSig = "$combinedPath.p7"
  #$bundleSig = Join-Path $tempDir "bundle.p7s"
  $bundleSig = Join-Path $InputBundle "bundle.p7s"

  if (Test-Path $combinedSig) {
    Move-Item $combinedSig $bundleSig -Force
    Write-Host "  ? Renamed to bundle.p7s" -ForegroundColor Green
  }
  else {
    throw "Combined signature file was not created"
  }

  # Remove temporary combined data file
  Remove-Item $combinedPath -Force

  Write-Host ""

  # Verify signature file was created
  if (-not (Test-Path $bundleSig)) {
    throw "Bundle signature file was not created: $bundleSig"
  }
}
catch {
  Write-Host ""
  Write-Host "? Error: $($_.Exception.Message)" -ForegroundColor Red
  exit 1
}