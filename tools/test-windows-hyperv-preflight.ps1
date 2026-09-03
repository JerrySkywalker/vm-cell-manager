$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$scriptPath = Join-Path $PSScriptRoot 'windows-hyperv-preflight.ps1'
$fixturePath = Join-Path $repositoryRoot 'tests\fixtures\hyperv-preflight\eligible.json'
$templatePath = Join-Path $repositoryRoot 'docs\receipts\windows-hyperv-image-provenance-template.json'
$matrixPath = Join-Path $repositoryRoot 'tests\fixtures\hyperv-preflight\provenance-contract-matrix.json'
$documentationPath = Join-Path $repositoryRoot 'docs\windows-hyperv-r5-preflight.md'
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

function Invoke-StructuredFixture {
  param([Parameter(Mandatory)][string]$Path)

  $raw = @(& $scriptPath -FixturePath $Path)
  Assert-True -Condition ($raw.Count -eq 1) -Message 'fixture invocation did not emit exactly one JSON document'
  Assert-True -Condition ($raw[0] -notmatch '(?i)(PropertyNotFound|RuntimeException|stack trace|\bat\s+.+\.ps1:)') `
    -Message 'fixture output exposed a raw PowerShell exception or stack trace'
  return [pscustomobject]@{ raw = $raw[0]; result = (ConvertFrom-ContractJson -Text $raw[0]) }
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

function Get-JsonPathEntry {
  param([Parameter(Mandatory)][object]$Object, [Parameter(Mandatory)][string]$Path)

  $current = $Object
  foreach ($segment in $Path.Split('.')) {
    if ($null -eq $current -or $current -isnot [pscustomobject]) {
      return [pscustomobject]@{ exists = $false; value = $null }
    }
    $property = $current.PSObject.Properties[$segment]
    if ($null -eq $property) {
      return [pscustomobject]@{ exists = $false; value = $null }
    }
    $current = $property.Value
  }
  return [pscustomobject]@{ exists = $true; value = $current }
}

function Get-IndependentGoldenMatrix {
  $matrix = Read-ContractJson -Path $matrixPath
  Assert-True -Condition ($matrix.schema_version -eq 1 -and
    $matrix.contract -ceq 'vmcell.hyperv-r1-provenance-golden.v1') 'golden matrix identity drifted'
  $fields = @($matrix.fields)
  Assert-True -Condition ($fields.Count -eq 63) 'golden matrix field count drifted'
  $paths = @($fields | ForEach-Object { [string]$_.path })
  Assert-True -Condition (@($paths | Sort-Object -Unique).Count -eq $fields.Count) 'golden matrix has duplicate paths'
  foreach ($field in $fields) {
    Assert-True -Condition ($field.path -match '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$') 'golden matrix path is invalid'
    Assert-True -Condition ($field.json_type -cin @('string', 'integer', 'boolean')) 'golden matrix type is invalid'
    Assert-True -Condition (-not [string]::IsNullOrWhiteSpace([string]$field.semantic)) 'golden matrix semantic is missing'
    Assert-True -Condition ($field.placeholder_policy -cin @('none', 'token_boundary')) 'golden matrix placeholder policy is invalid'
  }
  return $matrix
}

function Assert-TemplateMatchesGoldenMatrix {
  param([Parameter(Mandatory)][object]$Template, [Parameter(Mandatory)][object[]]$Fields)

  foreach ($field in $Fields) {
    $entry = Get-JsonPathEntry -Object $Template -Path $field.path
    Assert-True -Condition $entry.exists "template omitted golden field: $($field.path)"
    $typeMatches = switch ($field.json_type) {
      'string' { $entry.value -is [string] }
      'integer' { $entry.value -is [int64] -or $entry.value -is [int32] }
      'boolean' { $entry.value -is [bool] }
    }
    Assert-True -Condition $typeMatches "template type drifted: $($field.path)"
  }
}

function Assert-ProductionSchemaMatchesGoldenMatrix {
  param([Parameter(Mandatory)][object[]]$Fields)

  $productionFields = @(Get-HyperVImageProvenanceSchema)
  Assert-True -Condition ($productionFields.Count -eq $Fields.Count) 'production schema field count differs from golden matrix'
  foreach ($field in $Fields) {
    $production = @($productionFields | Where-Object { $_.path -ceq $field.path })
    Assert-True -Condition ($production.Count -eq 1) "production schema omitted golden field: $($field.path)"
    Assert-True -Condition ($production[0].json_type -ceq $field.json_type) "production schema type drifted: $($field.path)"
    Assert-True -Condition ($production[0].placeholder_policy -ceq $field.placeholder_policy) "production placeholder policy drifted: $($field.path)"
  }
}

function Assert-RustTemplateTestMatchesGoldenMatrix {
  param([Parameter(Mandatory)][object[]]$Fields)

  $rust = Get-Content -LiteralPath (Join-Path $repositoryRoot 'tests\acceptance_receipt_templates.rs') -Raw
  foreach ($field in $Fields) {
    $pointer = '/' + $field.path.Replace('.', '/')
    Assert-True -Condition $rust.Contains($pointer, [StringComparison]::Ordinal) "Rust template test omitted golden field: $($field.path)"
  }
}

function Assert-DocumentationMatchesGoldenMatrix {
  param([Parameter(Mandatory)][object[]]$Fields)

  $documentation = Get-Content -LiteralPath $documentationPath -Raw
  Assert-True -Condition $documentation.Contains('Raw JSON provenance boundary', [StringComparison]::Ordinal) 'documentation omitted raw JSON boundary'
  Assert-True -Condition $documentation.Contains('image_source.reference', [StringComparison]::Ordinal) 'documentation omitted source reference binding'
  Assert-True -Condition $documentation.Contains('creation.created_by_evidence_id', [StringComparison]::Ordinal) 'documentation omitted creator evidence binding'
  foreach ($root in @($Fields | ForEach-Object { $_.path.Split('.')[0] } | Sort-Object -Unique)) {
    Assert-True -Condition $documentation.Contains($root, [StringComparison]::Ordinal) "documentation omitted golden semantic group: $root"
  }
}

function Write-RawFixture {
  param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Text)

  [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
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
    'Add-LocalGroupMember', 'Remove-LocalGroupMember',
    'Start-Service', 'Stop-Service', 'Restart-Service', 'Set-Service', 'New-Service', 'Remove-Service',
    'New-VM', 'Set-VM', 'Remove-VM', 'Start-VM', 'Stop-VM', 'Import-VM', 'Export-VM',
    'Checkpoint-VM', 'Restore-VM', 'Suspend-VM', 'Resume-VM', 'Add-VMHardDiskDrive',
    'Remove-VMHardDiskDrive', 'Add-VMNetworkAdapter', 'Remove-VMNetworkAdapter', 'Set-VMProcessor',
    'Set-VMMemory', 'New-VMSwitch', 'Set-VMSwitch', 'Remove-VMSwitch', 'Add-VMSwitchExtension', 'Remove-VMSwitchExtension', 'New-VHD', 'Set-VHD',
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

function Assert-PathSafetyAndDeterminism {
  $source = [IO.File]::ReadAllText($scriptPath)
  foreach ($required in @(
      'function Get-SafeProvenanceSnapshot',
      'function ConvertFrom-RawJsonObject',
      'function Test-RawJsonObjectMembers',
      '[System.Text.Json.JsonDocument]::Parse',
      '[StringComparer]::OrdinalIgnoreCase',
      'function Assert-NotReparsePoint',
      'function Get-PathItemWithoutFollowingReparse',
      'function Assert-OrdinaryPathAncestry',
      'function Get-OrdinaryPathItem',
      'function Get-OrdinaryProvenanceFile',
      'function Get-JsonObjectPropertyNames',
      '[IO.FileAttributes]::ReparsePoint',
      '$ancestor = $ancestor.Directory',
      '$ancestor = $ancestor.Parent',
      '$beforeHash = Get-Sha256File',
      '$afterHash = Get-Sha256File',
      '$verifiedItem = Get-OrdinaryProvenanceFile',
      'provenance evidence changed while it was read',
      'function Test-ClosedWorldObject',
      'provenance.unknown_property_count.$unknownPropertyCount',
      'Get-ObservationDigest -Observations $orderedObservations'
    )) {
    Assert-True -Condition $source.Contains($required, [StringComparison]::Ordinal) `
      -Message "R1 preflight omitted required safety binding: $required"
  }
  Assert-True -Condition ($source -notmatch [regex]::Escape(
      'EvidenceSourceDigest (Get-Sha256Text -Text ([DateTimeOffset]::UtcNow.ToString(''O'')))'
    )) -Message 'live result digest must not be derived from wall-clock time'
  Assert-True -Condition ($source -notmatch '\.PSObject\.Properties\.Name') `
    -Message 'preflight retained implicit property-name enumeration'
}

function Assert-PathAncestryBehavior {
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

  foreach ($case in @(
      @{ path = $reparsePath; directory = $true; description = 'reparse item' },
      @{ path = (Join-Path $reparsePath 'nested'); directory = $true; description = 'direct reparse parent' },
      @{ path = (Join-Path $reparsePath 'nested\evidence.json'); directory = $false; description = 'reparse grandparent' }
    )) {
    $rejected = $false
    try {
      Get-OrdinaryPathItem -Path $case.path -RequireDirectory $case.directory -Description $case.description | Out-Null
    } catch {
      $rejected = $true
    }
    Assert-True -Condition $rejected -Message "$($case.description) was accepted"
  }
  return 6
}

function Write-CaseFixture {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$Code,
    [Parameter(Mandatory)][string]$Status
  )

  $fixture = Read-ContractJson -Path $fixturePath
  $row = @($fixture.observations | Where-Object { $_.code -ceq $Code })[0]
  if ($null -eq $row) { throw "test fixture did not contain observation $Code" }
  $row.status = $Status
  Write-Fixture -Path $Path -Fixture $fixture
}

function Assert-RestoredBaselineCoverage {
  $testSource = [IO.File]::ReadAllText($PSCommandPath)
  foreach ($marker in @(
      'function Assert-PathSafetyAndDeterminism', 'function Assert-PathAncestryBehavior',
      'Add-LocalGroupMember', 'Remove-LocalGroupMember', 'Add-VMSwitchExtension',
      'Remove-VMSwitchExtension', '$observationCases = @(', 'malformed or scalar root did not return structured blocker'
    )) {
    Assert-True -Condition $testSource.Contains($marker, [StringComparison]::Ordinal) "restored baseline marker missing: $marker"
  }
  Assert-True -Condition ([regex]::Matches($testSource, "@\{ name = '").Count -ge 26) 'restored observation case count regressed'
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
  'Get-Partition', 'Get-Disk', 'Get-VHD', 'Get-ItemProperty', 'Get-ChildItem',
  'Get-NetAdapter', 'Get-NetIPAddress', 'Get-NetRoute', 'Get-NetFirewallProfile',
  'Get-StoragePool', 'Get-PhysicalDisk', 'Get-VirtualDisk', 'Get-Credential', 'Get-Secret',
  'Invoke-WebRequest', 'Invoke-RestMethod', 'Start-Process', 'git', 'gh', 'curl.exe',
  'cmd.exe', 'powershell.exe'
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

function Assert-RawParserAndSanitization {
  param(
    [Parameter(Mandatory)][string]$FixtureText,
    [Parameter(Mandatory)][object]$SanitizedErrorBehavior
  )

  $duplicateCases = @(
    @{ name = 'root-exact'; text = $FixtureText.Replace('"schema_version": 1,', '"schema_version": 1, "schema_version": 1,') },
    @{ name = 'root-case-conflict'; text = $FixtureText.Replace('"schema_version": 1,', '"schema_version": 1, "Schema_Version": 1,') },
    @{ name = 'candidate'; text = $FixtureText.Replace('"sha": "8fab858000111e35ab01789f2cbb6645dda3e6e7",', '"sha": "8fab858000111e35ab01789f2cbb6645dda3e6e7", "SHA": "8fab858000111e35ab01789f2cbb6645dda3e6e7",') },
    @{ name = 'hash'; text = $FixtureText.Replace('"archive_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",', '"archive_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ARCHIVE_SHA256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",') },
    @{ name = 'support'; text = $FixtureText.Replace('"support_status": "untested",', '"support_status": "untested", "SUPPORT_STATUS": "untested",') },
    @{ name = 'authority'; text = $FixtureText.Replace('"authority": "none",', '"authority": "none", "AUTHORITY": "none",') },
    @{ name = 'generation'; text = $FixtureText.Replace('"generation": 2,', '"generation": 2, "GENERATION": 2,') },
    @{ name = 'security'; text = $FixtureText.Replace('"secure_boot_enabled": true,', '"secure_boot_enabled": true, "SECURE_BOOT_ENABLED": true,') },
    @{ name = 'source-reference'; text = $FixtureText.Replace('"reference": "SRC-WS2022-EVAL-20260901",', '"reference": "SRC-WS2022-EVAL-20260901", "REFERENCE": "SRC-WS2022-EVAL-20260901",') },
    @{ name = 'creation-evidence'; text = $FixtureText.Replace('"created_by_evidence_id": "EVID-CREATOR-20260901"', '"created_by_evidence_id": "EVID-CREATOR-20260901", "CREATED_BY_EVIDENCE_ID": "EVID-CREATOR-20260901"') },
    @{ name = 'array-depth'; text = '{"outer":[{"member":1,"MEMBER":2}]}' }
  )
  foreach ($case in $duplicateCases) {
    $path = Join-Path $temporaryRoot ("duplicate-" + $case.name + '.json')
    Write-RawFixture -Path $path -Text $case.text
    $raw = ConvertFrom-RawJsonObject -Bytes ([IO.File]::ReadAllBytes($path))
    Assert-True -Condition (-not $raw.valid -and $raw.code -ceq 'json.duplicate_or_ambiguous_member') "raw duplicate was accepted: $($case.name)"
    $result = Invoke-Fixture -Path $path
    Assert-True -Condition ($result.disposition -ceq 'BLOCKED' -and @($result.blockers) -ccontains $SanitizedErrorBehavior.duplicate_or_case_conflict) "duplicate fixture did not return structured blocker: $($case.name)"
  }

  foreach ($text in @('{', '{"schema_version": 1', '7', '"scalar"', 'true', 'null', '[]')) {
    $path = Join-Path $temporaryRoot ('invalid-root-' + ([Guid]::NewGuid().ToString('N')) + '.json')
    Write-RawFixture -Path $path -Text $text
    $result = Invoke-Fixture -Path $path
    Assert-True -Condition ($result.disposition -ceq 'BLOCKED' -and @($result.blockers) -ccontains $SanitizedErrorBehavior.malformed_or_invalid_root) 'malformed or scalar root did not return structured blocker'
    $serialized = $result | ConvertTo-Json -Compress -Depth 16
    Assert-True -Condition (-not $serialized.Contains('PropertyNotFoundException', [StringComparison]::Ordinal)) 'scalar root leaked parser exception'
  }

  $fixture = Read-ContractJson -Path $fixturePath
  $unknownNames = @(
    ('AUDIT_' + 'SECRET' + '_TOKEN'),
    ('AUDIT_' + 'PRIVATE' + '_PATH'),
    ('AUDIT_CONTROL_' + [char]1),
    ('AUDIT_' + ('X' * 320))
  )
  foreach ($name in $unknownNames) {
    $unknownFixture = Copy-ContractObject -Value $fixture
    $unknownFixture.provenance | Add-Member -NotePropertyName $name -NotePropertyValue 'synthetic'
    $path = Join-Path $temporaryRoot ('unknown-' + ([Guid]::NewGuid().ToString('N')) + '.json')
    Write-Fixture -Path $path -Fixture $unknownFixture
    $result = Invoke-Fixture -Path $path
    $serialized = $result | ConvertTo-Json -Compress -Depth 16
    $expectedUnknownBlocker = ([string]$SanitizedErrorBehavior.unknown_property).Replace('N', '1')
    Assert-True -Condition ($result.disposition -ceq 'BLOCKED' -and @($result.blockers) -ccontains $expectedUnknownBlocker) 'unknown property did not return sanitized blocker code'
    Assert-True -Condition (-not $serialized.Contains($name, [StringComparison]::Ordinal) -and
      -not $serialized.Contains('synthetic', [StringComparison]::Ordinal)) 'unknown property receipt reflected attacker input'
  }
  return [pscustomobject]@{ duplicate_cases = $duplicateCases.Count; scalar_malformed_cases = 7; unknown_property_cases = $unknownNames.Count }
}

function Assert-EmptyObjectBehavior {
  param(
    [Parameter(Mandatory)][object]$Fixture,
    [Parameter(Mandatory)][object[]]$Fields
  )

  $emptyNames = @(Get-JsonObjectPropertyNames -Value ([pscustomobject]@{}))
  $singleNames = @(Get-JsonObjectPropertyNames -Value ([pscustomobject]@{ alpha = 1 }))
  $multipleNames = @(Get-JsonObjectPropertyNames -Value ([pscustomobject]@{ alpha = 1; beta = 2 }))
  Assert-True -Condition ($emptyNames.Count -eq 0) 'empty object property-name enumeration was not empty'
  Assert-True -Condition ($singleNames.Count -eq 1 -and $singleNames[0] -ceq 'alpha') 'single property-name enumeration drifted'
  Assert-True -Condition (($multipleNames -join '|') -ceq 'alpha|beta') 'multiple property-name enumeration drifted'
  foreach ($nonObject in @($null, 'scalar', [object[]]@('array'))) {
    Assert-True -Condition (-not (Test-JsonObject -Value $nonObject)) 'non-object input was classified as a JSON object'
  }

  $rootPath = Join-Path $temporaryRoot 'empty-fixture-root.json'
  Write-RawFixture -Path $rootPath -Text '{}'
  $rootFirst = Invoke-StructuredFixture -Path $rootPath
  $rootSecond = Invoke-StructuredFixture -Path $rootPath
  Assert-True -Condition ($rootFirst.raw -ceq $rootSecond.raw) 'empty fixture-root output was not deterministic'
  Assert-True -Condition ($rootFirst.result.disposition -ceq 'BLOCKED' -and
    @($rootFirst.result.blockers) -ccontains 'fixture.schema_invalid') 'empty fixture root did not return the fixture.schema_invalid blocker'
  Assert-True -Condition ($rootFirst.result.evidence_source -ceq 'fixture' -and
    $rootFirst.result.mutation_flags.host_observation -eq $false) 'empty fixture root left fixture mode'

  $containers = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
  [void]$containers.Add('')
  foreach ($field in $Fields) {
    $segments = ([string]$field.path).Split('.')
    for ($depth = 1; $depth -lt $segments.Count; $depth++) {
      [void]$containers.Add(($segments[0..($depth - 1)] -join '.'))
    }
  }
  $cases = [System.Collections.Generic.List[object]]::new()
  foreach ($container in @($containers | Sort-Object)) {
    $prefix = if ($container.Length -eq 0) { 'provenance.required_field_missing.' } else { "provenance.required_field_missing.$container." }
    $cases.Add([pscustomobject]@{
        name = if ($container.Length -eq 0) { 'empty-provenance-root' } else { "empty-provenance-$container" }
        container_paths = @($container)
        required_prefixes = @($prefix)
        single_property = $false
      })
  }
  $cases.Add([pscustomobject]@{
      name = 'multiple-empty-nested-objects'
      container_paths = @('image_source', 'creation')
      required_prefixes = @('provenance.required_field_missing.image_source.', 'provenance.required_field_missing.creation.')
      single_property = $false
    })
  $cases.Add([pscustomobject]@{
      name = 'single-valid-property-incomplete-object'
      container_paths = @('image_source')
      required_prefixes = @('provenance.required_field_missing.image_source.')
      single_property = $true
    })

  foreach ($case in $cases) {
    $candidate = Copy-ContractObject -Value $Fixture
    if ($case.single_property) {
      $candidate.provenance.image_source = [pscustomobject]@{ reference = [string]$Fixture.provenance.image_source.reference }
    } else {
      foreach ($container in @($case.container_paths)) {
        if ($container.Length -eq 0) {
          $candidate.provenance = [pscustomobject]@{}
        } else {
          Set-JsonPathValue -Object $candidate.provenance -Path $container -Value ([pscustomobject]@{})
        }
      }
    }
    $path = Join-Path $temporaryRoot ($case.name + '.json')
    Write-Fixture -Path $path -Fixture $candidate
    $first = Invoke-StructuredFixture -Path $path
    $second = Invoke-StructuredFixture -Path $path
    Assert-True -Condition ($first.raw -ceq $second.raw) "empty-object output was not deterministic: $($case.name)"
    Assert-True -Condition ($first.result.disposition -ceq 'BLOCKED' -and
      @($first.result.blockers) -ccontains 'precondition_failed.image_provenance' -and
      -not [bool]$first.result.provenance_validation.valid) "empty object was not structurally blocked: $($case.name)"
    foreach ($prefix in @($case.required_prefixes)) {
      Assert-True -Condition (@($first.result.provenance_validation.blockers | Where-Object { $_ -like "$prefix*" }).Count -gt 0) `
        "empty object did not retain required-field blocker family: $($case.name)"
    }
  }
  Assert-FixtureIsolation -Path $rootPath
  return 1 + $cases.Count
}

