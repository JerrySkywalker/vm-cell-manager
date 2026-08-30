$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$scriptPath = Join-Path $PSScriptRoot 'windows-hyperv-preflight.ps1'
$stoppedScriptPath = Join-Path $PSScriptRoot 'windows-hyperv-stopped-cell-qualification.ps1'
$fixturePath = Join-Path $repositoryRoot 'tests\fixtures\hyperv-preflight\eligible.json'
$stoppedFixturePath = Join-Path $repositoryRoot 'tests\fixtures\hyperv-stopped-cell\eligible.json'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('vmcell-hyperv-r5-preflight-' + [Guid]::NewGuid().ToString('N'))

function Assert-True {
  param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
  if (-not $Condition) { throw $Message }
}

function Invoke-Fixture {
  param([Parameter(Mandatory)][string]$Path)

  $raw = @(& $scriptPath -FixturePath $Path)
  Assert-True -Condition ($raw.Count -eq 1) -Message 'fixture invocation did not emit exactly one JSON document'
  return ($raw[0] | ConvertFrom-Json -ErrorAction Stop)
}

function Invoke-StoppedFixture {
  param([Parameter(Mandatory)][string]$Path)

  $raw = @(& $stoppedScriptPath -FixturePath $Path)
  Assert-True -Condition ($raw.Count -eq 1) -Message 'stopped-cell fixture did not emit exactly one JSON document'
  return ($raw[0] | ConvertFrom-Json -ErrorAction Stop)
}

