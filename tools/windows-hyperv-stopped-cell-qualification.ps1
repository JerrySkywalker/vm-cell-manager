[CmdletBinding(DefaultParameterSetName = 'Live')]
param(
  [Parameter(Mandatory, ParameterSetName = 'Fixture')]
  [string]$FixturePath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [Guid]$ExpectedVmId,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]{0,79}$')]
  [string]$ExpectedVmName,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$ExpectedDiskPath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$ExpectedDiskParentPath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(1, 64)]
  [int]$ExpectedProcessorCount,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidateRange(536870912, [long]::MaxValue)]
  [long]$ExpectedMemoryStartupBytes,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [ValidatePattern('^[A-Za-z0-9._-]{1,128}$')]
  [string]$ExpectedSecureBootTemplate,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$ReceiptPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$contract = 'vmcell.hyperv-stopped-cell-qualification.v1'
$fixtureContract = 'vmcell.hyperv-stopped-cell-qualification-fixture.v1'
$observationCodes = @(
  'vm_identity',
  'stopped_state',
  'generation_2',
  'secure_boot',
  'disk_layout',
  'resources',
  'network_adapters',
  'evidence_output'
)

function Get-Sha256Text {
  param([Parameter(Mandatory)][string]$Text)
  $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
  return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}

function Get-ObjectProperty {
  param([Parameter(Mandatory)][object]$InputObject, [Parameter(Mandatory)][string]$Name)
  $property = $InputObject.PSObject.Properties[$Name]
  if ($null -eq $property) { return $null }
  return $property.Value
}

function Test-Sha256 {
  param([AllowNull()][string]$Value)
  return $null -ne $Value -and $Value -cmatch '^[0-9a-f]{64}$'
}

function Assert-NotReparsePoint {
  param([Parameter(Mandatory)][IO.FileSystemInfo]$Item, [Parameter(Mandatory)][string]$Description)
  if (($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "$Description or parent must not be a reparse point"
  }
}

function Get-OrdinaryPathItem {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][bool]$RequireDirectory,
    [Parameter(Mandatory)][string]$Description
  )
  $fullPath = [IO.Path]::GetFullPath($Path)
  $rootPath = [IO.Path]::GetPathRoot($fullPath)
  if ([string]::IsNullOrWhiteSpace($rootPath)) { throw "$Description must have a filesystem root" }
  $current = Get-Item -LiteralPath $rootPath -Force -ErrorAction Stop
  Assert-NotReparsePoint -Item $current -Description $Description
  $separators = [char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
  foreach ($segment in $fullPath.Substring($rootPath.Length).Split($separators, [StringSplitOptions]::RemoveEmptyEntries)) {
    if (-not $current.PSIsContainer) { throw "$Description ancestor must be a directory" }
    $current = Get-Item -LiteralPath (Join-Path $current.FullName $segment) -Force -ErrorAction Stop
    Assert-NotReparsePoint -Item $current -Description $Description
  }
  for ($ancestor = $current; $null -ne $ancestor) {
    Assert-NotReparsePoint -Item $ancestor -Description $Description
    if ($ancestor -is [IO.FileInfo]) { $ancestor = $ancestor.Directory }
    elseif ($ancestor -is [IO.DirectoryInfo]) { $ancestor = $ancestor.Parent }
    else { throw "$Description has an unsupported filesystem item type" }
  }
  if ($RequireDirectory -ne [bool]$current.PSIsContainer) { throw "$Description item type is incorrect" }
  return $current
}

function Get-EvidenceOutputFacts {
  param([Parameter(Mandatory)][string]$Path)
  $fullPath = [IO.Path]::GetFullPath($Path)
  $parentPath = [IO.Path]::GetDirectoryName($fullPath)
  if ([string]::IsNullOrWhiteSpace($parentPath) -or [string]::IsNullOrWhiteSpace([IO.Path]::GetFileName($fullPath))) {
    throw 'receipt path must name a file beneath an existing parent'
  }
  $parent = Get-OrdinaryPathItem -Path $parentPath -RequireDirectory $true -Description 'receipt parent'
  return [pscustomobject]@{
    status = if ($parent.PSIsContainer -and -not (Test-Path -LiteralPath $fullPath)) { 'pass' } else { 'fail' }
    evidence = @{ parent_ordinary = $true; target_absent = -not (Test-Path -LiteralPath $fullPath); operational_storage_authority = $false }
  }
}

