[CmdletBinding(DefaultParameterSetName = 'Live')]
param(
  [Parameter(Mandatory, ParameterSetName = 'Fixture')]
  [string]$FixturePath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidatePattern('^[0-9a-f]{40}$')]
  [string]$CandidateSha,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$CandidatePackagePath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$CandidateBinaryPath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$VhdxPath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$ProvenancePath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$StateRoot,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$RuntimeRoot,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$ReceiptPath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, [long]::MaxValue)]
  [long]$StateRequiredFreeBytes,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, [long]::MaxValue)]
  [long]$RuntimeRequiredFreeBytes,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, [long]::MaxValue)]
  [long]$ImageRequiredFreeBytes,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, [long]::MaxValue)]
  [long]$PackageRequiredFreeBytes,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, [long]::MaxValue)]
  [long]$BinaryRequiredFreeBytes,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, [long]::MaxValue)]
  [long]$ProvenanceRequiredFreeBytes,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, [long]::MaxValue)]
  [long]$RollbackMarginBytes,

  [ValidateRange(1, 168)]
  [int]$MaximumReceiptAgeHours = 24
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$contract = 'vmcell.hyperv-r5-preflight.v1'
$fixtureContract = 'vmcell.hyperv-r5-preflight-fixture.v1'
$observationCodes = @(
  'elevation',
  'administrators_membership',
  'os_build_architecture',
  'hyperv_feature',
  'hyperv_module',
  'hyperv_cmdlets',
  'vmms_state',
  'hyperv_read_access',
  'vm_inventory',
  'switch_inventory',
  'virtualization_writers',
  'runner_codex_activity',
  'state_root_storage',
  'runtime_root_storage',
  'image_storage',
  'package_storage',
  'binary_storage',
  'provenance_storage',
  'evidence_output',
  'immutable_vhdx_presence',
  'vhdx_immutability',
  'vhdx_attachment',
  'vhdx_parent',
  'image_provenance',
  'candidate_hash',
  'package_hash',
  'binary_hash',
  'vhdx_hash',
  'admission_receipt',
  'exclusive_window'
)
$frozenCandidate = [ordered]@{
  release_ref = 'release/v0.4.1'
  sha = '0e7fcf37f4310562d318f9d5c709ddf8e8ca1637'
  version = '0.4.1'
  package_name = 'vmcell-v0.4.1-windows-x86_64.zip'
  package_sha256 = '3802a045148849c2dc7a385e2fee43865336dbd3d12ea64347503713230324b7'
  checksum_manifest_sha256 = 'ad0825847013090138ddfd7ab899a13b3d5588e0b60c893760eb2c8f27804a03'
  binary_name = 'vmcell.exe'
  binary_sha256 = '249db6841161d634449142584ad7924b26cbe7b31a41eca9b813dd2eb8acec1b'
  windows_target = 'x86_64-pc-windows-msvc'
  windows_edition = 'Windows Server 2022'
  windows_architecture = 'x86_64'
  secure_boot_template = 'MicrosoftWindows'
}

function Get-Sha256Text {
  param([Parameter(Mandatory)][string]$Text)

  $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
  $hash = [Security.Cryptography.SHA256]::HashData($bytes)
  return [Convert]::ToHexString($hash).ToLowerInvariant()
}