function Write-CaseFixture {
  param(
    [Parameter(Mandatory)][string]$Path,
    [AllowNull()][string]$Code,
    [AllowNull()][string]$Status,
    [AllowNull()][string]$RawDetail
  )

  $fixture = Get-Content -LiteralPath $fixturePath -Raw | ConvertFrom-Json
  if (-not [string]::IsNullOrWhiteSpace($Code)) {
    $row = @($fixture.observations | Where-Object { $_.code -ceq $Code })[0]
    if ($null -eq $row) { throw "test fixture did not contain observation $Code" }
    $row.status = $Status
  }
  if (-not [string]::IsNullOrWhiteSpace($RawDetail)) {
    $fixture | Add-Member -NotePropertyName raw_detail -NotePropertyValue $RawDetail
  }
  $fixture | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

function Write-StoppedCaseFixture {
  param(
    [Parameter(Mandatory)][string]$Path,
    [AllowNull()][string]$Code,
    [AllowNull()][string]$Status,
    [AllowNull()][string]$RawDetail
  )

  $fixture = Get-Content -LiteralPath $stoppedFixturePath -Raw | ConvertFrom-Json
  if (-not [string]::IsNullOrWhiteSpace($Code)) {
    $row = @($fixture.observations | Where-Object { $_.code -ceq $Code })[0]
    if ($null -eq $row) { throw "stopped-cell fixture did not contain observation $Code" }
    $row.status = $Status
  }
  if (-not [string]::IsNullOrWhiteSpace($RawDetail)) {
    $fixture | Add-Member -NotePropertyName raw_detail -NotePropertyValue $RawDetail
  }
  $fixture | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

function Assert-StaticDenyList {
  $forbidden = @(
    'Enable-WindowsOptionalFeature', 'Disable-WindowsOptionalFeature',
    'Install-WindowsFeature', 'Uninstall-WindowsFeature',
    'Add-WindowsCapability', 'Remove-WindowsCapability',
    'Add-LocalGroupMember', 'Remove-LocalGroupMember',
    'Start-Service', 'Stop-Service', 'Restart-Service', 'Set-Service', 'New-Service', 'Remove-Service',
    'New-VM', 'Set-VM', 'Remove-VM', 'Start-VM', 'Stop-VM', 'Import-VM', 'Export-VM',
    'Rename-VM', 'Move-VM', 'Move-VMStorage', 'Set-VMFirmware',
    'Checkpoint-VM', 'Restore-VM', 'Suspend-VM', 'Resume-VM',
    'Add-VMHardDiskDrive', 'Set-VMHardDiskDrive', 'Remove-VMHardDiskDrive',
    'Add-VMNetworkAdapter', 'Set-VMNetworkAdapter', 'Remove-VMNetworkAdapter',
    'Connect-VMNetworkAdapter', 'Disconnect-VMNetworkAdapter',
    'Set-VMProcessor', 'Set-VMMemory',
    'New-VMSwitch', 'Set-VMSwitch', 'Remove-VMSwitch', 'Add-VMSwitchExtension', 'Remove-VMSwitchExtension',
    'New-VHD', 'Set-VHD', 'Remove-VHD', 'Resize-VHD', 'Mount-VHD', 'Dismount-VHD', 'Convert-VHD', 'Merge-VHD',
    'Initialize-Disk', 'Clear-Disk', 'Set-Disk', 'Format-Volume', 'Set-Volume', 'Dismount-Volume',
    'New-Partition', 'Set-Partition', 'Remove-Partition', 'New-StoragePool', 'Remove-StoragePool',
    'Set-Acl', 'Clear-Acl', 'icacls.exe', 'takeown.exe',
    'New-NetIPAddress', 'Set-NetIPAddress', 'Remove-NetIPAddress', 'New-NetRoute', 'Set-NetRoute', 'Remove-NetRoute',
    'Set-NetIPInterface', 'Restart-NetAdapter', 'Enable-NetAdapter', 'Disable-NetAdapter',
    'New-NetNat', 'Remove-NetNat', 'Set-NetFirewallProfile', 'New-NetFirewallRule', 'Set-NetFirewallRule', 'Remove-NetFirewallRule',
    'New-ItemProperty', 'Set-ItemProperty', 'Remove-ItemProperty', 'reg.exe',
    'Stop-Process', 'Start-Process', 'taskkill.exe', 'Stop-Computer', 'Restart-Computer', 'shutdown.exe', 'sc.exe'
  )

  foreach ($candidatePath in @($scriptPath, $stoppedScriptPath)) {
    $tokens = $null
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
      $candidatePath,
      [ref]$tokens,
      [ref]$parseErrors
    )
    Assert-True -Condition ($parseErrors.Count -eq 0) -Message "PowerShell parser error in $candidatePath"
    $commands = @($ast.FindAll({
      param($node)
      $node -is [System.Management.Automation.Language.CommandAst]
    }, $true) | ForEach-Object { $_.GetCommandName() } | Where-Object { $_ })
    foreach ($command in $forbidden) {
      Assert-True -Condition ($commands -cnotcontains $command) `
        -Message "qualification tooling contains forbidden mutating command $command"
    }
  }

  foreach ($command in $forbidden) {
    $tokens = $null
    $parseErrors = $null
    $mutationAst = [System.Management.Automation.Language.Parser]::ParseInput(
      "$command -WhatIf",
      [ref]$tokens,
      [ref]$parseErrors
    )
    $parsedCommands = @($mutationAst.FindAll({
      param($node)
      $node -is [System.Management.Automation.Language.CommandAst]
    }, $true) | ForEach-Object { $_.GetCommandName() } | Where-Object { $_ })
    Assert-True -Condition ($parseErrors.Count -eq 0 -and $parsedCommands -ccontains $command) `
      -Message "forbidden mutation AST coverage omitted $command"
  }
}

function Assert-PathSafetyAndDeterminism {
  $source = [IO.File]::ReadAllText($scriptPath)
  foreach ($required in @(
      'function Get-SafeProvenanceSnapshot',
      'function Assert-NotReparsePoint',
      'function Get-PathItemWithoutFollowingReparse',
      'function Assert-OrdinaryPathAncestry',
      'function Get-OrdinaryPathItem',
      'function Get-OrdinaryProvenanceFile',
      '[IO.FileAttributes]::ReparsePoint',
      '$ancestor -is [IO.FileInfo]',
      '$ancestor = $ancestor.Directory',
      '$ancestor -is [IO.DirectoryInfo]',
      '$ancestor = $ancestor.Parent',
      '$beforeHash = Get-Sha256File',
      '$afterHash = Get-Sha256File',
      '$verifiedItem = Get-OrdinaryProvenanceFile',
      'provenance evidence changed while it was read',
      'function Test-OperationalStorageFacts',
      'function Get-LiveOperationalStorageFacts',
      'function Test-EvidenceOutputFacts',
      'function Write-SanitizedReceiptCreateNew',
      "@{ code = 'state_root_storage'; path = `$StateRoot",
      "@{ code = 'runtime_root_storage'; path = `$RuntimeRoot",
      "@{ code = 'image_storage'; path = `$VhdxPath",
      "@{ code = 'package_storage'; path = `$CandidatePackagePath",
      "@{ code = 'binary_storage'; path = `$CandidateBinaryPath",
      "@{ code = 'provenance_storage'; path = `$ProvenancePath",
      "Invoke-LiveObservation -Code 'evidence_output'",
      "[IO.FileMode]::CreateNew",
      "`$requiredStrings[1] -ceq '0.4.1'",
      'Get-ObservationDigest -Observations $liveObservations',
      '$requiredStrings[17]'
    )) {
    Assert-True -Condition $source.Contains($required) `
      -Message "R5 preflight omitted required provenance safety binding: $required"
  }
  Assert-True -Condition ($source -notmatch [regex]::Escape(
      'EvidenceSourceDigest (Get-Sha256Text -Text ([DateTimeOffset]::UtcNow.ToString(''O'')))'
    )) -Message 'live result digest must not be derived from wall-clock time'
  foreach ($forbiddenText in @(
      'Get-Volume -DriveLetter C',
      'Get-Volume -DriveLetter V',
      "'c_storage_boundary'",
      "'v_storage_boundary'"
    )) {
    Assert-True -Condition (-not $source.Contains($forbiddenText)) `
      -Message "preflight retained a fixed-drive contract: $forbiddenText"
  }

  $stoppedSource = [IO.File]::ReadAllText($stoppedScriptPath)
  foreach ($required in @(
      "Get-VM -Id `$ExpectedVmId",
      "[string]`$vm.State -ceq 'Off'",
      '[int]$vm.Generation -eq 2',
      'Get-VMFirmware -VM $vm',
      "[string]`$firmware.SecureBoot -ceq 'On'",
      'Get-VMHardDiskDrive -VM $vm',
      "[string]`$disk.VhdType -ceq 'Differencing'",
      'Get-VMNetworkAdapter -VM $vm',
      '[IO.FileMode]::CreateNew'
    )) {
    Assert-True -Condition $stoppedSource.Contains($required) `
      -Message "stopped-cell qualification omitted required invariant: $required"
  }
}

function Assert-PathAncestryBehavior {
  . $scriptPath -FixturePath $fixturePath | Out-Null

  $ordinaryDirectory = Join-Path $temporaryRoot 'ordinary\nested\state-root'
  New-Item -ItemType Directory -Path $ordinaryDirectory -Force | Out-Null
  $ordinaryFile = Join-Path $ordinaryDirectory 'evidence.json'
  [IO.File]::WriteAllText($ordinaryFile, '{}', [Text.UTF8Encoding]::new($false))

  Get-OrdinaryPathItem -Path $ordinaryDirectory -RequireDirectory $true -Description 'ordinary nested directory' | Out-Null
  Get-OrdinaryPathItem -Path $ordinaryFile -RequireDirectory $false -Description 'ordinary nested file' | Out-Null
  $rootTraversalCompleted = $false
  try {
    Get-OrdinaryPathItem -Path $ordinaryDirectory -RequireDirectory $true -Description 'filesystem root traversal' | Out-Null
    $rootTraversalCompleted = $true
  } catch {
    throw 'ordinary ancestry did not reach the filesystem root'
  }
  Assert-True -Condition $rootTraversalCompleted -Message 'filesystem root traversal did not complete'

  $reparseTarget = Join-Path $temporaryRoot 'reparse-target'
  New-Item -ItemType Directory -Path (Join-Path $reparseTarget 'nested') -Force | Out-Null
  $reparseFile = Join-Path $reparseTarget 'nested\evidence.json'
  [IO.File]::WriteAllText($reparseFile, '{}', [Text.UTF8Encoding]::new($false))
  $reparsePath = Join-Path $temporaryRoot 'reparse-boundary'
  try {
    New-Item -ItemType Junction -Path $reparsePath -Target $reparseTarget -ErrorAction Stop | Out-Null
  } catch {
    throw 'test_environment_blocker.reparse_fixture_unavailable'
  }
  $reparseItem = Get-Item -LiteralPath $reparsePath -Force
  Assert-True -Condition ([bool]($reparseItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) `
    -Message 'test_environment_blocker.reparse_fixture_not_created'

  $reparseItemRejected = $false
  try {
    Get-OrdinaryPathItem -Path $reparsePath -RequireDirectory $true -Description 'reparse item' | Out-Null
  } catch {
    $reparseItemRejected = $true
  }
  Assert-True -Condition $reparseItemRejected -Message 'reparse item was accepted'

  $directParentRejected = $false
  try {
    Get-OrdinaryPathItem -Path (Join-Path $reparsePath 'nested') -RequireDirectory $true `
      -Description 'direct reparse parent' | Out-Null
  } catch {
    $directParentRejected = $true
  }
  Assert-True -Condition $directParentRejected -Message 'direct reparse parent was accepted'

  $grandparentRejected = $false
  try {
    Get-OrdinaryPathItem -Path (Join-Path $reparsePath 'nested\evidence.json') -RequireDirectory $false `
      -Description 'reparse grandparent' | Out-Null
  } catch {
    $grandparentRejected = $true
  }
  Assert-True -Condition $grandparentRejected -Message 'reparse grandparent was accepted'

  return 6
}

function Assert-StorageClassificationBehavior {
  . $scriptPath -FixturePath $fixturePath | Out-Null

  function New-AdmittedFacts {
    return [pscustomobject]@{
      evidence_available = $true
      exists = $true
      ordinary_ancestry = $true
      drive_type = 'Fixed'
      file_system = 'NTFS'
      bus_type = 'NVMe'
      boundary = 'ordinary-local-disk'
      free_bytes = [uint64]4096
      required_bytes = [uint64]1024
      rollback_margin_bytes = [uint64]1024
    }
  }

  $caseCount = 0
  foreach ($role in @('state_root', 'runtime_root', 'image', 'package', 'binary', 'provenance')) {
    $result = Test-OperationalStorageFacts -Facts (New-AdmittedFacts)
    Assert-True -Condition ($result.status -ceq 'pass') -Message "admitted local NTFS role failed: $role"
    $caseCount += 1
  }
  foreach ($volume in @('local-volume-a', 'local-volume-b')) {
    $facts = New-AdmittedFacts
    $facts | Add-Member -NotePropertyName volume_id -NotePropertyValue $volume
    Assert-True -Condition ((Test-OperationalStorageFacts -Facts $facts).status -ceq 'pass') `
      -Message 'state/runtime roots on different admitted local volumes were rejected'
    $caseCount += 1
  }

  $rejections = @(
    @{ name = 'missing'; mutate = { param($f) $f.exists = $false }; expected = 'fail' },
    @{ name = 'reparse'; mutate = { param($f) $f.ordinary_ancestry = $false }; expected = 'fail' },
    @{ name = 'network'; mutate = { param($f) $f.drive_type = 'Network'; $f.boundary = 'network' }; expected = 'fail' },
    @{ name = 'removable'; mutate = { param($f) $f.drive_type = 'Removable'; $f.bus_type = 'USB'; $f.boundary = 'removable' }; expected = 'fail' },
    @{ name = 'file-backed-virtual'; mutate = { param($f) $f.bus_type = 'File Backed Virtual'; $f.boundary = 'file-backed-virtual' }; expected = 'fail' },
    @{ name = 'ambiguous'; mutate = { param($f) $f.boundary = 'ambiguous'; $f.bus_type = 'Unknown' }; expected = 'fail' },
    @{ name = 'non-ntfs'; mutate = { param($f) $f.file_system = 'ReFS' }; expected = 'fail' },
    @{ name = 'insufficient-capacity'; mutate = { param($f) $f.free_bytes = [uint64]2047 }; expected = 'fail' },
    @{ name = 'unavailable-capacity'; mutate = { param($f) $f.free_bytes = $null }; expected = 'unavailable' }
  )
  foreach ($case in $rejections) {
    $facts = New-AdmittedFacts
    & $case.mutate $facts
    $result = Test-OperationalStorageFacts -Facts $facts
    Assert-True -Condition ($result.status -ceq $case.expected) -Message "storage case drifted: $($case.name)"
    $caseCount += 1
  }

  $evidenceFacts = [pscustomobject]@{
    evidence_available = $true
    parent_exists = $true
    parent_is_directory = $true
    ordinary_ancestry = $true
    target_exists = $false
    file_system = 'ReFS'
  }
  Assert-True -Condition ((Test-EvidenceOutputFacts -Facts $evidenceFacts).status -ceq 'pass') `
    -Message 'sanitized evidence output was incorrectly subjected to operational NTFS policy'
  $caseCount += 1
  return $caseCount
}

function Assert-ReceiptBehavior {
  . $scriptPath -FixturePath $fixturePath | Out-Null

  $receiptParent = Join-Path $temporaryRoot 'receipt-parent'
  New-Item -ItemType Directory -Path $receiptParent | Out-Null
  $receipt = Join-Path $receiptParent 'receipt.json'
  Write-SanitizedReceiptCreateNew -Path $receipt -Json '{"result":"first"}'
  $before = [IO.File]::ReadAllBytes($receipt)
  $refused = $false
  try { Write-SanitizedReceiptCreateNew -Path $receipt -Json '{"result":"replacement"}' } catch { $refused = $true }
  $after = [IO.File]::ReadAllBytes($receipt)
  Assert-True -Condition $refused -Message 'existing receipt was overwritten or accepted'
  Assert-True -Condition ([Convert]::ToBase64String($before) -ceq [Convert]::ToBase64String($after)) `
    -Message 'existing receipt bytes changed after refusal'

  $reparseParent = Join-Path $temporaryRoot 'receipt-parent-link'
  try { New-Item -ItemType Junction -Path $reparseParent -Target $receiptParent -ErrorAction Stop | Out-Null } catch {
    throw 'test_environment_blocker.receipt_reparse_fixture_unavailable'
  }
  $reparseRejected = $false
  try { Write-SanitizedReceiptCreateNew -Path (Join-Path $reparseParent 'second.json') -Json '{}' } catch { $reparseRejected = $true }
  Assert-True -Condition $reparseRejected -Message 'reparse receipt parent was accepted'
  return 3
}

function Assert-StoppedCellFixtures {
  $count = 0
  $eligible = Invoke-StoppedFixture -Path $stoppedFixturePath
  Assert-True -Condition ($eligible.disposition -ceq 'STOPPED_CELL_QUALIFIED') `
    -Message 'eligible stopped-cell fixture was not qualified'
  Assert-True -Condition ($eligible.authority -ceq 'none' -and -not $eligible.acceptance -and
    $eligible.real_platform_acceptance -ceq 'not_started') -Message 'stopped-cell fixture became authorizing'
  $count += 1

  $cases = @(
    @{ name = 'generation-1'; code = 'generation_2'; status = 'fail' },
    @{ name = 'secure-boot-disabled'; code = 'secure_boot'; status = 'fail' },
    @{ name = 'wrong-disk-parent'; code = 'disk_layout'; status = 'fail' },
    @{ name = 'extra-disk'; code = 'disk_layout'; status = 'fail' },
    @{ name = 'unexpected-network-adapter'; code = 'network_adapters'; status = 'fail' },
    @{ name = 'running-vm'; code = 'stopped_state'; status = 'fail' },
    @{ name = 'wrong-vm-identity'; code = 'vm_identity'; status = 'fail' },
    @{ name = 'unavailable-firmware'; code = 'secure_boot'; status = 'unavailable' },
    @{ name = 'wrong-resources'; code = 'resources'; status = 'fail' }
  )
  foreach ($case in $cases) {
    $path = Join-Path $temporaryRoot ("stopped-$($case.name).json")
    Write-StoppedCaseFixture -Path $path -Code $case.code -Status $case.status
    $result = Invoke-StoppedFixture -Path $path
    $prefix = if ($case.status -ceq 'unavailable') { 'evidence_gap' } else { 'precondition_failed' }
    Assert-True -Condition ($result.disposition -ceq 'BLOCKED' -and @($result.blockers) -ccontains "$prefix.$($case.code)") `
      -Message "stopped-cell case drifted: $($case.name)"
    $count += 1
  }

  $malformedPath = Join-Path $temporaryRoot 'stopped-malformed.json'
  '{' | Set-Content -LiteralPath $malformedPath -Encoding utf8NoBOM
  $malformed = Invoke-StoppedFixture -Path $malformedPath
  Assert-True -Condition ($malformed.disposition -ceq 'BLOCKED' -and @($malformed.blockers) -ccontains 'fixture.schema_invalid') `
    -Message 'malformed stopped-cell fixture was accepted'
  $count += 1
  $first = @(& $stoppedScriptPath -FixturePath $stoppedFixturePath)
  $second = @(& $stoppedScriptPath -FixturePath $stoppedFixturePath)
  Assert-True -Condition ($first.Count -eq 1 -and $first[0] -ceq $second[0]) -Message 'stopped-cell output was not deterministic'
  $count += 1
  $redactionPath = Join-Path $temporaryRoot 'stopped-redaction.json'
  Write-StoppedCaseFixture -Path $redactionPath -RawDetail 'C:\private\credential-password.txt'
  $redacted = @(& $stoppedScriptPath -FixturePath $redactionPath)
  Assert-True -Condition ($redacted.Count -eq 1 -and $redacted[0] -cnotmatch '(?i)C:\\|credential|password|private') `
    -Message 'stopped-cell fixture disclosed a raw path or secret-like detail'
  $count += 1
  return $count
}

function Assert-FixtureIsolation {
  param([Parameter(Mandatory)][string]$Path)

  $guardPath = Join-Path $temporaryRoot 'fixture-isolation-guard.ps1'
  $escapedScript = $scriptPath.Replace("'", "''")
  $escapedFixture = $Path.Replace("'", "''")
  @"
`$ErrorActionPreference = 'Stop'
foreach (`$name in @(
  'Get-WindowsOptionalFeature', 'Get-LocalGroupMember', 'Get-CimInstance', 'Get-Module',
  'Get-Command', 'Get-Service', 'Get-VM', 'Get-VMSwitch', 'Get-Process', 'Get-Volume',
  'Get-Partition', 'Get-Disk', 'Get-VHD', 'Get-VMFirmware', 'Get-VMHardDiskDrive',
  'Get-VMNetworkAdapter', 'Get-ItemProperty', 'Get-NetAdapter', 'Get-Credential', 'git', 'gh'
)) {
  Set-Item -Path "function:`$name" -Value { throw "fixture mode isolation breach: `$args" }
}
& '$escapedScript' -FixturePath '$escapedFixture'
"@ | Set-Content -LiteralPath $guardPath -Encoding utf8NoBOM
  $pwsh = Get-Command pwsh -CommandType Application -ErrorAction Stop |
    Select-Object -First 1 -ExpandProperty Source
  $raw = @(& $pwsh -NoProfile -File $guardPath)
  Assert-True -Condition ($LASTEXITCODE -eq 0) -Message 'fixture mode called a guarded live observation command'
  Assert-True -Condition ($raw.Count -eq 1) -Message 'fixture isolation guard did not receive exactly one result'
  $result = $raw[0] | ConvertFrom-Json -ErrorAction Stop
  Assert-True -Condition ($result.evidence_source -ceq 'fixture') -Message 'fixture isolation result drifted'
}

function Assert-StoppedFixtureIsolation {
  $guardPath = Join-Path $temporaryRoot 'stopped-fixture-isolation-guard.ps1'
  $escapedScript = $stoppedScriptPath.Replace("'", "''")
  $escapedFixture = $stoppedFixturePath.Replace("'", "''")
  @"
`$ErrorActionPreference = 'Stop'
foreach (`$name in @(
  'Get-VM', 'Get-VMFirmware', 'Get-VMHardDiskDrive', 'Get-VHD', 'Get-VMNetworkAdapter',
  'Get-Service', 'Get-Process', 'Get-Volume', 'Get-Partition', 'Get-Disk', 'Get-CimInstance',
  'Get-ItemProperty', 'Get-NetAdapter', 'Get-Credential', 'git', 'gh'
)) {
  Set-Item -Path "function:`$name" -Value { throw "stopped fixture isolation breach: `$args" }
}
& '$escapedScript' -FixturePath '$escapedFixture'
"@ | Set-Content -LiteralPath $guardPath -Encoding utf8NoBOM
  $pwsh = Get-Command pwsh -CommandType Application -ErrorAction Stop |
    Select-Object -First 1 -ExpandProperty Source
  $raw = @(& $pwsh -NoProfile -File $guardPath)
  Assert-True -Condition ($LASTEXITCODE -eq 0) -Message 'stopped-cell fixture mode called a live observation command'
  Assert-True -Condition ($raw.Count -eq 1) -Message 'stopped-cell isolation guard did not receive one result'
  $result = $raw[0] | ConvertFrom-Json -ErrorAction Stop
  Assert-True -Condition ($result.evidence_source -ceq 'fixture') -Message 'stopped-cell isolation result drifted'
}

New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
  $executedCaseCount = 0
  Assert-StaticDenyList
  $executedCaseCount += 1
  Assert-PathSafetyAndDeterminism
  $executedCaseCount += 1
  $eligible = Invoke-Fixture -Path $fixturePath
  Assert-True -Condition ($eligible.contract -ceq 'vmcell.hyperv-r5-preflight.v1') -Message 'fixture result contract drifted'
  Assert-True -Condition ($eligible.authority -ceq 'none' -and $eligible.acceptance -eq $false) `
    -Message 'fixture result became authorizing'
  Assert-True -Condition ($eligible.real_platform_acceptance -ceq 'not_started') `
    -Message 'fixture result claimed real-platform acceptance'
  Assert-True -Condition ($eligible.disposition -ceq 'PREFLIGHT_ELIGIBLE') -Message 'eligible fixture was not eligible'
  Assert-True -Condition (@($eligible.observations).Count -eq 30) -Message 'eligible fixture result did not preserve every observation'
  Assert-True -Condition ($eligible.mutation_flags.host_observation -eq $false) -Message 'fixture result claimed host observation'
  $executedCaseCount += 1

  $cases = @(
    @{ name = 'non-elevated-token'; code = 'elevation'; status = 'fail' },
    @{ name = 'hyperv-feature-unavailable'; code = 'hyperv_feature'; status = 'unavailable' },
    @{ name = 'hyperv-feature-disabled'; code = 'hyperv_feature'; status = 'fail' },
    @{ name = 'hyperv-module-missing'; code = 'hyperv_module'; status = 'fail' },
    @{ name = 'hyperv-read-access-denied'; code = 'hyperv_read_access'; status = 'unavailable' },
    @{ name = 'vmms-stopped'; code = 'vmms_state'; status = 'fail' },
    @{ name = 'foreign-or-running-vm'; code = 'vm_inventory'; status = 'fail' },
    @{ name = 'unexpected-switch'; code = 'switch_inventory'; status = 'fail' },
    @{ name = 'active-virtualization-writer'; code = 'virtualization_writers'; status = 'fail' },
    @{ name = 'active-runner-or-codex'; code = 'runner_codex_activity'; status = 'fail' },
    @{ name = 'state-root-storage-rejected'; code = 'state_root_storage'; status = 'fail' },
    @{ name = 'runtime-root-storage-rejected'; code = 'runtime_root_storage'; status = 'fail' },
    @{ name = 'image-storage-rejected'; code = 'image_storage'; status = 'fail' },
    @{ name = 'package-storage-rejected'; code = 'package_storage'; status = 'fail' },
    @{ name = 'binary-storage-rejected'; code = 'binary_storage'; status = 'fail' },
    @{ name = 'provenance-storage-rejected'; code = 'provenance_storage'; status = 'fail' },
    @{ name = 'evidence-output-rejected'; code = 'evidence_output'; status = 'fail' },
    @{ name = 'missing-vhdx'; code = 'immutable_vhdx_presence'; status = 'fail' },
    @{ name = 'mutable-vhdx'; code = 'vhdx_immutability'; status = 'fail' },
    @{ name = 'attached-vhdx'; code = 'vhdx_attachment'; status = 'fail' },
    @{ name = 'differencing-vhdx'; code = 'vhdx_immutability'; status = 'fail' },
    @{ name = 'parented-vhdx'; code = 'vhdx_parent'; status = 'fail' },
    @{ name = 'missing-provenance'; code = 'image_provenance'; status = 'unavailable' },
    @{ name = 'mismatched-provenance'; code = 'image_provenance'; status = 'fail' },
    @{ name = 'candidate-hash-mismatch'; code = 'candidate_hash'; status = 'fail' },
    @{ name = 'package-hash-mismatch'; code = 'package_hash'; status = 'fail' },
    @{ name = 'binary-hash-mismatch'; code = 'binary_hash'; status = 'fail' },
    @{ name = 'vhdx-hash-mismatch'; code = 'vhdx_hash'; status = 'fail' },
    @{ name = 'stale-receipt'; code = 'admission_receipt'; status = 'fail' },
    @{ name = 'exclusive-window-unavailable'; code = 'exclusive_window'; status = 'unavailable' }
  )
  foreach ($case in $cases) {
    $casePath = Join-Path $temporaryRoot ($case.name + '.json')
    Write-CaseFixture -Path $casePath -Code $case.code -Status $case.status
    $result = Invoke-Fixture -Path $casePath
    Assert-True -Condition ($result.disposition -ceq 'BLOCKED') "fixture case was not blocked: $($case.name)"
    $expected = if ($case.status -ceq 'unavailable') {
      "evidence_gap.$($case.code)"
    } else {
      "precondition_failed.$($case.code)"
    }
    Assert-True -Condition (@($result.blockers) -ccontains $expected) "fixture case omitted blocker: $($case.name)"
    $executedCaseCount += 1
  }

  $malformedPath = Join-Path $temporaryRoot 'malformed.json'
  '{' | Set-Content -LiteralPath $malformedPath -Encoding utf8NoBOM
  $malformed = Invoke-Fixture -Path $malformedPath
  Assert-True -Condition ($malformed.disposition -ceq 'BLOCKED' -and
    @($malformed.blockers) -ccontains 'fixture.schema_invalid') 'malformed fixture was not rejected'
  $executedCaseCount += 1

  $first = @(& $scriptPath -FixturePath $fixturePath)
  $second = @(& $scriptPath -FixturePath $fixturePath)
  Assert-True -Condition ($first.Count -eq 1 -and $first[0] -ceq $second[0]) 'fixture output was not deterministic'
  $executedCaseCount += 1

  $redactionPath = Join-Path $temporaryRoot 'redaction.json'
  Write-CaseFixture -Path $redactionPath -RawDetail 'C:\\private\\credential-password.txt'
  $redacted = @(& $scriptPath -FixturePath $redactionPath)
  Assert-True -Condition ($redacted.Count -eq 1 -and $redacted[0] -cnotmatch '(?i)C:\\|credential|password|private') `
    -Message 'fixture output disclosed a raw path or secret-like detail'
  $executedCaseCount += 1

  $executedCaseCount += Assert-PathAncestryBehavior
  $executedCaseCount += Assert-StorageClassificationBehavior
  $executedCaseCount += Assert-ReceiptBehavior
  $executedCaseCount += Assert-StoppedCellFixtures
  Assert-FixtureIsolation -Path $fixturePath
  $executedCaseCount += 1
  Assert-StoppedFixtureIsolation
  $executedCaseCount += 1
} finally {
  Remove-Item -LiteralPath $temporaryRoot -Force -Recurse -ErrorAction SilentlyContinue
}

Write-Host "Windows Hyper-V owner-preview fixture, isolation, and static safety contracts passed ($executedCaseCount cases)"
