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
    available = $true
    parent_ordinary = [bool]$parent.PSIsContainer
    target_absent = -not (Test-Path -LiteralPath $fullPath)
  }
}

function Write-SanitizedReceiptCreateNew {
  param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Json)
  $facts = Get-EvidenceOutputFacts -Path $Path
  if (-not $facts.available -or -not $facts.parent_ordinary -or -not $facts.target_absent) {
    throw 'receipt output contract is not satisfied'
  }
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

function Test-PathEqual {
  param([Parameter(Mandatory)][string]$Left, [Parameter(Mandatory)][string]$Right)
  return [IO.Path]::GetFullPath($Left).Equals([IO.Path]::GetFullPath($Right), [StringComparison]::OrdinalIgnoreCase)
}

function Get-RawFactProperty {
  param([Parameter(Mandatory)][object]$Fact, [Parameter(Mandatory)][string]$Name)
  $property = $Fact.PSObject.Properties[$Name]
  if ($null -eq $property) { throw "stopped-cell raw facts omitted $Name" }
  return $property.Value
}

function Get-RawFactBoolean {
  param([Parameter(Mandatory)][object]$Fact, [Parameter(Mandatory)][string]$Name)
  $value = Get-RawFactProperty -Fact $Fact -Name $Name
  if ($value -isnot [bool]) { throw "stopped-cell raw fact $Name was not boolean" }
  return [bool]$value
}

function Get-RawFactString {
  param([Parameter(Mandatory)][object]$Fact, [Parameter(Mandatory)][string]$Name)
  $value = Get-RawFactProperty -Fact $Fact -Name $Name
  if ($value -isnot [string] -or [string]::IsNullOrWhiteSpace($value)) { throw "stopped-cell raw fact $Name was not a non-empty string" }
  return [string]$value
}

function Get-RawFactInt64 {
  param([Parameter(Mandatory)][object]$Fact, [Parameter(Mandatory)][string]$Name, [long]$Minimum = 0)
  $value = Get-RawFactProperty -Fact $Fact -Name $Name
  $parsed = 0L
  if ($value -is [bool] -or -not [long]::TryParse([string]$value, [ref]$parsed) -or $parsed -lt $Minimum) {
    throw "stopped-cell raw fact $Name was not an admitted integer"
  }
  return $parsed
}

function New-RawClassificationObservation {
  param(
    [Parameter(Mandatory)][string]$Code,
    [Parameter(Mandatory)][bool]$Available,
    [Parameter(Mandatory)][bool]$Valid,
    [Parameter(Mandatory)][hashtable]$Evidence
  )
  $status = if (-not $Available) { 'unavailable' } elseif ($Valid) { 'pass' } else { 'fail' }
  return New-Observation -Code $Code -Status $status -EvidenceSha256 (Get-Sha256Text -Text ($Evidence | ConvertTo-Json -Compress -Depth 8))
}

