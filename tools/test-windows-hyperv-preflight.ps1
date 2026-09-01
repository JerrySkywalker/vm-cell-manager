$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$scriptPath = Join-Path $PSScriptRoot 'windows-hyperv-preflight.ps1'
$fixturePath = Join-Path $repositoryRoot 'tests\fixtures\hyperv-preflight\eligible.json'
$templatePath = Join-Path $repositoryRoot 'docs\receipts\windows-hyperv-image-provenance-template.json'
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('vmcell-hyperv-r1-provenance-' + [Guid]::NewGuid().ToString('N'))

function Assert-True {
  param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
  if (-not $Condition) { throw $Message }
}

function ConvertFrom-ContractJson {
  param([Parameter(Mandatory)][string]$Text)

  $parameters = @{ ErrorAction = 'Stop' }
  if ($PSVersionTable.PSVersion -ge [Version]'7.5') { $parameters.DateKind = 'String' }
  return $Text | ConvertFrom-Json @parameters
}

function Read-ContractJson {
  param([Parameter(Mandatory)][string]$Path)

  return ConvertFrom-ContractJson -Text (Get-Content -LiteralPath $Path -Raw)
}

function Copy-ContractObject {
  param([Parameter(Mandatory)][object]$Value)

  return ConvertFrom-ContractJson -Text ($Value | ConvertTo-Json -Depth 32)
}

function Invoke-Fixture {
  param([Parameter(Mandatory)][string]$Path)

  $raw = @(& $scriptPath -FixturePath $Path)
  Assert-True -Condition ($raw.Count -eq 1) -Message 'fixture invocation did not emit exactly one JSON document'
  return ConvertFrom-ContractJson -Text $raw[0]
}

function Write-Fixture {
  param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][object]$Fixture)

  $Fixture | ConvertTo-Json -Depth 32 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

function Set-JsonPathValue {
  param(
    [Parameter(Mandatory)][object]$Object,
    [Parameter(Mandatory)][string]$Path,
    [AllowNull()][object]$Value
  )

  $segments = $Path.Split('.')
  if ($segments.Count -eq 1) {
    $Object.PSObject.Properties[$segments[0]].Value = $Value
    return
  }
  $current = $Object
  foreach ($segment in $segments[0..($segments.Count - 2)]) {
    $current = $current.PSObject.Properties[$segment].Value
  }
  $current.PSObject.Properties[$segments[-1]].Value = $Value
}

function Remove-JsonPathValue {
  param([Parameter(Mandatory)][object]$Object, [Parameter(Mandatory)][string]$Path)

  $segments = $Path.Split('.')
  if ($segments.Count -eq 1) {
    [void]$Object.PSObject.Properties.Remove($segments[0])
    return
  }
  $current = $Object
  foreach ($segment in $segments[0..($segments.Count - 2)]) {
    $current = $current.PSObject.Properties[$segment].Value
  }
  [void]$current.PSObject.Properties.Remove($segments[-1])
}

function Get-WrongJsonValues {
  param([Parameter(Mandatory)][string]$JsonType)

  $values = [System.Collections.Generic.List[object]]::new()
  switch ($JsonType) {
    'string' {
      $values.Add($true)
      $values.Add([Int64]7)
      $values.Add([pscustomobject]@{ invalid = 'object' })
      $values.Add([object[]]@('array'))
    }
    'integer' {
      $values.Add('7')
      $values.Add($true)
      $values.Add([pscustomobject]@{ invalid = 'object' })
      $values.Add([object[]]@('array'))
    }
    'boolean' {
      $values.Add('false')
      $values.Add([Int64]0)
      $values.Add([pscustomobject]@{ invalid = 'object' })
      $values.Add([object[]]@('array'))
    }
  }
  return $values
}

