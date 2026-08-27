$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$scriptPath = Join-Path $PSScriptRoot 'windows-hyperv-preflight.ps1'
$fixturePath = Join-Path $repositoryRoot 'tests\fixtures\hyperv-preflight\eligible.json'
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

function Assert-StaticDenyList {
  $tokens = $null
  $parseErrors = $null
  $ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $scriptPath,
    [ref]$tokens,
    [ref]$parseErrors
  )
  Assert-True -Condition ($parseErrors.Count -eq 0) -Message 'R5 preflight has a PowerShell parser error'
  $commands = @($ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst]
  }, $true) | ForEach-Object { $_.GetCommandName() } | Where-Object { $_ })
  $forbidden = @(
    'Enable-WindowsOptionalFeature', 'Disable-WindowsOptionalFeature',
    'Install-WindowsFeature', 'Uninstall-WindowsFeature',
    'Add-WindowsCapability', 'Remove-WindowsCapability',
    'Add-LocalGroupMember', 'Remove-LocalGroupMember',
    'Start-Service', 'Stop-Service', 'Restart-Service', 'Set-Service', 'New-Service', 'Remove-Service',
    'New-VM', 'Set-VM', 'Remove-VM', 'Start-VM', 'Stop-VM', 'Import-VM', 'Export-VM',
    'Checkpoint-VM', 'Restore-VM', 'Suspend-VM', 'Resume-VM',
    'Add-VMHardDiskDrive', 'Remove-VMHardDiskDrive', 'Add-VMNetworkAdapter', 'Remove-VMNetworkAdapter',
    'Set-VMProcessor', 'Set-VMMemory',
    'New-VMSwitch', 'Set-VMSwitch', 'Remove-VMSwitch', 'Add-VMSwitchExtension', 'Remove-VMSwitchExtension',
    'New-VHD', 'Set-VHD', 'Remove-VHD', 'Resize-VHD', 'Mount-VHD', 'Dismount-VHD', 'Convert-VHD', 'Merge-VHD',
    'Initialize-Disk', 'Clear-Disk', 'Set-Disk', 'Format-Volume', 'Set-Volume', 'Dismount-Volume',
    'New-Partition', 'Set-Partition', 'Remove-Partition',
    'Set-Acl', 'Clear-Acl', 'icacls.exe', 'takeown.exe',
    'New-NetIPAddress', 'Set-NetIPAddress', 'Remove-NetIPAddress', 'New-NetRoute', 'Set-NetRoute', 'Remove-NetRoute',
    'Set-NetIPInterface', 'Restart-NetAdapter', 'Enable-NetAdapter', 'Disable-NetAdapter',
    'New-NetNat', 'Remove-NetNat', 'Set-NetFirewallProfile',
    'Stop-Process', 'taskkill.exe', 'Stop-Computer', 'Restart-Computer', 'shutdown.exe', 'sc.exe'
  )
  foreach ($command in $forbidden) {
    Assert-True -Condition ($commands -cnotcontains $command) `
      -Message "R5 preflight contains forbidden mutating command $command"
  }
}

function Assert-PathSafetyAndDeterminism {
  $source = [IO.File]::ReadAllText($scriptPath)
  foreach ($required in @(
      'function Get-SafeProvenanceSnapshot',
      'function Get-OrdinaryPathItem',
      'function Get-OrdinaryProvenanceFile',
      '[IO.FileAttributes]::ReparsePoint',
      '$beforeHash = Get-Sha256File',
      '$afterHash = Get-Sha256File',
      '$verifiedItem = Get-OrdinaryProvenanceFile',
      'provenance evidence changed while it was read',
      "Get-OrdinaryPathItem -Path `$StateRoot -RequireDirectory `$true -Description 'state root'",
      "Get-OrdinaryPathItem -Path `$VhdxPath -RequireDirectory `$false -Description 'VHDX path'",
      "Get-OrdinaryPathItem -Path `$CandidatePackagePath -RequireDirectory `$false -Description 'candidate package path'",
      "Get-OrdinaryPathItem -Path `$CandidateBinaryPath -RequireDirectory `$false -Description 'candidate binary path'",
      'Get-ObservationDigest -Observations $liveObservations',
      '$requiredStrings[17]'
    )) {
    Assert-True -Condition $source.Contains($required) `
      -Message "R5 preflight omitted required provenance safety binding: $required"
  }
  Assert-True -Condition ($source -notmatch [regex]::Escape(
      'EvidenceSourceDigest (Get-Sha256Text -Text ([DateTimeOffset]::UtcNow.ToString(''O'')))'
    )) -Message 'live result digest must not be derived from wall-clock time'
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
  'Get-Partition', 'Get-Disk', 'Get-VHD', 'git'
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

New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
  Assert-StaticDenyList
  Assert-PathSafetyAndDeterminism
  $eligible = Invoke-Fixture -Path $fixturePath
  Assert-True -Condition ($eligible.contract -ceq 'vmcell.hyperv-r5-preflight.v1') -Message 'fixture result contract drifted'
  Assert-True -Condition ($eligible.authority -ceq 'none' -and $eligible.acceptance -eq $false) `
    -Message 'fixture result became authorizing'
  Assert-True -Condition ($eligible.real_platform_acceptance -ceq 'not_started') `
    -Message 'fixture result claimed real-platform acceptance'
  Assert-True -Condition ($eligible.disposition -ceq 'PREFLIGHT_ELIGIBLE') -Message 'eligible fixture was not eligible'
  Assert-True -Condition (@($eligible.observations).Count -eq 25) -Message 'eligible fixture result did not preserve every observation'
  Assert-True -Condition ($eligible.mutation_flags.host_observation -eq $false) -Message 'fixture result claimed host observation'

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
    @{ name = 'unsuitable-c-boundary'; code = 'c_storage_boundary'; status = 'fail' },
    @{ name = 'refs-v-rejected'; code = 'v_storage_boundary'; status = 'fail' },
    @{ name = 'file-backed-virtual-v-rejected'; code = 'v_storage_boundary'; status = 'fail' },
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
  }

  $malformedPath = Join-Path $temporaryRoot 'malformed.json'
  '{' | Set-Content -LiteralPath $malformedPath -Encoding utf8NoBOM
  $malformed = Invoke-Fixture -Path $malformedPath
  Assert-True -Condition ($malformed.disposition -ceq 'BLOCKED' -and
    @($malformed.blockers) -ccontains 'fixture.schema_invalid') 'malformed fixture was not rejected'

  $first = @(& $scriptPath -FixturePath $fixturePath)
  $second = @(& $scriptPath -FixturePath $fixturePath)
  Assert-True -Condition ($first.Count -eq 1 -and $first[0] -ceq $second[0]) 'fixture output was not deterministic'

  $redactionPath = Join-Path $temporaryRoot 'redaction.json'
  Write-CaseFixture -Path $redactionPath -RawDetail 'C:\\private\\credential-password.txt'
  $redacted = @(& $scriptPath -FixturePath $redactionPath)
  Assert-True -Condition ($redacted.Count -eq 1 -and $redacted[0] -cnotmatch '(?i)C:\\|credential|password|private') `
    -Message 'fixture output disclosed a raw path or secret-like detail'

  Assert-FixtureIsolation -Path $fixturePath
} finally {
  Remove-Item -LiteralPath $temporaryRoot -Force -Recurse -ErrorAction SilentlyContinue
}

Write-Host 'Windows Hyper-V R5 fixture, isolation, and static safety contracts passed (32 cases)'