function Get-Sha256File {
  param([Parameter(Mandatory)][string]$Path)

  return (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
}

function Assert-NotReparsePoint {
  param(
    [Parameter(Mandatory)][IO.FileSystemInfo]$Item,
    [Parameter(Mandatory)][string]$Description
  )

  if (($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "$Description or parent must not be a reparse point"
  }
}

function Get-PathItemWithoutFollowingReparse {
  param(
    [Parameter(Mandatory)][string]$FullPath,
    [Parameter(Mandatory)][string]$Description
  )

  $rootPath = [IO.Path]::GetPathRoot($FullPath)
  if ([string]::IsNullOrWhiteSpace($rootPath)) {
    throw "$Description must have a filesystem root"
  }

  $currentItem = Get-Item -LiteralPath $rootPath -Force -ErrorAction Stop
  if (-not $currentItem.PSIsContainer) {
    throw "$Description root must be a directory"
  }
  Assert-NotReparsePoint -Item $currentItem -Description $Description

  $relativePath = $FullPath.Substring($rootPath.Length)
  $separators = [char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
  $segments = $relativePath.Split($separators, [StringSplitOptions]::RemoveEmptyEntries)
  foreach ($segment in $segments) {
    if (-not $currentItem.PSIsContainer) {
      throw "$Description ancestor must be a directory"
    }
    $nextPath = Join-Path -Path $currentItem.FullName -ChildPath $segment
    $currentItem = Get-Item -LiteralPath $nextPath -Force -ErrorAction Stop
    Assert-NotReparsePoint -Item $currentItem -Description $Description
  }
  return $currentItem
}

function Assert-OrdinaryPathAncestry {
  param(
    [Parameter(Mandatory)][IO.FileSystemInfo]$Item,
    [Parameter(Mandatory)][string]$Description
  )

  for ($ancestor = $Item; $null -ne $ancestor) {
    Assert-NotReparsePoint -Item $ancestor -Description $Description
    if ($ancestor -is [IO.FileInfo]) {
      $ancestor = $ancestor.Directory
    } elseif ($ancestor -is [IO.DirectoryInfo]) {
      $ancestor = $ancestor.Parent
    } else {
      throw "$Description has an unsupported filesystem item type"
    }
  }
}

function Get-OrdinaryPathItem {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][bool]$RequireDirectory,
    [Parameter(Mandatory)][string]$Description
  )

  $fullPath = [IO.Path]::GetFullPath($Path)
  $item = Get-PathItemWithoutFollowingReparse -FullPath $fullPath -Description $Description
  if ($RequireDirectory -and -not $item.PSIsContainer) {
    throw "$Description must be a directory"
  }
  if (-not $RequireDirectory -and $item.PSIsContainer) {
    throw "$Description must be a file"
  }
  Assert-OrdinaryPathAncestry -Item $item -Description $Description
  return $item
}

function Get-OrdinaryProvenanceFile {
  param([Parameter(Mandatory)][string]$Path)

  return Get-OrdinaryPathItem -Path $Path -RequireDirectory $false -Description 'provenance path'
}

function Get-SafeProvenanceSnapshot {
  param([Parameter(Mandatory)][string]$Path)

  $item = Get-OrdinaryProvenanceFile -Path $Path
  $fullPath = $item.FullName
  $beforeHash = Get-Sha256File -Path $fullPath
  $bytes = [IO.File]::ReadAllBytes($fullPath)
  $verifiedItem = Get-OrdinaryProvenanceFile -Path $fullPath
  $afterHash = Get-Sha256File -Path $fullPath
  if ($beforeHash -cne $afterHash -or
      $item.Length -ne $verifiedItem.Length -or
      $item.LastWriteTimeUtc -ne $verifiedItem.LastWriteTimeUtc) {
    throw 'provenance evidence changed while it was read'
  }

  try {
    $value = [Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json -ErrorAction Stop
  } catch {
    throw 'provenance evidence is not valid UTF-8 JSON'
  }
  return [pscustomobject]@{ value = $value; sha256 = $afterHash }
}

function Test-Sha256 {
  param([AllowNull()][string]$Value)

  return $null -ne $Value -and $Value -cmatch '^[0-9a-f]{64}$'
}

function Get-ObjectProperty {
  param(
    [AllowNull()][object]$InputObject,
    [Parameter(Mandatory)][string]$Name
  )

  if ($null -eq $InputObject) { return $null }
  $property = $InputObject.PSObject.Properties[$Name]
  if ($null -eq $property) { return $null }
  return $property.Value
}

function Test-OperationalStorageFacts {
  param([Parameter(Mandatory)][object]$Facts)

  $available = Get-ObjectProperty -InputObject $Facts -Name 'evidence_available'
  if ($available -isnot [bool] -or -not [bool]$available) {
    return [pscustomobject]@{ status = 'unavailable'; evidence = @{ complete = $false } }
  }

  $exists = Get-ObjectProperty -InputObject $Facts -Name 'exists'
  $ordinary = Get-ObjectProperty -InputObject $Facts -Name 'ordinary_ancestry'
  $driveType = [string](Get-ObjectProperty -InputObject $Facts -Name 'drive_type')
  $fileSystem = [string](Get-ObjectProperty -InputObject $Facts -Name 'file_system')
  $busType = [string](Get-ObjectProperty -InputObject $Facts -Name 'bus_type')
  $boundary = [string](Get-ObjectProperty -InputObject $Facts -Name 'boundary')
  $freeBytes = Get-ObjectProperty -InputObject $Facts -Name 'free_bytes'
  $requiredBytes = Get-ObjectProperty -InputObject $Facts -Name 'required_bytes'
  $rollbackBytes = Get-ObjectProperty -InputObject $Facts -Name 'rollback_margin_bytes'
  if ($exists -isnot [bool] -or $ordinary -isnot [bool] -or
      $freeBytes -isnot [ValueType] -or $requiredBytes -isnot [ValueType] -or
      $rollbackBytes -isnot [ValueType]) {
    return [pscustomobject]@{ status = 'unavailable'; evidence = @{ complete = $false } }
  }

  try {
    $free = [uint64]$freeBytes
    $required = [uint64]$requiredBytes
    $rollback = [uint64]$rollbackBytes
    if ($required -eq 0 -or $rollback -eq 0 -or $required -gt ([uint64]::MaxValue - $rollback)) {
      throw 'capacity values are invalid'
    }
    $capacity = $free -ge ($required + $rollback)
  } catch {
    return [pscustomobject]@{ status = 'unavailable'; evidence = @{ complete = $false } }
  }

  $admittedBusTypes = @('ATA', 'SATA', 'SAS', 'SCSI', 'RAID', 'NVMe', 'SCM')
  $admittedBacking = $driveType -ceq 'Fixed' -and
    $admittedBusTypes -ccontains $busType -and
    $boundary -ceq 'ordinary-local-disk'
  $suitable = [bool]$exists -and [bool]$ordinary -and $admittedBacking -and
    $fileSystem -ceq 'NTFS' -and $capacity
  return [pscustomobject]@{
    status = if ($suitable) { 'pass' } else { 'fail' }
    evidence = @{
      present = [bool]$exists
      ordinary = [bool]$ordinary
      fixed = $driveType -ceq 'Fixed'
      local_backing = $admittedBacking
      ntfs = $fileSystem -ceq 'NTFS'
      capacity_and_rollback = $capacity
    }
  }
}

function Get-LiveOperationalStorageFacts {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][bool]$RequireDirectory,
    [Parameter(Mandatory)][long]$RequiredFreeBytes,
    [Parameter(Mandatory)][long]$RollbackBytes,
    [Parameter(Mandatory)][string]$Description
  )

  $item = Get-OrdinaryPathItem -Path $Path -RequireDirectory $RequireDirectory -Description $Description
  $volumes = @(Get-Volume -Path $item.FullName -ErrorAction Stop)
  if ($volumes.Count -ne 1) { throw "$Description volume evidence is ambiguous" }
  $partitions = @($volumes[0] | Get-Partition -ErrorAction Stop)
  if ($partitions.Count -ne 1) { throw "$Description partition evidence is ambiguous" }
  $disks = @(Get-Disk -Number $partitions[0].DiskNumber -ErrorAction Stop)
  if ($disks.Count -ne 1) { throw "$Description disk evidence is ambiguous" }
  $disk = $disks[0]
  $busType = [string]$disk.BusType
  $boundary = if ($busType -ceq 'File Backed Virtual') {
    'file-backed-virtual'
  } elseif ([string]$volumes[0].DriveType -cne 'Fixed') {
    'non-fixed'
  } elseif ($busType -in @('USB', 'SD', 'MMC')) {
    'removable'
  } elseif ($busType -in @('Unknown', 'Virtual', 'Spaces')) {
    'ambiguous'
  } else {
    'ordinary-local-disk'
  }
  return [pscustomobject]@{
    evidence_available = $true
    exists = $true
    ordinary_ancestry = $true
    drive_type = [string]$volumes[0].DriveType
    file_system = [string]$volumes[0].FileSystem
    bus_type = $busType
    boundary = $boundary
    free_bytes = [uint64]$volumes[0].SizeRemaining
    required_bytes = [uint64]$RequiredFreeBytes
    rollback_margin_bytes = [uint64]$RollbackBytes
  }
}

