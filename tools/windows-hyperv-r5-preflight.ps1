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
  'c_storage_boundary',
  'v_storage_boundary',
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

function Test-Sha256 {
  param([AllowNull()][string]$Value)

  return $null -ne $Value -and $Value -cmatch '^[0-9a-f]{64}$'
}

function Get-ObjectProperty {
  param(
    [Parameter(Mandatory)][object]$InputObject,
    [Parameter(Mandatory)][string]$Name
  )

  $property = $InputObject.PSObject.Properties[$Name]
  if ($null -eq $property) { return $null }
  return $property.Value
}

function Test-LiveProvenance {
  param([Parameter(Mandatory)][object]$Provenance)

  $requiredStrings = @(
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'candidate') -Name 'sha'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'candidate') -Name 'version'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'package') -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'candidate_binary') -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'windows') -Name 'edition'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'windows') -Name 'build'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'image_source') -Name 'kind'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'image_source') -Name 'source_sha256'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'vhdx') -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'vhdx') -Name 'generation'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'vhdx') -Name 'secure_boot'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'vhdx') -Name 'virtualization_based_security'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'creation') -Name 'created_at_utc'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'admission_receipt') -Name 'issued_at_utc'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'admission_receipt') -Name 'sha256'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'exclusive_window') -Name 'starts_at_utc'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'exclusive_window') -Name 'ends_at_utc'),
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'exclusive_window') -Name 'evidence_sha256')
  )
  $hashes = @(
    $requiredStrings[2], $requiredStrings[3], $requiredStrings[7], $requiredStrings[8],
    $requiredStrings[14], $requiredStrings[16]
  )
  $vhdx = Get-ObjectProperty -InputObject $Provenance -Name 'vhdx'
  $immutability = Get-ObjectProperty -InputObject $Provenance -Name 'immutability'
  $receipt = Get-ObjectProperty -InputObject $Provenance -Name 'admission_receipt'
  $exclusiveWindow = Get-ObjectProperty -InputObject $Provenance -Name 'exclusive_window'
  $timestampValid = $true
  foreach ($timestamp in @($requiredStrings[12], $requiredStrings[13], $requiredStrings[15], $requiredStrings[16])) {
    try { [DateTimeOffset]::Parse($timestamp) | Out-Null } catch { $timestampValid = $false }
  }
  $windowOrdered = $false
  try {
    $windowOrdered = [DateTimeOffset]::Parse($requiredStrings[15]) -lt
      [DateTimeOffset]::Parse($requiredStrings[16])
  } catch {}
  return $Provenance.schema_version -eq 1 -and
    $Provenance.contract -ceq 'vmcell.hyperv-r5-image-provenance.v1' -and
    $requiredStrings.Count -eq @($requiredStrings | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }).Count -and
    @($hashes | Where-Object { -not (Test-Sha256 -Value $_) }).Count -eq 0 -and
    [string](Get-ObjectProperty -InputObject (Get-ObjectProperty -InputObject $Provenance -Name 'windows') -Name 'architecture') -ceq 'x86_64' -and
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
    }
  }
}

