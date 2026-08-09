[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$BinaryPath,

  [Parameter(Mandatory = $true)]
  [string]$Version
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$testRoot = Join-Path (
  [IO.Path]::GetTempPath()
) "vmcell-package-test-$PID-$([Guid]::NewGuid().ToString('N'))"
$first = Join-Path $testRoot 'first'
$second = Join-Path $testRoot 'second'
$sourceCommit = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
$sourceDateEpoch = 1704067200
$archiveName = "vmcell-v$Version-windows-x86_64.zip"
$expectedEntries = @(
  "vmcell-v$Version-windows-x86_64/vmcell.exe",
  "vmcell-v$Version-windows-x86_64/LICENSE.txt",
  "vmcell-v$Version-windows-x86_64/NOTICE.txt",
  "vmcell-v$Version-windows-x86_64/INSTALL.txt",
  "vmcell-v$Version-windows-x86_64/BUILD-PROVENANCE.json"
)

try {
  $packageScript = Join-Path $PSScriptRoot 'package-windows.ps1'
  & $packageScript `
    -BinaryPath $BinaryPath `
    -OutputDirectory $first `
    -Version $Version `
    -SourceCommit $sourceCommit `
    -SourceDateEpoch $sourceDateEpoch |
    Out-Null
  & $packageScript `
    -BinaryPath $BinaryPath `
    -OutputDirectory $second `
    -Version $Version `
    -SourceCommit $sourceCommit `
    -SourceDateEpoch $sourceDateEpoch |
    Out-Null

  $mismatchOutput = Join-Path $testRoot 'version-mismatch'
  $versionRejected = $false
  try {
    & $packageScript `
      -BinaryPath $BinaryPath `
      -OutputDirectory $mismatchOutput `
      -Version '999.999.999' `
      -SourceCommit $sourceCommit `
      -SourceDateEpoch $sourceDateEpoch |
      Out-Null
  } catch {
    $versionRejected = $true
  }
  if (-not $versionRejected -or (Test-Path -LiteralPath $mismatchOutput)) {
    throw 'package creation did not fail closed before a version mismatch write'
  }

  $firstArchive = Join-Path $first $archiveName
  $secondArchive = Join-Path $second $archiveName
  $firstHash = (Get-FileHash -LiteralPath $firstArchive -Algorithm SHA256).Hash
  $secondHash = (Get-FileHash -LiteralPath $secondArchive -Algorithm SHA256).Hash
  if ($firstHash -ne $secondHash) {
    throw 'repeated package builds were not byte-identical'
  }
  $firstChecksums = [IO.File]::ReadAllBytes((Join-Path $first 'SHA256SUMS.txt'))
  $secondChecksums = [IO.File]::ReadAllBytes((Join-Path $second 'SHA256SUMS.txt'))
  if (-not [Linq.Enumerable]::SequenceEqual($firstChecksums, $secondChecksums)) {
    throw 'repeated checksum manifests were not byte-identical'
  }
  $checksumText = [Text.Encoding]::UTF8.GetString($firstChecksums).Trim()
  $checksumParts = $checksumText -split '  ', 2
  if ($checksumParts.Count -ne 2 -or
      $checksumParts[0] -ne $firstHash.ToLowerInvariant() -or
      $checksumParts[1] -ne $archiveName) {
    throw 'checksum manifest did not bind the exact portable archive'
  }

  Add-Type -AssemblyName System.IO.Compression
  $archive = [IO.Compression.ZipFile]::OpenRead($firstArchive)
  try {
    $actualEntries = @($archive.Entries | ForEach-Object { $_.FullName })
    if (($actualEntries -join "`n") -ne ($expectedEntries -join "`n")) {
      throw 'portable archive layout changed unexpectedly'
    }
    $expectedTimestamp = [DateTimeOffset]::FromUnixTimeSeconds(
      $sourceDateEpoch
    ).UtcDateTime
    foreach ($entry in $archive.Entries) {
      if ($entry.LastWriteTime.DateTime -ne $expectedTimestamp) {
        throw "archive timestamp was not normalized: $($entry.FullName)"
      }
    }
    $provenanceEntry = $archive.GetEntry($expectedEntries[4])
    $reader = [IO.StreamReader]::new($provenanceEntry.Open(), [Text.Encoding]::UTF8)
    try {
      $provenance = $reader.ReadToEnd() | ConvertFrom-Json
    } finally {
      $reader.Dispose()
    }
    if ($provenance.schema_version -ne 1 -or
        $provenance.version -ne $Version -or
        $provenance.source_commit -ne $sourceCommit -or
        $provenance.source_date_epoch -ne $sourceDateEpoch -or
        $provenance.build_profile -ne 'release' -or
        -not $provenance.rustc_version.StartsWith('rustc ') -or
        -not $provenance.cargo_version.StartsWith('cargo ')) {
      throw 'package provenance did not preserve its exact build identity'
    }
    $expectedBinaryHash = (Get-FileHash -LiteralPath $BinaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($provenance.binary_sha256 -ne $expectedBinaryHash) {
      throw 'package provenance binary hash mismatch'
    }
    $installEntry = $archive.GetEntry($expectedEntries[3])
    $reader = [IO.StreamReader]::new($installEntry.Open(), [Text.Encoding]::UTF8)
    try {
      $installText = $reader.ReadToEnd()
    } finally {
      $reader.Dispose()
    }
    foreach ($required in @('Install', 'vmcell.exe doctor', 'Remove')) {
      if (-not $installText.Contains($required, [StringComparison]::Ordinal)) {
        throw "portable package instructions omitted: $required"
      }
    }
  } finally {
    $archive.Dispose()
  }
} finally {
  if (Test-Path -LiteralPath $testRoot) {
    Remove-Item -LiteralPath $testRoot -Recurse -Force
  }
}

Write-Host 'Windows portable package determinism and layout passed'