function Test-EvidenceOutputFacts {
  param([Parameter(Mandatory)][object]$Facts)

  $available = Get-ObjectProperty -InputObject $Facts -Name 'evidence_available'
  if ($available -isnot [bool] -or -not [bool]$available) {
    return [pscustomobject]@{ status = 'unavailable'; evidence = @{ complete = $false } }
  }
  $parentExists = Get-ObjectProperty -InputObject $Facts -Name 'parent_exists'
  $parentDirectory = Get-ObjectProperty -InputObject $Facts -Name 'parent_is_directory'
  $ordinary = Get-ObjectProperty -InputObject $Facts -Name 'ordinary_ancestry'
  $targetExists = Get-ObjectProperty -InputObject $Facts -Name 'target_exists'
  if ($parentExists -isnot [bool] -or $parentDirectory -isnot [bool] -or
      $ordinary -isnot [bool] -or $targetExists -isnot [bool]) {
    return [pscustomobject]@{ status = 'unavailable'; evidence = @{ complete = $false } }
  }
  $suitable = [bool]$parentExists -and [bool]$parentDirectory -and
    [bool]$ordinary -and -not [bool]$targetExists
  return [pscustomobject]@{
    status = if ($suitable) { 'pass' } else { 'fail' }
    evidence = @{
      parent_exists = [bool]$parentExists
      parent_directory = [bool]$parentDirectory
      ordinary = [bool]$ordinary
      target_absent = -not [bool]$targetExists
      operational_storage_authority = $false
    }
  }
}

function Get-LiveEvidenceOutputFacts {
  param([Parameter(Mandatory)][string]$Path)

  $fullPath = [IO.Path]::GetFullPath($Path)
  $parentPath = [IO.Path]::GetDirectoryName($fullPath)
  if ([string]::IsNullOrWhiteSpace($parentPath) -or
      [string]::IsNullOrWhiteSpace([IO.Path]::GetFileName($fullPath))) {
    throw 'receipt path must name a file beneath an existing parent'
  }
  $parent = Get-OrdinaryPathItem -Path $parentPath -RequireDirectory $true -Description 'receipt parent'
  return [pscustomobject]@{
    evidence_available = $true
    parent_exists = $true
    parent_is_directory = [bool]$parent.PSIsContainer
    ordinary_ancestry = $true
    target_exists = Test-Path -LiteralPath $fullPath
  }
}

function Write-SanitizedReceiptCreateNew {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$Json
  )

  $facts = Get-LiveEvidenceOutputFacts -Path $Path
  $qualification = Test-EvidenceOutputFacts -Facts $facts
  if ($qualification.status -cne 'pass') { throw 'receipt output contract is not satisfied' }
  $fullPath = [IO.Path]::GetFullPath($Path)
  $bytes = [Text.UTF8Encoding]::new($false).GetBytes($Json + [Environment]::NewLine)
  $stream = [IO.File]::Open($fullPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
  try {
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
  } finally {
    $stream.Dispose()
  }
}

