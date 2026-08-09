[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$BinaryPath,

  [Parameter(Mandatory = $true)]
  [string]$OutputDirectory,

  [Parameter(Mandatory = $true)]
  [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')]
  [string]$Version,

  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[0-9a-fA-F]{40}$')]
  [string]$SourceCommit,

  [Parameter(Mandatory = $true)]
  [ValidateRange(315532800, 4354819199)]
  [long]$SourceDateEpoch,

  [ValidatePattern('^[A-Za-z0-9_.-]+$')]
  [string]$TargetTriple = 'x86_64-pc-windows-msvc'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$binary = Get-Item -LiteralPath $BinaryPath -Force
if (-not $binary.PSIsContainer -and
    -not ($binary.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
  $binaryPathFull = $binary.FullName
} else {
  throw 'BinaryPath must identify one ordinary non-reparse file'
}
if ($binary.Length -le 0 -or $binary.Length -gt 268435456) {
  throw 'BinaryPath must contain one non-empty binary no larger than 256 MiB'
}

$binaryVersion = (& $binaryPathFull --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $binaryVersion -ne "vmcell $Version") {
  throw "binary version mismatch: expected vmcell $Version"
}

$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
[IO.Directory]::CreateDirectory($outputPath) | Out-Null

$layoutRoot = "vmcell-v$Version-windows-x86_64"
$archiveName = "$layoutRoot.zip"
$archivePath = Join-Path $outputPath $archiveName
$temporaryArchive = Join-Path $outputPath ".$archiveName.tmp-$PID"
$checksumPath = Join-Path $outputPath 'SHA256SUMS.txt'
$installPath = Join-Path $repositoryRoot 'packaging\windows\INSTALL.txt'
$licensePath = Join-Path $repositoryRoot 'LICENSE'
$noticePath = Join-Path $repositoryRoot 'NOTICE'

foreach ($inputPath in @($installPath, $licensePath, $noticePath)) {
  $input = Get-Item -LiteralPath $inputPath -Force
  if ($input.PSIsContainer -or
      ($input.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw "package input must be an ordinary non-reparse file: $inputPath"
  }
}

$binarySha256 = (Get-FileHash -LiteralPath $binaryPathFull -Algorithm SHA256).Hash.ToLowerInvariant()
$rustcVersion = (& rustc --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($rustcVersion)) {
  throw 'rustc version provenance was unavailable'
}
$cargoVersion = (& cargo --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($cargoVersion)) {
  throw 'cargo version provenance was unavailable'
}
$entryNames = @(
  "$layoutRoot/vmcell.exe",
  "$layoutRoot/LICENSE.txt",
  "$layoutRoot/NOTICE.txt",
  "$layoutRoot/INSTALL.txt",
  "$layoutRoot/BUILD-PROVENANCE.json"
)
$provenance = [ordered]@{
  schema_version = 1
  package = 'vmcell'
  version = $Version
  target = $TargetTriple
  source_commit = $SourceCommit.ToLowerInvariant()
  source_date_epoch = $SourceDateEpoch
  build_profile = 'release'
  rustc_version = $rustcVersion
  cargo_version = $cargoVersion
  binary_sha256 = $binarySha256
  archive_layout = $entryNames
}
$provenanceJson = (($provenance | ConvertTo-Json -Depth 4) -replace "`r`n", "`n") + "`n"
$utf8 = [Text.UTF8Encoding]::new($false)
$entryTimestamp = [DateTimeOffset]::FromUnixTimeSeconds($SourceDateEpoch)

Add-Type -AssemblyName System.IO.Compression

try {
  $stream = [IO.File]::Open(
    $temporaryArchive,
    [IO.FileMode]::Create,
    [IO.FileAccess]::ReadWrite,
    [IO.FileShare]::None
  )
  try {
    $archive = [IO.Compression.ZipArchive]::new(
      $stream,
      [IO.Compression.ZipArchiveMode]::Create,
      $true
    )
    try {
      $inputs = [ordered]@{
        $entryNames[0] = [IO.File]::ReadAllBytes($binaryPathFull)
        $entryNames[1] = [IO.File]::ReadAllBytes($licensePath)
        $entryNames[2] = [IO.File]::ReadAllBytes($noticePath)
        $entryNames[3] = [IO.File]::ReadAllBytes($installPath)
        $entryNames[4] = $utf8.GetBytes($provenanceJson)
      }
      foreach ($entryName in $entryNames) {
        $entry = $archive.CreateEntry(
          $entryName,
          [IO.Compression.CompressionLevel]::Optimal
        )
        $entry.LastWriteTime = $entryTimestamp
        $entry.ExternalAttributes = 0
        $entryStream = $entry.Open()
        try {
          $bytes = $inputs[$entryName]
          $entryStream.Write($bytes, 0, $bytes.Length)
        } finally {
          $entryStream.Dispose()
        }
      }
    } finally {
      $archive.Dispose()
    }
  } finally {
    $stream.Dispose()
  }

  Move-Item -LiteralPath $temporaryArchive -Destination $archivePath -Force
} finally {
  if (Test-Path -LiteralPath $temporaryArchive) {
    Remove-Item -LiteralPath $temporaryArchive -Force
  }
}

$archiveSha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText(
  $checksumPath,
  "$archiveSha256  $archiveName`n",
  $utf8
)

[PSCustomObject]@{
  archive_path = $archivePath
  archive_sha256 = $archiveSha256
  checksum_path = $checksumPath
  binary_sha256 = $binarySha256
  source_commit = $SourceCommit.ToLowerInvariant()
  source_date_epoch = $SourceDateEpoch
}