function Write-SanitizedReceiptCreateNew {
  param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Json)
  if ((Get-EvidenceOutputFacts -Path $Path).status -cne 'pass') { throw 'receipt output contract is not satisfied' }
  $bytes = [Text.UTF8Encoding]::new($false).GetBytes($Json + [Environment]::NewLine)
  $stream = [IO.File]::Open([IO.Path]::GetFullPath($Path), [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
  try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}

function New-Observation {
  param(
    [Parameter(Mandatory)][string]$Code,
    [Parameter(Mandatory)][ValidateSet('pass', 'fail', 'unavailable')][string]$Status,
    [Parameter(Mandatory)][string]$EvidenceSha256
  )
  if ($observationCodes -cnotcontains $Code -or -not (Test-Sha256 -Value $EvidenceSha256)) {
    throw 'stopped-cell qualification produced invalid sanitized evidence'
  }
  return [ordered]@{ code = $Code; status = $Status; evidence_sha256 = $EvidenceSha256 }
}

function Get-ObservationDigest {
  param([Parameter(Mandatory)][object[]]$Observations)
  return Get-Sha256Text -Text (@($Observations | ForEach-Object { "$($_.code)|$($_.status)|$($_.evidence_sha256)" }) -join "`n")
}

function New-QualificationResult {
  param(
    [Parameter(Mandatory)][ValidateSet('fixture', 'live-read-only')][string]$EvidenceSource,
    [Parameter(Mandatory)][object[]]$Observations,
    [Parameter(Mandatory)][string]$SourceSha256,
    [bool]$ReceiptWrite = $false
  )
  $ordered = @($observationCodes | ForEach-Object {
    $code = $_
    @($Observations | Where-Object { $_.code -ceq $code })[0]
  })
  $blockers = @($ordered | Where-Object status -ne 'pass' | ForEach-Object {
    if ($_.status -ceq 'unavailable') { "evidence_gap.$($_.code)" } else { "precondition_failed.$($_.code)" }
  } | Sort-Object -Unique)
  return [ordered]@{
    schema_version = 1
    contract = $contract
    evidence_source = $EvidenceSource
    authority = 'none'
    acceptance = $false
    real_platform_acceptance = 'not_started'
    disposition = if ($blockers.Count -eq 0) { 'STOPPED_CELL_QUALIFIED' } else { 'BLOCKED' }
    observations = $ordered
    blockers = $blockers
    evidence_digests = [ordered]@{ source_sha256 = $SourceSha256; observations_sha256 = Get-ObservationDigest -Observations $ordered }
    mutation_flags = [ordered]@{
      host_observation = $EvidenceSource -ceq 'live-read-only'
      hyperv_mutation = $false
      vm_mutation = $false
      disk_mutation = $false
      network_mutation = $false
      service_mutation = $false
      feature_mutation = $false
      receipt_write = $ReceiptWrite
    }
  }
}

function Convert-FixtureToObservations {
  param([Parameter(Mandatory)][string]$Path)
  try {
    $bytes = [IO.File]::ReadAllBytes([IO.Path]::GetFullPath($Path))
    $fixture = [Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json -ErrorAction Stop
  } catch { return $null }
  if ($fixture.schema_version -ne 1 -or $fixture.contract -cne $fixtureContract -or
      [string]$fixture.fixture_id -cnotmatch '^[a-z0-9][a-z0-9-]{2,63}$') { return $null }
  $rows = @($fixture.observations)
  if ($rows.Count -ne $observationCodes.Count) { return $null }
  $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
  $observations = [Collections.Generic.List[object]]::new()
  foreach ($row in $rows) {
    $code = [string](Get-ObjectProperty -InputObject $row -Name 'code')
    $status = [string](Get-ObjectProperty -InputObject $row -Name 'status')
    $digest = [string](Get-ObjectProperty -InputObject $row -Name 'evidence_sha256')
    if ($observationCodes -cnotcontains $code -or $status -cnotin @('pass', 'fail', 'unavailable') -or
        -not (Test-Sha256 -Value $digest) -or -not $seen.Add($code)) { return $null }
    $observations.Add((New-Observation -Code $code -Status $status -EvidenceSha256 $digest))
  }
  return [pscustomobject]@{ observations = @($observations); digest = Get-Sha256Text -Text ([Convert]::ToBase64String($bytes)) }
}

function Invoke-LiveObservation {
  param([Parameter(Mandatory)][string]$Code, [Parameter(Mandatory)][scriptblock]$Probe)
  try {
    $value = & $Probe
    $status = [string](Get-ObjectProperty -InputObject $value -Name 'status')
    if ($status -cnotin @('pass', 'fail')) { throw 'probe returned an invalid status' }
    $digest = Get-Sha256Text -Text ((Get-ObjectProperty -InputObject $value -Name 'evidence') | ConvertTo-Json -Compress -Depth 8)
    return New-Observation -Code $Code -Status $status -EvidenceSha256 $digest
  } catch {
    return New-Observation -Code $Code -Status 'unavailable' -EvidenceSha256 (Get-Sha256Text -Text "$Code|$($_.Exception.GetType().Name)")
  }
}

function Test-PathEqual {
  param([Parameter(Mandatory)][string]$Left, [Parameter(Mandatory)][string]$Right)
  return [IO.Path]::GetFullPath($Left).Equals([IO.Path]::GetFullPath($Right), [StringComparison]::OrdinalIgnoreCase)
}

function Get-LiveObservations {
  $vm = $null
  try { $matches = @(Get-VM -Id $ExpectedVmId -ErrorAction Stop); if ($matches.Count -eq 1) { $vm = $matches[0] } } catch {}
  $result = [Collections.Generic.List[object]]::new()
  $result.Add((Invoke-LiveObservation -Code 'vm_identity' -Probe {
    if ($null -eq $vm) { throw 'VM identity unavailable' }
    $exact = [Guid]$vm.Id -eq $ExpectedVmId -and [string]$vm.Name -ceq $ExpectedVmName
    [pscustomobject]@{ status = if ($exact) { 'pass' } else { 'fail' }; evidence = @{ exact = $exact } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'stopped_state' -Probe {
    if ($null -eq $vm) { throw 'VM state unavailable' }
    $stopped = [string]$vm.State -ceq 'Off'
    [pscustomobject]@{ status = if ($stopped) { 'pass' } else { 'fail' }; evidence = @{ stopped = $stopped } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'generation_2' -Probe {
    if ($null -eq $vm) { throw 'VM generation unavailable' }
    $generation2 = [int]$vm.Generation -eq 2
    [pscustomobject]@{ status = if ($generation2) { 'pass' } else { 'fail' }; evidence = @{ generation_2 = $generation2 } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'secure_boot' -Probe {
    if ($null -eq $vm) { throw 'firmware unavailable' }
    $firmware = Get-VMFirmware -VM $vm -ErrorAction Stop
    $valid = [string]$firmware.SecureBoot -ceq 'On' -and [string]$firmware.SecureBootTemplate -ceq $ExpectedSecureBootTemplate
    [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ enabled_and_template_exact = $valid } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'disk_layout' -Probe {
    if ($null -eq $vm) { throw 'disk evidence unavailable' }
    $drives = @(Get-VMHardDiskDrive -VM $vm -ErrorAction Stop)
    $valid = $drives.Count -eq 1
    if ($valid) {
      $valid = Test-PathEqual -Left ([string]$drives[0].Path) -Right $ExpectedDiskPath
      $disk = Get-VHD -Path $drives[0].Path -ErrorAction Stop
      $valid = $valid -and [string]$disk.VhdType -ceq 'Differencing' -and
        (Test-PathEqual -Left ([string]$disk.ParentPath) -Right $ExpectedDiskParentPath)
      $parent = Get-VHD -Path $ExpectedDiskParentPath -ErrorAction Stop
      $valid = $valid -and [string]::IsNullOrWhiteSpace([string]$parent.ParentPath)
    }
    [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ exact_one_level_layout = $valid } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'resources' -Probe {
    if ($null -eq $vm) { throw 'resource evidence unavailable' }
    $valid = [long]$vm.ProcessorCount -eq $ExpectedProcessorCount -and [long]$vm.MemoryStartup -eq $ExpectedMemoryStartupBytes
    [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ exact = $valid } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'network_adapters' -Probe {
    if ($null -eq $vm) { throw 'network evidence unavailable' }
    $adapters = @(Get-VMNetworkAdapter -VM $vm -ErrorAction Stop)
    [pscustomobject]@{ status = if ($adapters.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ zero = $adapters.Count -eq 0 } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'evidence_output' -Probe { Get-EvidenceOutputFacts -Path $ReceiptPath }))
  return @($result)
}

if ($PSCmdlet.ParameterSetName -eq 'Fixture') {
  $fixture = Convert-FixtureToObservations -Path $FixturePath
  if ($null -eq $fixture) {
    [ordered]@{
      schema_version = 1; contract = $contract; evidence_source = 'fixture'; authority = 'none'; acceptance = $false
      real_platform_acceptance = 'not_started'; disposition = 'BLOCKED'; observations = @(); blockers = @('fixture.schema_invalid')
      mutation_flags = [ordered]@{ host_observation = $false; hyperv_mutation = $false; vm_mutation = $false; disk_mutation = $false; network_mutation = $false; service_mutation = $false; feature_mutation = $false; receipt_write = $false }
    } | ConvertTo-Json -Compress -Depth 12
  } else {
    New-QualificationResult -EvidenceSource fixture -Observations $fixture.observations -SourceSha256 $fixture.digest |
      ConvertTo-Json -Compress -Depth 12
  }
} else {
  $observations = Get-LiveObservations
  $receiptWrite = @($observations | Where-Object { $_.code -ceq 'evidence_output' })[0].status -ceq 'pass'
  $result = New-QualificationResult -EvidenceSource live-read-only -Observations $observations `
    -SourceSha256 (Get-ObservationDigest -Observations $observations) -ReceiptWrite $receiptWrite
  $json = $result | ConvertTo-Json -Compress -Depth 12
  if ($receiptWrite) {
    Write-SanitizedReceiptCreateNew -Path $ReceiptPath -Json $json
  }
  $json
}