function Test-LiveProvenance {
  param([Parameter(Mandatory)][object]$Provenance)

  $candidate = Get-ObjectProperty -InputObject $Provenance -Name 'candidate'
  $package = Get-ObjectProperty -InputObject $Provenance -Name 'package'
  $binary = Get-ObjectProperty -InputObject $Provenance -Name 'candidate_binary'
  $windows = Get-ObjectProperty -InputObject $Provenance -Name 'windows'
  $imageSource = Get-ObjectProperty -InputObject $Provenance -Name 'image_source'
  $vhdx = Get-ObjectProperty -InputObject $Provenance -Name 'vhdx'
  $immutability = Get-ObjectProperty -InputObject $Provenance -Name 'immutability'
  $receipt = Get-ObjectProperty -InputObject $Provenance -Name 'admission_receipt'
  $exclusiveWindow = Get-ObjectProperty -InputObject $Provenance -Name 'exclusive_window'
  $requiredStrings = @(
    [string](Get-ObjectProperty -InputObject $candidate -Name 'release_ref'),
    [string](Get-ObjectProperty -InputObject $candidate -Name 'sha'),
    [string](Get-ObjectProperty -InputObject $candidate -Name 'version'),
    [string](Get-ObjectProperty -InputObject $package -Name 'archive_name'),
    [string](Get-ObjectProperty -InputObject $package -Name 'archive_sha256'),
    [string](Get-ObjectProperty -InputObject $package -Name 'checksum_manifest_sha256'),
    [string](Get-ObjectProperty -InputObject $package -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject $binary -Name 'name'),
    [string](Get-ObjectProperty -InputObject $binary -Name 'version'),
    [string](Get-ObjectProperty -InputObject $binary -Name 'target'),
    [string](Get-ObjectProperty -InputObject $binary -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject $windows -Name 'edition'),
    [string](Get-ObjectProperty -InputObject $windows -Name 'build'),
    [string](Get-ObjectProperty -InputObject $windows -Name 'architecture'),
    [string](Get-ObjectProperty -InputObject $Provenance -Name 'support_status'),
    [string](Get-ObjectProperty -InputObject $imageSource -Name 'kind'),
    [string](Get-ObjectProperty -InputObject $imageSource -Name 'source_sha256'),
    [string](Get-ObjectProperty -InputObject $vhdx -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject $vhdx -Name 'generation'),
    [string](Get-ObjectProperty -InputObject $vhdx -Name 'secure_boot'),
    [string](Get-ObjectProperty -InputObject $vhdx -Name 'secure_boot_template'),
    [string](Get-ObjectProperty -InputObject $vhdx -Name 'virtualization_based_security'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'creation') -Name 'created_at_utc'),
    [string](Get-ObjectProperty -InputObject $receipt -Name 'issued_at_utc'),
    [string](Get-ObjectProperty -InputObject $receipt -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject $exclusiveWindow -Name 'starts_at_utc'),
    [string](Get-ObjectProperty -InputObject $exclusiveWindow -Name 'ends_at_utc'),
    [string](Get-ObjectProperty -InputObject $exclusiveWindow -Name 'evidence_sha256')
  )
  $hashes = @(
    $requiredStrings[4], $requiredStrings[5], $requiredStrings[6], $requiredStrings[10],
    $requiredStrings[16], $requiredStrings[17], $requiredStrings[24], $requiredStrings[27]
  )
  $timestampValid = $true
  foreach ($timestamp in @($requiredStrings[22], $requiredStrings[23], $requiredStrings[25], $requiredStrings[26])) {
    try { [DateTimeOffset]::Parse($timestamp) | Out-Null } catch { $timestampValid = $false }
  }
  $windowOrdered = $false
  try {
    $windowOrdered = [DateTimeOffset]::Parse($requiredStrings[25]) -lt
      [DateTimeOffset]::Parse($requiredStrings[26])
  } catch {}
  return $Provenance.schema_version -eq 1 -and
    $Provenance.contract -ceq 'vmcell.hyperv-r5-image-provenance.v1' -and
    $Provenance.authority -ceq 'none' -and
    $Provenance.acceptance -is [bool] -and -not [bool]$Provenance.acceptance -and
    $Provenance.real_platform_acceptance -ceq 'not_started' -and
    $Provenance.authorizing -is [bool] -and -not [bool]$Provenance.authorizing -and
    $requiredStrings.Count -eq @($requiredStrings | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }).Count -and
    $requiredStrings[0] -ceq $frozenCandidate.release_ref -and
    $requiredStrings[1] -ceq $frozenCandidate.sha -and
    $requiredStrings[2] -ceq $frozenCandidate.version -and
    $requiredStrings[3] -ceq $frozenCandidate.package_name -and
    $requiredStrings[4] -ceq $frozenCandidate.package_sha256 -and
    $requiredStrings[5] -ceq $frozenCandidate.checksum_manifest_sha256 -and
    $requiredStrings[6] -ceq $frozenCandidate.package_sha256 -and
    $requiredStrings[7] -ceq $frozenCandidate.binary_name -and
    $requiredStrings[8] -ceq $frozenCandidate.version -and
    $requiredStrings[9] -ceq $frozenCandidate.windows_target -and
    $requiredStrings[10] -ceq $frozenCandidate.binary_sha256 -and
    $requiredStrings[11] -ceq $frozenCandidate.windows_edition -and
    $requiredStrings[12] -cmatch '^[0-9]{4,10}$' -and
    $requiredStrings[13] -ceq $frozenCandidate.windows_architecture -and
    $requiredStrings[14] -ceq 'untested' -and
    $requiredStrings[18] -ceq '2' -and
    $requiredStrings[19] -ceq 'On' -and
    $requiredStrings[20] -ceq $frozenCandidate.secure_boot_template -and
    @($hashes | Where-Object { -not (Test-Sha256 -Value $_) }).Count -eq 0 -and
    [string](Get-ObjectProperty -InputObject $windows -Name 'architecture') -ceq $frozenCandidate.windows_architecture -and
    [string](Get-ObjectProperty -InputObject $vhdx -Name 'vhd_type') -ceq 'fixed' -and
    $null -eq (Get-ObjectProperty -InputObject $vhdx -Name 'parent_path') -and
    (Get-ObjectProperty -InputObject $vhdx -Name 'attached') -is [bool] -and
    -not [bool](Get-ObjectProperty -InputObject $vhdx -Name 'attached') -and
    (Get-ObjectProperty -InputObject $immutability -Name 'declared') -is [bool] -and
    [bool](Get-ObjectProperty -InputObject $immutability -Name 'declared') -and
    (Test-Sha256 -Value ([string](Get-ObjectProperty -InputObject $immutability -Name 'verification_evidence_sha256'))) -and
    [string](Get-ObjectProperty -InputObject $receipt -Name 'receipt_id') -cmatch '^[A-Za-z0-9._-]{12,128}$' -and
    (Get-ObjectProperty -InputObject $exclusiveWindow -Name 'eligible') -is [bool] -and
    $timestampValid -and $windowOrdered
}