function Convert-StoppedCellRawFactsToObservations {
  param([Parameter(Mandatory)][object]$RawFacts)

  $expected = Get-RawFactProperty -Fact $RawFacts -Name 'expected'
  $vm = Get-RawFactProperty -Fact $RawFacts -Name 'vm'
  $firmware = Get-RawFactProperty -Fact $RawFacts -Name 'firmware'
  $disks = Get-RawFactProperty -Fact $RawFacts -Name 'disks'
  $network = Get-RawFactProperty -Fact $RawFacts -Name 'network_adapters'
  $output = Get-RawFactProperty -Fact $RawFacts -Name 'evidence_output'
  $expectedVmId = Get-RawFactString -Fact $expected -Name 'vm_id'
  $expectedVmName = Get-RawFactString -Fact $expected -Name 'vm_name'
  $expectedDiskPath = Get-RawFactString -Fact $expected -Name 'disk_path'
  $expectedParentPath = Get-RawFactString -Fact $expected -Name 'disk_parent_path'
  $expectedProcessorCount = Get-RawFactInt64 -Fact $expected -Name 'processor_count' -Minimum 1
  $expectedMemoryStartupBytes = Get-RawFactInt64 -Fact $expected -Name 'memory_startup_bytes' -Minimum 1
  $expectedTemplate = Get-RawFactString -Fact $expected -Name 'secure_boot_template'
  $expectedGuid = [Guid]::Empty
  if (-not [Guid]::TryParse($expectedVmId, [ref]$expectedGuid)) { throw 'stopped-cell expected VM ID was malformed' }

  $vmAvailable = Get-RawFactBoolean -Fact $vm -Name 'available'
  $vmId = $null; $vmName = $null; $vmState = $null; $generation = $null; $processors = $null; $memory = $null
  if ($vmAvailable) {
    $vmId = Get-RawFactString -Fact $vm -Name 'id'
    $vmName = Get-RawFactString -Fact $vm -Name 'name'
    $vmState = Get-RawFactString -Fact $vm -Name 'state'
    $generation = Get-RawFactInt64 -Fact $vm -Name 'generation' -Minimum 1
    $processors = Get-RawFactInt64 -Fact $vm -Name 'processor_count' -Minimum 1
    $memory = Get-RawFactInt64 -Fact $vm -Name 'memory_startup_bytes' -Minimum 1
  }

  $identityValid = $false
  if ($vmAvailable) {
    $actualGuid = [Guid]::Empty
    if (-not [Guid]::TryParse($vmId, [ref]$actualGuid)) { throw 'stopped-cell raw VM ID was malformed' }
    $identityValid = $actualGuid -eq $expectedGuid -and $vmName -ceq $expectedVmName
  }
  $observations = [Collections.Generic.List[object]]::new()
  $observations.Add((New-RawClassificationObservation -Code 'vm_identity' -Available $vmAvailable -Valid $identityValid -Evidence @{ exact = $identityValid }))
  $stopped = $vmAvailable -and $vmState -ceq 'Off'
  $observations.Add((New-RawClassificationObservation -Code 'stopped_state' -Available $vmAvailable -Valid $stopped -Evidence @{ stopped = $stopped }))
  $generation2 = $vmAvailable -and $generation -eq 2
  $observations.Add((New-RawClassificationObservation -Code 'generation_2' -Available $vmAvailable -Valid $generation2 -Evidence @{ generation_2 = $generation2 }))

  $firmwareAvailable = Get-RawFactBoolean -Fact $firmware -Name 'available'
  $secureBootValid = $false
  if ($firmwareAvailable) {
    $secureBoot = Get-RawFactString -Fact $firmware -Name 'secure_boot'
    $template = Get-RawFactString -Fact $firmware -Name 'secure_boot_template'
    $secureBootValid = $secureBoot -ceq 'On' -and $template -ceq $expectedTemplate
  }
  $observations.Add((New-RawClassificationObservation -Code 'secure_boot' -Available $firmwareAvailable -Valid $secureBootValid -Evidence @{ enabled_and_template_exact = $secureBootValid }))

  $disksAvailable = Get-RawFactBoolean -Fact $disks -Name 'available'
  $diskLayoutValid = $false
  if ($disksAvailable) {
    $entries = @(Get-RawFactProperty -Fact $disks -Name 'entries')
    $diskLayoutValid = $entries.Count -eq 1
    if ($diskLayoutValid) {
      $entry = $entries[0]
      $entryPath = Get-RawFactString -Fact $entry -Name 'path'
      $vhdType = Get-RawFactString -Fact $entry -Name 'vhd_type'
      $parentPath = Get-RawFactString -Fact $entry -Name 'parent_path'
      $parentParentPath = Get-RawFactProperty -Fact $entry -Name 'parent_parent_path'
      if ($null -ne $parentParentPath -and $parentParentPath -isnot [string]) { throw 'stopped-cell raw parent parent path was malformed' }
      $diskLayoutValid = (Test-PathEqual -Left $entryPath -Right $expectedDiskPath) -and
        $vhdType -ceq 'Differencing' -and
        (Test-PathEqual -Left $parentPath -Right $expectedParentPath) -and
        [string]::IsNullOrWhiteSpace([string]$parentParentPath)
    }
  }
  $observations.Add((New-RawClassificationObservation -Code 'disk_layout' -Available $disksAvailable -Valid $diskLayoutValid -Evidence @{ exact_one_level_layout = $diskLayoutValid }))

  $resourcesValid = $vmAvailable -and $processors -eq $expectedProcessorCount -and $memory -eq $expectedMemoryStartupBytes
  $observations.Add((New-RawClassificationObservation -Code 'resources' -Available $vmAvailable -Valid $resourcesValid -Evidence @{ exact = $resourcesValid }))

  $networkAvailable = Get-RawFactBoolean -Fact $network -Name 'available'
  $networkCount = 0L
  if ($networkAvailable) { $networkCount = Get-RawFactInt64 -Fact $network -Name 'count' -Minimum 0 }
  $networkValid = $networkAvailable -and $networkCount -eq 0
  $observations.Add((New-RawClassificationObservation -Code 'network_adapters' -Available $networkAvailable -Valid $networkValid -Evidence @{ zero = $networkValid }))

  $outputAvailable = Get-RawFactBoolean -Fact $output -Name 'available'
  $outputValid = $false
  if ($outputAvailable) {
    $outputValid = (Get-RawFactBoolean -Fact $output -Name 'parent_ordinary') -and
      (Get-RawFactBoolean -Fact $output -Name 'target_absent')
  }
  $observations.Add((New-RawClassificationObservation -Code 'evidence_output' -Available $outputAvailable -Valid $outputValid -Evidence @{ parent_ordinary_and_target_absent = $outputValid }))
  return @($observations)
}