function Assert-PlaceholderPolicy {
  param([Parameter(Mandatory)][object]$Fixture)

  $markers = @('required_owner', 'TODO', 'tBd', 'CHANGE-ME', 'sample', 'EXAMPLE', 'dummy', 'unknown', 'NOT_EXECUTED', 'fixme')
  $cases = 0
  foreach ($definition in @(
      @{ path = 'image_source.reference'; prefix = 'SRC'; suffix = 'MEDIA-20260902' },
      @{ path = 'creation.created_by_evidence_id'; prefix = 'EVID'; suffix = 'CREATOR-20260902' }
    )) {
    foreach ($marker in $markers + @("`u{FF34}`u{FF2F}`u{FF24}`u{FF2F}")) {
      $candidate = Copy-ContractObject -Value $Fixture.provenance
      Set-JsonPathValue -Object $candidate -Path $definition.path -Value "$($definition.prefix)-$marker-$($definition.suffix)"
      $validation = Test-HyperVImageProvenance -Provenance $candidate -ExpectedIdentity $Fixture.expected_identity
      Assert-True -Condition (-not $validation.valid -and @($validation.blockers) -ccontains "provenance.placeholder_value.$($definition.path)") `
        "placeholder marker was admitted: $($definition.path)"
      $cases += 1
    }
  }
  foreach ($nearMiss in @(
      @{ path = 'image_source.reference'; value = 'SRC-EXAMPLE123-MEDIA-20260902' },
      @{ path = 'image_source.reference'; value = 'SRC-CHANGELOG-MEDIA-20260902' },
      @{ path = 'creation.created_by_evidence_id'; value = 'EVID-SAMPLER-CREATOR-20260902' },
      @{ path = 'creation.created_by_evidence_id'; value = 'EVID-TODOLIST-CREATOR-20260902' }
    )) {
    $candidate = Copy-ContractObject -Value $Fixture.provenance
    Set-JsonPathValue -Object $candidate -Path $nearMiss.path -Value $nearMiss.value
    $validation = Test-HyperVImageProvenance -Provenance $candidate -ExpectedIdentity $Fixture.expected_identity
    Assert-True -Condition $validation.valid "safe near-miss evidence was over-rejected: $($nearMiss.value)"
  }
  $sourceFixture = Copy-ContractObject -Value $Fixture
  $sourceFixture.provenance.image_source.reference = 'SRC-NOT_EXECUTED'
  $sourcePath = Join-Path $temporaryRoot 'placeholder-source.json'
  Write-Fixture -Path $sourcePath -Fixture $sourceFixture
  $sourceResult = Invoke-Fixture -Path $sourcePath
  Assert-True -Condition ($sourceResult.disposition -ceq 'BLOCKED' -and @($sourceResult.blockers) -ccontains 'provenance.placeholder_value.image_source.reference') 'fixture source placeholder was admitted'
  $creatorFixture = Copy-ContractObject -Value $Fixture
  $creatorFixture.provenance.creation.created_by_evidence_id = 'EVID-SAMPLE-CREATOR'
  $creatorPath = Join-Path $temporaryRoot 'placeholder-creator.json'
  Write-Fixture -Path $creatorPath -Fixture $creatorFixture
  $creatorResult = Invoke-Fixture -Path $creatorPath
  Assert-True -Condition ($creatorResult.disposition -ceq 'BLOCKED' -and @($creatorResult.blockers) -ccontains 'provenance.placeholder_value.creation.created_by_evidence_id') 'fixture creator placeholder was admitted'
  return [pscustomobject]@{ placeholder_cases = $cases + 2; safe_near_miss_cases = 4 }
}

New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
  $executedCases = 0
  $negativeCases = 0
  $placeholderCases = 0
  $safeNearMissCases = 0
  $duplicateMemberCases = 0
  $scalarMalformedCases = 0
  $emptyObjectCases = 0
  $matrix = Get-IndependentGoldenMatrix
  $matrixFields = @($matrix.fields)
  Assert-StaticProductionSafety
  $executedCases += 1
  Assert-RestoredBaselineCoverage
  $executedCases += 1

  . $scriptPath -FixturePath $fixturePath | Out-Null
  Assert-PathSafetyAndDeterminism
  $executedCases += 1
  $fixture = Read-ContractJson -Path $fixturePath
  $template = Read-ContractJson -Path $templatePath
  Assert-TemplateMatchesGoldenMatrix -Template $template -Fields $matrixFields
  Assert-ProductionSchemaMatchesGoldenMatrix -Fields $matrixFields
  Assert-RustTemplateTestMatchesGoldenMatrix -Fields $matrixFields
  Assert-DocumentationMatchesGoldenMatrix -Fields $matrixFields
  $executedCases += 5
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

  $directEligible = Test-HyperVImageProvenance -Provenance $fixture.provenance -ExpectedIdentity $fixture.expected_identity
  Assert-True -Condition ($directEligible.valid -and $directEligible.required_field_count -eq $matrixFields.Count) 'validator and eligible fixture disagree'
  foreach ($field in $matrixFields) {
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
    $semantic = Copy-ContractObject -Value $fixture.provenance
    $semanticInvalidValue = switch ($field.json_type) {
      'string' { '!' }
      'integer' { [Int64]0 }
      'boolean' { -not [bool](Get-JsonPathEntry -Object $semantic -Path $field.path).value }
    }
    Set-JsonPathValue -Object $semantic -Path $field.path -Value $semanticInvalidValue
    $validation = Test-HyperVImageProvenance -Provenance $semantic -ExpectedIdentity $fixture.expected_identity
    Assert-True -Condition (-not $validation.valid) "golden semantic constraint was admitted: $($field.path)"
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

  $observationCases = @(
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
  foreach ($case in $observationCases) {
    $casePath = Join-Path $temporaryRoot ($case.name + '.json')
    Write-CaseFixture -Path $casePath -Code $case.code -Status $case.status
    $result = Invoke-Fixture -Path $casePath
    $expected = if ($case.status -ceq 'unavailable') { "evidence_gap.$($case.code)" } else { "precondition_failed.$($case.code)" }
    Assert-True -Condition ($result.disposition -ceq 'BLOCKED' -and @($result.blockers) -ccontains $expected) `
      "restored baseline observation case failed: $($case.name)"
    $executedCases += 1
  }

  $rawStats = Assert-RawParserAndSanitization -FixtureText (Get-Content -LiteralPath $fixturePath -Raw) `
    -SanitizedErrorBehavior $matrix.sanitized_error_behavior
  $duplicateMemberCases = [int]$rawStats.duplicate_cases
  $scalarMalformedCases = [int]$rawStats.scalar_malformed_cases
  $executedCases += $duplicateMemberCases + $scalarMalformedCases + [int]$rawStats.unknown_property_cases
  $emptyObjectCases = Assert-EmptyObjectBehavior -Fixture $fixture -Fields $matrixFields
  $executedCases += $emptyObjectCases
  $placeholderStats = Assert-PlaceholderPolicy -Fixture $fixture
  $placeholderCases += [int]$placeholderStats.placeholder_cases
  $safeNearMissCases = [int]$placeholderStats.safe_near_miss_cases
  $executedCases += [int]$placeholderStats.placeholder_cases + $safeNearMissCases

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
  $executedCases += Assert-PathAncestryBehavior
} finally {
  Remove-Item -LiteralPath $temporaryRoot -Force -Recurse -ErrorAction SilentlyContinue
}

Write-Host "Windows Hyper-V R1 provenance contract tests passed (fields=$($matrixFields.Count); negative_cases=$negativeCases; placeholder_cases=$placeholderCases; safe_near_miss_cases=$safeNearMissCases; duplicate_member_cases=$duplicateMemberCases; scalar_malformed_cases=$scalarMalformedCases; empty_object_cases=$emptyObjectCases; fixture_cases=$executedCases)"