function New-Observation {
  param(
    [Parameter(Mandatory)][string]$Code,
    [Parameter(Mandatory)][ValidateSet('pass', 'fail', 'unavailable')][string]$Status,
    [Parameter(Mandatory)][string]$EvidenceSha256
  )

  if ($observationCodes -cnotcontains $Code) {
    throw "preflight.internal_invalid_observation: $Code"
  }
  if (-not (Test-Sha256 -Value $EvidenceSha256)) {
    throw "preflight.internal_invalid_evidence_digest: $Code"
  }
  return [ordered]@{
    code = $Code
    status = $Status
    evidence_sha256 = $EvidenceSha256
  }
}

function Get-ObservationDigest {
  param([Parameter(Mandatory)][object[]]$Observations)

  $digestInput = @($Observations | ForEach-Object {
    "$($_.code)|$($_.status)|$($_.evidence_sha256)"
  }) -join "`n"
  return Get-Sha256Text -Text $digestInput
}

function New-InvalidFixtureResult {
  param([Parameter(Mandatory)][string]$FailureCode)

  return [ordered]@{
    schema_version = 1
    contract = $contract
    evidence_source = 'fixture'
    authority = 'none'
    acceptance = $false
    real_platform_acceptance = 'not_started'
    disposition = 'BLOCKED'
    observations = @()
    blockers = @($FailureCode)
    owner_actions = @('replace_invalid_fixture_with_a_complete_sanitized_fixture')
    evidence_digests = [ordered]@{
      fixture_sha256 = 'unavailable'
      observations_sha256 = 'unavailable'
    }
    mutation_flags = [ordered]@{
      host_observation = $false
      hyperv_mutation = $false
      service_mutation = $false
      vm_mutation = $false
      disk_mutation = $false
      image_mutation = $false
      network_mutation = $false
      runner_mutation = $false
      registry_mutation = $false
      github_observation = $false
      receipt_write = $false
    }
  }
}

function New-PreflightResult {
  param(
    [Parameter(Mandatory)][ValidateSet('fixture', 'live-read-only')][string]$EvidenceSource,
    [Parameter(Mandatory)][object[]]$Observations,
    [Parameter(Mandatory)][string]$EvidenceSourceDigest,
    [Parameter(Mandatory)][bool]$HostObservation,
    [bool]$ReceiptWrite = $false
  )

  $blockers = [System.Collections.Generic.List[string]]::new()
  $actions = [System.Collections.Generic.List[string]]::new()
  foreach ($observation in $Observations) {
    if ($observation.status -eq 'unavailable') {
      $blockers.Add("evidence_gap.$($observation.code)")
      $actions.Add("obtain_read_only_evidence.$($observation.code)")
    } elseif ($observation.status -eq 'fail') {
      $blockers.Add("precondition_failed.$($observation.code)")
      $actions.Add("resolve_precondition_outside_this_tool.$($observation.code)")
    }
  }
  $orderedObservations = @($observationCodes | ForEach-Object {
    $code = $_
    @($Observations | Where-Object { $_.code -ceq $code })[0]
  })
  $observationDigest = Get-ObservationDigest -Observations $orderedObservations
  $eligible = $blockers.Count -eq 0
  if ($eligible) {
    $actions.Add('obtain_separate_authorization_before_any_real_platform_action')
  }
  return [ordered]@{
    schema_version = 1
    contract = $contract
    evidence_source = $EvidenceSource
    authority = 'none'
    acceptance = $false
    real_platform_acceptance = 'not_started'
    disposition = if ($eligible) { 'PREFLIGHT_ELIGIBLE' } else { 'BLOCKED' }
    observations = $orderedObservations
    blockers = @($blockers | Sort-Object -Unique)
    owner_actions = @($actions | Sort-Object -Unique)
    evidence_digests = [ordered]@{
      source_sha256 = $EvidenceSourceDigest
      observations_sha256 = $observationDigest
    }
    mutation_flags = [ordered]@{
      host_observation = $HostObservation
      hyperv_mutation = $false
      service_mutation = $false
      vm_mutation = $false
      disk_mutation = $false
      image_mutation = $false
      network_mutation = $false
      runner_mutation = $false
      registry_mutation = $false
      github_observation = $false
      receipt_write = $ReceiptWrite
    }
  }
}