function Test-ClassificationOnlyRawFacts {
  param([AllowNull()][object]$Value)
  if ($null -eq $Value -or $Value -is [string] -or $Value -is [ValueType]) { return $true }
  if ($Value -is [System.Collections.IEnumerable] -and $Value -isnot [pscustomobject]) {
    foreach ($entry in $Value) {
      if (-not (Test-ClassificationOnlyRawFacts -Value $entry)) { return $false }
    }
    return $true
  }
  foreach ($property in $Value.PSObject.Properties) {
    if ($property.Name -cin @('observations', 'status', 'disposition', 'blockers', 'readiness', 'eligibility')) { return $false }
    if (-not (Test-ClassificationOnlyRawFacts -Value $property.Value)) { return $false }
  }
  return $true
}

function Convert-FixtureToRawFacts {
  param([Parameter(Mandatory)][string]$Path)
  try {
    $bytes = [IO.File]::ReadAllBytes([IO.Path]::GetFullPath($Path))
    $fixture = [Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json -ErrorAction Stop
    if ($fixture.schema_version -ne 1 -or $fixture.contract -cne $fixtureContract -or
        [string]$fixture.fixture_id -cnotmatch '^[a-z0-9][a-z0-9-]{2,63}$' -or
        $null -eq $fixture.PSObject.Properties['raw_facts']) { return $null }
    foreach ($prohibited in @('observations', 'disposition', 'blockers', 'readiness', 'eligibility')) {
      if ($null -ne $fixture.PSObject.Properties[$prohibited]) { return $null }
    }
    if (-not (Test-ClassificationOnlyRawFacts -Value $fixture.raw_facts)) { return $null }
    return [pscustomobject]@{ raw_facts = $fixture.raw_facts; digest = Get-Sha256Text -Text ([Convert]::ToBase64String($bytes)) }
  } catch { return $null }
}

function Get-LiveRawFacts {
  $raw = [ordered]@{
    expected = [ordered]@{ vm_id = $ExpectedVmId.ToString(); vm_name = $ExpectedVmName; disk_path = $ExpectedDiskPath; disk_parent_path = $ExpectedDiskParentPath; processor_count = $ExpectedProcessorCount; memory_startup_bytes = $ExpectedMemoryStartupBytes; secure_boot_template = $ExpectedSecureBootTemplate }
    vm = [ordered]@{ available = $false }
    firmware = [ordered]@{ available = $false }
    disks = [ordered]@{ available = $false }
    network_adapters = [ordered]@{ available = $false }
    evidence_output = [ordered]@{ available = $false }
  }
  $vm = $null
  try {
    $matches = @(Get-VM -Id $ExpectedVmId -ErrorAction Stop)
    if ($matches.Count -eq 1) {
      $vm = $matches[0]
      $raw.vm = [ordered]@{ available = $true; id = [string]$vm.Id; name = [string]$vm.Name; state = [string]$vm.State; generation = [int]$vm.Generation; processor_count = [int]$vm.ProcessorCount; memory_startup_bytes = [long]$vm.MemoryStartup }
    }
  } catch {}
  if ($null -ne $vm) {
    try { $firmware = Get-VMFirmware -VM $vm -ErrorAction Stop; $raw.firmware = [ordered]@{ available = $true; secure_boot = [string]$firmware.SecureBoot; secure_boot_template = [string]$firmware.SecureBootTemplate } } catch {}
    try {
      $drives = @(Get-VMHardDiskDrive -VM $vm -ErrorAction Stop)
      $entries = [Collections.Generic.List[object]]::new()
      foreach ($drive in $drives) {
        $disk = Get-VHD -Path $drive.Path -ErrorAction Stop
        $parent = Get-VHD -Path $disk.ParentPath -ErrorAction Stop
        $entries.Add([ordered]@{ path = [string]$drive.Path; vhd_type = [string]$disk.VhdType; parent_path = [string]$disk.ParentPath; parent_parent_path = $parent.ParentPath })
      }
      $raw.disks = [ordered]@{ available = $true; entries = @($entries) }
    } catch {}
    try { $adapters = @(Get-VMNetworkAdapter -VM $vm -ErrorAction Stop); $raw.network_adapters = [ordered]@{ available = $true; count = $adapters.Count } } catch {}
  }
  try { $facts = Get-EvidenceOutputFacts -Path $ReceiptPath; $raw.evidence_output = [ordered]@{ available = [bool]$facts.available; parent_ordinary = [bool]$facts.parent_ordinary; target_absent = [bool]$facts.target_absent } } catch {}
  return [pscustomobject]$raw
}

if ($PSCmdlet.ParameterSetName -eq 'Fixture') {
  $fixture = Convert-FixtureToRawFacts -Path $FixturePath
  if ($null -eq $fixture) {
    [ordered]@{
      schema_version = 1; contract = $contract; evidence_source = 'fixture'; authority = 'none'; acceptance = $false
      real_platform_acceptance = 'not_started'; disposition = 'BLOCKED'; observations = @(); blockers = @('fixture.schema_invalid')
      mutation_flags = [ordered]@{ host_observation = $false; hyperv_mutation = $false; vm_mutation = $false; disk_mutation = $false; network_mutation = $false; service_mutation = $false; feature_mutation = $false; receipt_write = $false }
    } | ConvertTo-Json -Compress -Depth 12
  } else {
    try {
      $observations = Convert-StoppedCellRawFactsToObservations -RawFacts $fixture.raw_facts
      New-QualificationResult -EvidenceSource fixture -Observations $observations -SourceSha256 $fixture.digest |
        ConvertTo-Json -Compress -Depth 12
    } catch {
      [ordered]@{
        schema_version = 1; contract = $contract; evidence_source = 'fixture'; authority = 'none'; acceptance = $false
        real_platform_acceptance = 'not_started'; disposition = 'BLOCKED'; observations = @(); blockers = @('fixture.schema_invalid')
        mutation_flags = [ordered]@{ host_observation = $false; hyperv_mutation = $false; vm_mutation = $false; disk_mutation = $false; network_mutation = $false; service_mutation = $false; feature_mutation = $false; receipt_write = $false }
      } | ConvertTo-Json -Compress -Depth 12
    }
  }
} else {
  $observations = Convert-StoppedCellRawFactsToObservations -RawFacts (Get-LiveRawFacts)
  $receiptWrite = @($observations | Where-Object { $_.code -ceq 'evidence_output' })[0].status -ceq 'pass'
  $result = New-QualificationResult -EvidenceSource live-read-only -Observations $observations `
    -SourceSha256 (Get-ObservationDigest -Observations $observations) -ReceiptWrite $receiptWrite
  $json = $result | ConvertTo-Json -Compress -Depth 12
  if ($receiptWrite) {
    Write-SanitizedReceiptCreateNew -Path $ReceiptPath -Json $json
  }
  $json
}