function New-PreflightResult {
  param(
    [Parameter(Mandatory)][ValidateSet('fixture', 'live-read-only')][string]$EvidenceSource,
    [Parameter(Mandatory)][object[]]$Observations,
    [Parameter(Mandatory)][string]$EvidenceSourceDigest,
    [Parameter(Mandatory)][bool]$HostObservation
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
  $observationDigestInput = @($orderedObservations | ForEach-Object {
    "$($_.code)|$($_.status)|$($_.evidence_sha256)"
  }) -join "`n"
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
      observations_sha256 = Get-Sha256Text -Text $observationDigestInput
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
    [Parameter(Mandatory)][scriptblock]$Probe
  )

  try {
    $value = & $Probe
    $status = [string](Get-ObjectProperty -InputObject $value -Name 'status')
    if ($status -cnotin @('pass', 'fail')) {
      throw 'probe did not return pass or fail'
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
  $provenance = $null
  $vhd = $null
  $vhdItem = $null
  $packageHash = $null
  $binaryHash = $null
  $vhdxHash = $null

  $result = [System.Collections.Generic.List[object]]::new()
  $result.Add((Invoke-LiveObservation -Code 'elevation' -Probe {
    $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    $admin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    [pscustomobject]@{ status = if ($admin) { 'pass' } else { 'fail' }; evidence = @{ elevated = $admin } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'administrators_membership' -Probe {
    $currentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $members = @(Get-LocalGroupMember -SID 'S-1-5-32-544' -ErrorAction Stop)
    $member = @($members | Where-Object { $_.SID.Value -ceq $currentSid }).Count -eq 1
    [pscustomobject]@{ status = if ($member) { 'pass' } else { 'fail' }; evidence = @{ member = $member } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'os_build_architecture' -Probe {
    $os = Get-CimInstance -ClassName Win32_OperatingSystem -ErrorAction Stop
    $x64 = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ceq 'X64'
    $valid = $x64 -and [string]$os.Caption -match 'Windows' -and [string]$os.BuildNumber -match '^\d+$'
    [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ x64 = $x64; build = [string]$os.BuildNumber } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'hyperv_feature' -Probe {
    $feature = Get-WindowsOptionalFeature -Online -FeatureName 'Microsoft-Hyper-V-All' -ErrorAction Stop
    $enabled = [string]$feature.State -ceq 'Enabled'
    [pscustomobject]@{ status = if ($enabled) { 'pass' } else { 'fail' }; evidence = @{ enabled = $enabled } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'hyperv_module' -Probe {
    $module = @(Get-Module -ListAvailable -Name Hyper-V -ErrorAction Stop)
    [pscustomobject]@{ status = if ($module.Count -gt 0) { 'pass' } else { 'fail' }; evidence = @{ count = $module.Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'hyperv_cmdlets' -Probe {
    $names = @('Get-VM', 'Get-VHD', 'Get-VMSwitch')
    $resolved = @($names | ForEach-Object { Get-Command -Name $_ -ErrorAction Stop })
    [pscustomobject]@{ status = if ($resolved.Count -eq $names.Count) { 'pass' } else { 'fail' }; evidence = @{ count = $resolved.Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vmms_state' -Probe {
    $vmms = Get-Service -Name vmms -ErrorAction Stop
    $running = [string]$vmms.Status -ceq 'Running'
    [pscustomobject]@{ status = if ($running) { 'pass' } else { 'fail' }; evidence = @{ running = $running } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'hyperv_read_access' -Probe {
    $vms = @(Get-VM -ErrorAction Stop)
    [pscustomobject]@{ status = 'pass'; evidence = @{ count = $vms.Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vm_inventory' -Probe {
    $vms = @(Get-VM -ErrorAction Stop)
    [pscustomobject]@{ status = if ($vms.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $vms.Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'switch_inventory' -Probe {
    $switches = @(Get-VMSwitch -ErrorAction Stop)
    [pscustomobject]@{ status = if ($switches.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $switches.Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'virtualization_writers' -Probe {
    $names = @('vmwp', 'vmcompute', 'vmmem', 'qemu-system-x86_64', 'VirtualBoxVM', 'vmware-vmx')
    $processes = @(Get-Process -ErrorAction Stop | Where-Object { $names -ccontains $_.ProcessName })
    [pscustomobject]@{ status = if ($processes.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $processes.Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'runner_codex_activity' -Probe {
    $names = @('Runner.Listener', 'Runner.Worker', 'codex')
    $processes = @(Get-Process -ErrorAction Stop | Where-Object { $names -ccontains $_.ProcessName })
    [pscustomobject]@{ status = if ($processes.Count -eq 0) { 'pass' } else { 'fail' }; evidence = @{ count = $processes.Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'c_storage_boundary' -Probe {
    $state = Get-Item -LiteralPath $StateRoot -ErrorAction Stop
    $volume = Get-Volume -DriveLetter C -ErrorAction Stop
    $stateRoot = [IO.Path]::GetPathRoot([IO.Path]::GetFullPath($state.FullName))
    $suitable = $state.PSIsContainer -and $stateRoot -ceq 'C:\' -and
      [string]$volume.FileSystem -ceq 'NTFS'
    [pscustomobject]@{ status = if ($suitable) { 'pass' } else { 'fail' }; evidence = @{ suitable = $suitable } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'v_storage_boundary' -Probe {
    $volume = Get-Volume -DriveLetter V -ErrorAction Stop
    $partition = Get-Partition -DriveLetter V -ErrorAction Stop
    $disk = Get-Disk -Number $partition.DiskNumber -ErrorAction Stop
    $suitable = [string]$volume.FileSystem -ceq 'NTFS' -and [string]$disk.BusType -cne 'File Backed Virtual'
    [pscustomobject]@{ status = if ($suitable) { 'pass' } else { 'fail' }; evidence = @{ suitable = $suitable } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'immutable_vhdx_presence' -Probe {
    $vhdItem = Get-Item -LiteralPath $VhdxPath -ErrorAction Stop
    $exists = -not $vhdItem.PSIsContainer -and $vhdItem.Extension -ieq '.vhdx'
    [pscustomobject]@{ status = if ($exists) { 'pass' } else { 'fail' }; evidence = @{ present = $exists } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_immutability' -Probe {
    $vhdItem = Get-Item -LiteralPath $VhdxPath -ErrorAction Stop
    $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
    $readOnly = ($vhdItem.Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0
    $fixed = [string]$vhd.VhdType -ceq 'Fixed'
    [pscustomobject]@{ status = if ($readOnly -and $fixed) { 'pass' } else { 'fail' }; evidence = @{ readonly = $readOnly; fixed = $fixed } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_attachment' -Probe {
    $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
    $detached = -not [bool]$vhd.Attached
    [pscustomobject]@{ status = if ($detached) { 'pass' } else { 'fail' }; evidence = @{ detached = $detached } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_parent' -Probe {
    $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
    $parentless = [string]::IsNullOrWhiteSpace([string]$vhd.ParentPath)
    [pscustomobject]@{ status = if ($parentless) { 'pass' } else { 'fail' }; evidence = @{ parentless = $parentless } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'image_provenance' -Probe {
    $provenance = [IO.File]::ReadAllText([IO.Path]::GetFullPath($ProvenancePath)) | ConvertFrom-Json -ErrorAction Stop
    $valid = Test-LiveProvenance -Provenance $provenance
    [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ valid = $valid } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'candidate_hash' -Probe {
    $provenance = [IO.File]::ReadAllText([IO.Path]::GetFullPath($ProvenancePath)) | ConvertFrom-Json -ErrorAction Stop
    $matches = [string]$provenance.candidate.sha -ceq $CandidateSha
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'package_hash' -Probe {
    $provenance = [IO.File]::ReadAllText([IO.Path]::GetFullPath($ProvenancePath)) | ConvertFrom-Json -ErrorAction Stop
    $packageHash = Get-Sha256File -Path $CandidatePackagePath
    $matches = [string]$provenance.package.sha256 -ceq $packageHash
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'binary_hash' -Probe {
    $provenance = [IO.File]::ReadAllText([IO.Path]::GetFullPath($ProvenancePath)) | ConvertFrom-Json -ErrorAction Stop
    $binaryHash = Get-Sha256File -Path $CandidateBinaryPath
    $matches = [string]$provenance.candidate_binary.sha256 -ceq $binaryHash
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_hash' -Probe {
    $provenance = [IO.File]::ReadAllText([IO.Path]::GetFullPath($ProvenancePath)) | ConvertFrom-Json -ErrorAction Stop
    $vhdxHash = Get-Sha256File -Path $VhdxPath
    $matches = [string]$provenance.vhdx.sha256 -ceq $vhdxHash
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'admission_receipt' -Probe {
    $provenance = [IO.File]::ReadAllText([IO.Path]::GetFullPath($ProvenancePath)) | ConvertFrom-Json -ErrorAction Stop
    $issued = [DateTimeOffset]::Parse([string]$provenance.admission_receipt.issued_at_utc)
    $fresh = ([DateTimeOffset]::UtcNow - $issued).TotalHours -ge 0 -and
      ([DateTimeOffset]::UtcNow - $issued).TotalHours -le $MaximumReceiptAgeHours
    [pscustomobject]@{ status = if ($fresh) { 'pass' } else { 'fail' }; evidence = @{ fresh = $fresh } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'exclusive_window' -Probe {
    $provenance = [IO.File]::ReadAllText([IO.Path]::GetFullPath($ProvenancePath)) | ConvertFrom-Json -ErrorAction Stop
    $eligible = $provenance.exclusive_window.eligible -is [bool] -and
      [bool]$provenance.exclusive_window.eligible -and
      [DateTimeOffset]::Parse([string]$provenance.exclusive_window.ends_at_utc) -gt [DateTimeOffset]::UtcNow
    [pscustomobject]@{ status = if ($eligible) { 'pass' } else { 'fail' }; evidence = @{ eligible = $eligible } }
  }))
  return @($result)
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
  New-PreflightResult -EvidenceSource 'live-read-only' -Observations $liveObservations `
    -EvidenceSourceDigest (Get-Sha256Text -Text ([DateTimeOffset]::UtcNow.ToString('O'))) `
    -HostObservation $true |
    ConvertTo-Json -Compress -Depth 12
}