function Convert-FixtureToObservations {
  param([Parameter(Mandatory)][string]$Path)

  try {
    $bytes = [IO.File]::ReadAllBytes([IO.Path]::GetFullPath($Path))
    $text = [Text.Encoding]::UTF8.GetString($bytes)
    $fixture = $text | ConvertFrom-Json -ErrorAction Stop
  } catch {
    return $null
  }
  if ($fixture.schema_version -ne 1 -or $fixture.contract -cne $fixtureContract) {
    return $null
  }
  $fixtureId = Get-ObjectProperty -InputObject $fixture -Name 'fixture_id'
  if ([string]$fixtureId -cnotmatch '^[a-z0-9][a-z0-9-]{2,63}$') {
    return $null
  }
  $rows = @($fixture.observations)
  if ($rows.Count -ne $observationCodes.Count) {
    return $null
  }
  $observations = [System.Collections.Generic.List[object]]::new()
  $seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
  foreach ($row in $rows) {
    $code = [string](Get-ObjectProperty -InputObject $row -Name 'code')
    $status = [string](Get-ObjectProperty -InputObject $row -Name 'status')
    $digest = [string](Get-ObjectProperty -InputObject $row -Name 'evidence_sha256')
    if ($observationCodes -cnotcontains $code -or
        $status -cnotin @('pass', 'fail', 'unavailable') -or
        -not (Test-Sha256 -Value $digest) -or
        -not $seen.Add($code)) {
      return $null
    }
    $observations.Add((New-Observation -Code $code -Status $status -EvidenceSha256 $digest))
  }
  if ($seen.Count -ne $observationCodes.Count) {
    return $null
  }
  return [pscustomobject]@{
    observations = @($observations)
    digest = Get-Sha256Text -Text ([Convert]::ToBase64String($bytes))
  }
}