function Assert-StaticProductionSafety {
  $tokens = $null
  $parseErrors = $null
  $ast = [System.Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
  Assert-True -Condition ($parseErrors.Count -eq 0) -Message 'preflight has a PowerShell parser error'
  $commands = @($ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.CommandAst] }, $true) |
    ForEach-Object { $_.GetCommandName() } | Where-Object { $_ })
  $forbidden = @(
    'Enable-WindowsOptionalFeature', 'Disable-WindowsOptionalFeature', 'Install-WindowsFeature',
    'Uninstall-WindowsFeature', 'Add-WindowsCapability', 'Remove-WindowsCapability',
    'Start-Service', 'Stop-Service', 'Restart-Service', 'Set-Service', 'New-Service', 'Remove-Service',
    'New-VM', 'Set-VM', 'Remove-VM', 'Start-VM', 'Stop-VM', 'Import-VM', 'Export-VM',
    'Checkpoint-VM', 'Restore-VM', 'Suspend-VM', 'Resume-VM', 'Add-VMHardDiskDrive',
    'Remove-VMHardDiskDrive', 'Add-VMNetworkAdapter', 'Remove-VMNetworkAdapter', 'Set-VMProcessor',
    'Set-VMMemory', 'New-VMSwitch', 'Set-VMSwitch', 'Remove-VMSwitch', 'New-VHD', 'Set-VHD',
    'Remove-VHD', 'Resize-VHD', 'Mount-VHD', 'Dismount-VHD', 'Convert-VHD', 'Merge-VHD',
    'Initialize-Disk', 'Clear-Disk', 'Set-Disk', 'Format-Volume', 'Set-Volume', 'Dismount-Volume',
    'New-Partition', 'Set-Partition', 'Remove-Partition', 'Set-Acl', 'Clear-Acl', 'icacls.exe',
    'takeown.exe', 'New-NetIPAddress', 'Set-NetIPAddress', 'Remove-NetIPAddress', 'New-NetRoute',
    'Set-NetRoute', 'Remove-NetRoute', 'Set-NetIPInterface', 'Restart-NetAdapter', 'Enable-NetAdapter',
    'Disable-NetAdapter', 'New-NetNat', 'Remove-NetNat', 'Set-NetFirewallProfile', 'Stop-Process',
    'taskkill.exe', 'Stop-Computer', 'Restart-Computer', 'shutdown.exe', 'sc.exe'
  )
  foreach ($command in $forbidden) {
    Assert-True -Condition ($commands -cnotcontains $command) "preflight contains forbidden mutating command $command"
  }
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
  $pwsh = Get-Command pwsh -CommandType Application -ErrorAction Stop | Select-Object -First 1 -ExpandProperty Source
  $raw = @(& $pwsh -NoProfile -File $guardPath)
  Assert-True -Condition ($LASTEXITCODE -eq 0) -Message 'fixture mode called a guarded live observation command'
  Assert-True -Condition ($raw.Count -eq 1) -Message 'fixture isolation guard did not receive exactly one result'
  $result = ConvertFrom-ContractJson -Text $raw[0]
  Assert-True -Condition ($result.evidence_source -ceq 'fixture' -and $result.mutation_flags.host_observation -eq $false) `
    -Message 'fixture isolation result drifted'
}

New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
  $executedCases = 0
  $negativeCases = 0
  $placeholderCases = 0
  Assert-StaticProductionSafety
  $executedCases += 1

  . $scriptPath -FixturePath $fixturePath | Out-Null
  $fixture = Read-ContractJson -Path $fixturePath
  $eligible = Invoke-Fixture -Path $fixturePath
  Assert-True -Condition ($eligible.contract -ceq 'vmcell.hyperv-r5-preflight.v2') 'fixture result contract drifted'
  Assert-True -Condition ($eligible.authority -ceq 'none' -and $eligible.acceptance -eq $false -and $eligible.authorizing -eq $false) `
    'fixture result became authorizing'
  Assert-True -Condition ($eligible.real_platform_acceptance -ceq 'not_started' -and $eligible.support_status -ceq 'untested') `
    'fixture result promoted platform acceptance or support'
  Assert-True -Condition ($eligible.disposition -ceq 'PREFLIGHT_ELIGIBLE') 'complete eligible fixture was not eligible'
  Assert-True -Condition ($eligible.provenance_validation.valid -eq $true) 'eligible provenance was not valid'
  Assert-True -Condition (@($eligible.observations).Count -eq 25) 'fixture result did not preserve every observation'
  $executedCases += 1

  $schema = @(Get-HyperVImageProvenanceSchema)
  $directEligible = Test-HyperVImageProvenance -Provenance $fixture.provenance -ExpectedIdentity $fixture.expected_identity
  Assert-True -Condition ($directEligible.valid -and $directEligible.required_field_count -eq $schema.Count) 'validator and eligible fixture disagree'
  foreach ($field in $schema) {
    $missing = Copy-ContractObject -Value $fixture.provenance
    Remove-JsonPathValue -Object $missing -Path $field.path
    $validation = Test-HyperVImageProvenance -Provenance $missing -ExpectedIdentity $fixture.expected_identity
    Assert-True -Condition (-not $validation.valid) "missing required field was admitted: $($field.path)"
    $negativeCases += 1

    foreach ($wrongValue in Get-WrongJsonValues -JsonType $field.json_type) {
      $wrongType = Copy-ContractObject -Value $fixture.provenance
      Set-JsonPathValue -Object $wrongType -Path $field.path -Value $wrongValue
      $validation = Test-HyperVImageProvenance -Provenance $wrongType -ExpectedIdentity $fixture.expected_identity
      Assert-True -Condition (-not $validation.valid) "wrong JSON type was admitted: $($field.path)"
      $negativeCases += 1
    }
    $nullValue = Copy-ContractObject -Value $fixture.provenance
    Set-JsonPathValue -Object $nullValue -Path $field.path -Value $null
    $validation = Test-HyperVImageProvenance -Provenance $nullValue -ExpectedIdentity $fixture.expected_identity
    Assert-True -Condition (-not $validation.valid) "null JSON value was admitted: $($field.path)"
    $negativeCases += 1
    if ($field.json_type -eq 'string') {
      foreach ($value in @('', '   ')) {
        $empty = Copy-ContractObject -Value $fixture.provenance
        Set-JsonPathValue -Object $empty -Path $field.path -Value $value
        $validation = Test-HyperVImageProvenance -Provenance $empty -ExpectedIdentity $fixture.expected_identity
        Assert-True -Condition (-not $validation.valid) "empty string was admitted: $($field.path)"
        $negativeCases += 1
      }
      foreach ($value in @('REQUIRED_VALUE', ' required_value ', 'TODO', ' tBd ', 'FixMe', ' UNKNOWN ')) {
        $placeholder = Copy-ContractObject -Value $fixture.provenance
        Set-JsonPathValue -Object $placeholder -Path $field.path -Value $value
        $validation = Test-HyperVImageProvenance -Provenance $placeholder -ExpectedIdentity $fixture.expected_identity
        Assert-True -Condition (-not $validation.valid) "placeholder was admitted: $($field.path)"
        $negativeCases += 1
        $placeholderCases += 1
      }
    }
  }

  foreach ($expectedProperty in @('candidate_sha', 'package_archive_sha256', 'package_sha256', 'candidate_binary_sha256', 'vhdx_sha256')) {
    $mismatchIdentity = Copy-ContractObject -Value $fixture.expected_identity
    $mismatchIdentity.PSObject.Properties[$expectedProperty].Value = ('f' * ([string]$mismatchIdentity.PSObject.Properties[$expectedProperty].Value).Length)
    $validation = Test-HyperVImageProvenance -Provenance $fixture.provenance -ExpectedIdentity $mismatchIdentity
    Assert-True -Condition (-not $validation.valid) "identity mismatch was admitted: $expectedProperty"
    $negativeCases += 1
  }
  foreach ($change in @(
      @{ path = 'windows.edition'; value = 'Datacenter' },
      @{ path = 'windows.build'; value = '22621.1' },
      @{ path = 'hyperv.generation'; value = [Int64]1 },
      @{ path = 'hyperv.secure_boot_enabled'; value = $false },
      @{ path = 'vhdx.parentless'; value = $false },
      @{ path = 'vhdx.attached'; value = $true },
      @{ path = 'vhdx.immutable_owner_policy'; value = 'UNMANAGED' },
      @{ path = 'vhdx.preparation_timestamp_utc'; value = 'not-a-timestamp' }
    )) {
    $invalid = Copy-ContractObject -Value $fixture.provenance
    Set-JsonPathValue -Object $invalid -Path $change.path -Value $change.value
    $validation = Test-HyperVImageProvenance -Provenance $invalid -ExpectedIdentity $fixture.expected_identity
    Assert-True -Condition (-not $validation.valid) "required safety value was admitted: $($change.path)"
    $negativeCases += 1
  }

  $ownerAttestation = Copy-ContractObject -Value $fixture.provenance
  Set-JsonPathValue -Object $ownerAttestation -Path 'vhdx.credentials_embedded' -Value 'UNKNOWN_REQUIRES_OWNER_ATTESTATION'
  $ownerAttestationValidation = Test-HyperVImageProvenance -Provenance $ownerAttestation -ExpectedIdentity $fixture.expected_identity
  Assert-True -Condition ($ownerAttestationValidation.valid -and $ownerAttestationValidation.requires_owner_attestation) `
    'permitted owner-attestation enum was rejected'
  $ownerFixture = Copy-ContractObject -Value $fixture
  $ownerFixture.provenance = $ownerAttestation
  $ownerPath = Join-Path $temporaryRoot 'owner-attestation.json'
  Write-Fixture -Path $ownerPath -Fixture $ownerFixture
  $ownerResult = Invoke-Fixture -Path $ownerPath
  Assert-True -Condition ($ownerResult.disposition -ceq 'BLOCKED' -and
    @($ownerResult.blockers) -ccontains 'owner_attestation.credentials_embedded_required') `
    'owner-attestation requirement did not block preflight without rejecting the enum'
  $executedCases += 1

  $template = Read-ContractJson -Path $templatePath
  $templateValidation = Test-HyperVImageProvenance -Provenance $template -ExpectedIdentity $fixture.expected_identity
  Assert-True -Condition (-not $templateValidation.valid -and
    @($templateValidation.blockers | Where-Object { $_ -like 'provenance.placeholder_value.*' }).Count -gt 0) `
    'template placeholders were accepted by the admitted-evidence validator'
  $executedCases += 1

  $first = @(& $scriptPath -FixturePath $fixturePath)
  $second = @(& $scriptPath -FixturePath $fixturePath)
  Assert-True -Condition ($first.Count -eq 1 -and $first[0] -ceq $second[0]) 'fixture output was not deterministic'
  $redactionFixture = Copy-ContractObject -Value $fixture
  $redactionFixture | Add-Member -NotePropertyName raw_detail -NotePropertyValue 'C:\private\credential-password.txt'
  $redactionPath = Join-Path $temporaryRoot 'redaction.json'
  Write-Fixture -Path $redactionPath -Fixture $redactionFixture
  $redacted = @(& $scriptPath -FixturePath $redactionPath)
  Assert-True -Condition ($redacted.Count -eq 1 -and $redacted[0] -cnotmatch '(?i)C:\\|credential|password|private') `
    'fixture output disclosed raw detail'
  $executedCases += 2

  Assert-FixtureIsolation -Path $fixturePath
  $executedCases += 1
} finally {
  Remove-Item -LiteralPath $temporaryRoot -Force -Recurse -ErrorAction SilentlyContinue
}

Write-Host "Windows Hyper-V R1 provenance contract tests passed (fields=$($schema.Count); negative_cases=$negativeCases; placeholder_cases=$placeholderCases; fixture_cases=$executedCases)"