function Invoke-LiveObservation {
  param(
    [Parameter(Mandatory)][string]$Code,
    [AllowNull()][object]$ProvenanceSnapshot
  )

  try {
    $value = switch ($Code) {
      'elevation' {
        $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
        $admin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        [pscustomobject]@{ status = if ($admin) { 'pass' } else { 'fail' }; evidence = @{ elevated = $admin } }
        break
      }
      'administrators_membership' {
        $currentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        $members = @(Get-LocalGroupMember -SID 'S-1-5-32-544' -ErrorAction Stop)
        $member = @($members | Where-Object { $_.SID.Value -ceq $currentSid }).Count -eq 1
        [pscustomobject]@{ status = if ($member) { 'pass' } else { 'fail' }; evidence = @{ member = $member } }
        break
      }
      'os_build_architecture' {
        $os = Get-CimInstance -ClassName Win32_OperatingSystem -ErrorAction Stop
        $x64 = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ceq 'X64'
        $valid = $x64 -and [string]$os.Caption -match 'Windows' -and [string]$os.BuildNumber -match '^\d+$'
        [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ x64 = $x64; build = [string]$os.BuildNumber } }
        break
      }
      'hyperv_feature' {
        $feature = Get-WindowsOptionalFeature -Online -FeatureName 'Microsoft-Hyper-V-All' -ErrorAction Stop
        $enabled = [string]$feature.State -ceq 'Enabled'
        [pscustomobject]@{ status = if ($enabled) { 'pass' } else { 'fail' }; evidence = @{ enabled = $enabled } }
        break
      }
      'hyperv_module' {
        $module = @(Get-Module -ListAvailable -Name Hyper-V -ErrorAction Stop)
        [pscustomobject]@{ status = if ($module.Count -gt 0) { 'pass' } else { 'fail' }; evidence = @{ count = $module.Count } }
        break
      }
      'hyperv_cmdlets' {
        $names = @('Get-VM', 'Get-VHD', 'Get-VMSwitch')
        $resolved = @($names | ForEach-Object { Get-Command -Name $_ -ErrorAction Stop })
        [pscustomobject]@{ status = if ($resolved.Count -eq $names.Count) { 'pass' } else { 'fail' }; evidence = @{ count = $resolved.Count } }
        break
      }
      'vmms_state' {
        $vmms = Get-Service -Name vmms -ErrorAction Stop
        $running = [string]$vmms.Status -ceq 'Running'
        [pscustomobject]@{ status = if ($running) { 'pass' } else { 'fail' }; evidence = @{ running = $running } }
        break
      }
      'hyperv_read_access' {
        $vms = @(Get-VM -ErrorAction Stop)
        [pscustomobject]@{ status = 'pass'; evidence = @{ count = $vms.Count } }
        break
      }
      'vm_inventory' {
        $vms = @(Get-VM -ErrorAction Stop)
        [pscustomobject]@{ status = if ($vms.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $vms.Count } }
        break
      }
      'switch_inventory' {
        $switches = @(Get-VMSwitch -ErrorAction Stop)
        [pscustomobject]@{ status = if ($switches.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $switches.Count } }
        break
      }
      'virtualization_writers' {
        $names = @('vmwp', 'vmcompute', 'vmmem', 'qemu-system-x86_64', 'VirtualBoxVM', 'vmware-vmx')
        $processes = @(Get-Process -ErrorAction Stop | Where-Object { $names -ccontains $_.ProcessName })
        [pscustomobject]@{ status = if ($processes.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $processes.Count } }
        break
      }
      'runner_codex_activity' {
        $names = @('Runner.Listener', 'Runner.Worker', 'codex')
        $processes = @(Get-Process -ErrorAction Stop | Where-Object { $names -ccontains $_.ProcessName })
        [pscustomobject]@{ status = if ($processes.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $processes.Count } }
        break
      }
      'state_root_storage' {
        $facts = Get-LiveOperationalStorageFacts -Path $StateRoot -RequireDirectory $true -RequiredFreeBytes $StateRequiredFreeBytes -RollbackBytes $RollbackMarginBytes -Description 'state root'
        Test-OperationalStorageFacts -Facts $facts
        break
      }
      'runtime_root_storage' {
        $facts = Get-LiveOperationalStorageFacts -Path $RuntimeRoot -RequireDirectory $true -RequiredFreeBytes $RuntimeRequiredFreeBytes -RollbackBytes $RollbackMarginBytes -Description 'runtime root'
        Test-OperationalStorageFacts -Facts $facts
        break
      }
      'image_storage' {
        $facts = Get-LiveOperationalStorageFacts -Path $VhdxPath -RequireDirectory $false -RequiredFreeBytes $ImageRequiredFreeBytes -RollbackBytes $RollbackMarginBytes -Description 'image path'
        Test-OperationalStorageFacts -Facts $facts
        break
      }
      'package_storage' {
        $facts = Get-LiveOperationalStorageFacts -Path $CandidatePackagePath -RequireDirectory $false -RequiredFreeBytes $PackageRequiredFreeBytes -RollbackBytes $RollbackMarginBytes -Description 'candidate package path'
        Test-OperationalStorageFacts -Facts $facts
        break
      }
      'binary_storage' {
        $facts = Get-LiveOperationalStorageFacts -Path $CandidateBinaryPath -RequireDirectory $false -RequiredFreeBytes $BinaryRequiredFreeBytes -RollbackBytes $RollbackMarginBytes -Description 'candidate binary path'
        Test-OperationalStorageFacts -Facts $facts
        break
      }
      'provenance_storage' {
        $facts = Get-LiveOperationalStorageFacts -Path $ProvenancePath -RequireDirectory $false -RequiredFreeBytes $ProvenanceRequiredFreeBytes -RollbackBytes $RollbackMarginBytes -Description 'provenance path'
        Test-OperationalStorageFacts -Facts $facts
        break
      }
      'evidence_output' {
        Test-EvidenceOutputFacts -Facts (Get-LiveEvidenceOutputFacts -Path $ReceiptPath)
        break
      }
      'immutable_vhdx_presence' {
        $vhdItem = Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path'
        $exists = -not $vhdItem.PSIsContainer -and $vhdItem.Extension -ieq '.vhdx'
        [pscustomobject]@{ status = if ($exists) { 'pass' } else { 'fail' }; evidence = @{ present = $exists } }
        break
      }
      'vhdx_immutability' {
        $vhdItem = Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path'
        $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
        $readOnly = ($vhdItem.Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0
        $fixed = [string]$vhd.VhdType -ceq 'Fixed'
        [pscustomobject]@{ status = if ($readOnly -and $fixed) { 'pass' } else { 'fail' }; evidence = @{ readonly = $readOnly; fixed = $fixed } }
        break
      }
      'vhdx_attachment' {
        Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path' | Out-Null
        $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
        $detached = -not [bool]$vhd.Attached
        [pscustomobject]@{ status = if ($detached) { 'pass' } else { 'fail' }; evidence = @{ detached = $detached } }
        break
      }
      'vhdx_parent' {
        Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path' | Out-Null
        $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
        $parentless = [string]::IsNullOrWhiteSpace([string]$vhd.ParentPath)
        [pscustomobject]@{ status = if ($parentless) { 'pass' } else { 'fail' }; evidence = @{ parentless = $parentless } }
        break
      }
      'image_provenance' {
        if ($null -eq $ProvenanceSnapshot) { throw 'provenance snapshot unavailable' }
        $valid = Test-LiveProvenance -Provenance $ProvenanceSnapshot.value
        [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ valid = $valid } }
        break
      }
      'candidate_hash' {
        if ($null -eq $ProvenanceSnapshot) { throw 'provenance snapshot unavailable' }
        $matches = $CandidateSha -ceq $frozenCandidate.sha -and [string]$ProvenanceSnapshot.value.candidate.sha -ceq $frozenCandidate.sha
        [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
        break
      }
      'package_hash' {
        if ($null -eq $ProvenanceSnapshot) { throw 'provenance snapshot unavailable' }
        $package = Get-OrdinaryPathItem -Path $CandidatePackagePath -RequireDirectory $false -Description 'candidate package path'
        $packageHash = Get-Sha256File -Path $package.FullName
        $matches = $packageHash -ceq $frozenCandidate.package_sha256 -and [string]$ProvenanceSnapshot.value.package.sha256 -ceq $frozenCandidate.package_sha256
        [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
        break
      }
      'binary_hash' {
        if ($null -eq $ProvenanceSnapshot) { throw 'provenance snapshot unavailable' }
        $binary = Get-OrdinaryPathItem -Path $CandidateBinaryPath -RequireDirectory $false -Description 'candidate binary path'
        $binaryHash = Get-Sha256File -Path $binary.FullName
        $matches = $binaryHash -ceq $frozenCandidate.binary_sha256 -and [string]$ProvenanceSnapshot.value.candidate_binary.sha256 -ceq $frozenCandidate.binary_sha256
        [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
        break
      }
      'vhdx_hash' {
        if ($null -eq $ProvenanceSnapshot) { throw 'provenance snapshot unavailable' }
        $vhdxHash = Get-Sha256File -Path $VhdxPath
        $matches = [string]$ProvenanceSnapshot.value.vhdx.sha256 -ceq $vhdxHash
        [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
        break
      }
      'admission_receipt' {
        if ($null -eq $ProvenanceSnapshot) { throw 'provenance snapshot unavailable' }
        $issued = [DateTimeOffset]::Parse([string]$ProvenanceSnapshot.value.admission_receipt.issued_at_utc)
        $fresh = ([DateTimeOffset]::UtcNow - $issued).TotalHours -ge 0 -and ([DateTimeOffset]::UtcNow - $issued).TotalHours -le $MaximumReceiptAgeHours
        [pscustomobject]@{ status = if ($fresh) { 'pass' } else { 'fail' }; evidence = @{ fresh = $fresh } }
        break
      }
      'exclusive_window' {
        if ($null -eq $ProvenanceSnapshot) { throw 'provenance snapshot unavailable' }
        $eligible = $ProvenanceSnapshot.value.exclusive_window.eligible -is [bool] -and [bool]$ProvenanceSnapshot.value.exclusive_window.eligible -and [DateTimeOffset]::Parse([string]$ProvenanceSnapshot.value.exclusive_window.ends_at_utc) -gt [DateTimeOffset]::UtcNow
        [pscustomobject]@{ status = if ($eligible) { 'pass' } else { 'fail' }; evidence = @{ eligible = $eligible } }
        break
      }
      default { throw "unknown observation code: $Code" }
    }
    $status = [string](Get-ObjectProperty -InputObject $value -Name 'status')
    if ($status -cnotin @('pass', 'fail', 'unavailable')) {
      throw 'probe did not return a supported status'
    }
    $evidence = Get-ObjectProperty -InputObject $value -Name 'evidence'
    $evidenceJson = $evidence | ConvertTo-Json -Compress -Depth 8
    return New-Observation -Code $Code -Status $status `
      -EvidenceSha256 (Get-Sha256Text -Text $evidenceJson)
  } catch {
    return New-Observation -Code $Code -Status 'unavailable' `
      -EvidenceSha256 (Get-Sha256Text -Text "$Code|$($_.Exception.GetType().Name)")
  }
}

function Get-LiveObservations {
  $provenanceSnapshot = $null
  try {
    $provenanceSnapshot = Get-SafeProvenanceSnapshot -Path $ProvenancePath
  } catch {}

  return @($observationCodes | ForEach-Object {
    Invoke-LiveObservation -Code $_ -ProvenanceSnapshot $provenanceSnapshot
  })
}

if ($PSCmdlet.ParameterSetName -eq 'Fixture') {
  $fixture = Convert-FixtureToObservations -Path $FixturePath
  if ($null -eq $fixture) {
    New-InvalidFixtureResult -FailureCode 'fixture.schema_invalid' | ConvertTo-Json -Compress -Depth 12
  } else {
    New-PreflightResult -EvidenceSource 'fixture' -Observations $fixture.observations `
      -EvidenceSourceDigest $fixture.digest -HostObservation $false |
      ConvertTo-Json -Compress -Depth 12
  }
} else {
  $liveObservations = Get-LiveObservations
  $receiptWrite = @($liveObservations | Where-Object { $_.code -ceq 'evidence_output' })[0].status -ceq 'pass'
  $liveResult = New-PreflightResult -EvidenceSource 'live-read-only' -Observations $liveObservations `
    -EvidenceSourceDigest (Get-ObservationDigest -Observations $liveObservations) `
    -HostObservation $true -ReceiptWrite $receiptWrite
  $liveJson = $liveResult | ConvertTo-Json -Compress -Depth 12
  if ($receiptWrite) {
    Write-SanitizedReceiptCreateNew -Path $ReceiptPath -Json $liveJson
  }
  $liveJson
}
