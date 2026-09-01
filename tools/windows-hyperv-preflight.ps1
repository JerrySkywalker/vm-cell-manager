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
  [string]$CandidateChecksumManifestPath,

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

$contract = 'vmcell.hyperv-r5-preflight.v2'
$fixtureContract = 'vmcell.hyperv-r5-preflight-fixture.v2'
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
    $convertFromJsonParameters = @{ ErrorAction = 'Stop' }
    if ($PSVersionTable.PSVersion -ge [Version]'7.5') {
      $convertFromJsonParameters.DateKind = 'String'
    }
    $value = [Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json @convertFromJsonParameters
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
    [Parameter(Mandatory)][object]$InputObject,
    [Parameter(Mandatory)][string]$Name
  )

  $property = $InputObject.PSObject.Properties[$Name]
  if ($null -eq $property) { return $null }
  return $property.Value
}

function New-ProvenanceField {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][ValidateSet('string', 'integer', 'boolean')][string]$JsonType,
    [AllowNull()][object]$ExactValue,
    [string[]]$AllowedValues = @(),
    [ValidateSet('', 'sha256', 'sha40', 'opaque', 'source_reference', 'archive_name', 'timestamp', 'windows_server_2022_build')][string]$ValueKind = '',
    [bool]$Positive = $false,
    [bool]$AllowUnknownRequiresOwnerAttestation = $false
  )

  return [pscustomobject]@{
    path = $Path
    json_type = $JsonType
    has_exact_value = $PSBoundParameters.ContainsKey('ExactValue')
    exact_value = $ExactValue
    allowed_values = @($AllowedValues)
    value_kind = $ValueKind
    positive = $Positive
    allow_unknown_requires_owner_attestation = $AllowUnknownRequiresOwnerAttestation
  }
}

function Get-HyperVImageProvenanceSchema {
  return @(
    (New-ProvenanceField -Path 'schema_version' -JsonType integer -ExactValue 2),
    (New-ProvenanceField -Path 'contract' -JsonType string -ExactValue 'vmcell.hyperv-r5-image-provenance.v2'),
    (New-ProvenanceField -Path 'authority' -JsonType string -ExactValue 'none'),
    (New-ProvenanceField -Path 'acceptance' -JsonType boolean -ExactValue $false),
    (New-ProvenanceField -Path 'authorizing' -JsonType boolean -ExactValue $false),
    (New-ProvenanceField -Path 'admission_result' -JsonType string -ExactValue 'NOT_PROMOTED'),
    (New-ProvenanceField -Path 'admission_status' -JsonType string -ExactValue 'NOT_STARTED'),
    (New-ProvenanceField -Path 'real_platform_acceptance' -JsonType string -ExactValue 'not_started'),
    (New-ProvenanceField -Path 'support_status' -JsonType string -ExactValue 'untested'),
    (New-ProvenanceField -Path 'admitted_for_candidate_sha' -JsonType string -ValueKind sha40),
    (New-ProvenanceField -Path 'candidate.sha' -JsonType string -ValueKind sha40),
    (New-ProvenanceField -Path 'candidate.version' -JsonType string -ExactValue '0.4.1'),
    (New-ProvenanceField -Path 'candidate.frozen_release_ref' -JsonType string -ExactValue 'release/v0.4.1'),
    (New-ProvenanceField -Path 'candidate.frozen_release_sha' -JsonType string -ExactValue '0e7fcf37f4310562d318f9d5c709ddf8e8ca1637'),
    (New-ProvenanceField -Path 'package.archive_name' -JsonType string -ValueKind archive_name),
    (New-ProvenanceField -Path 'package.archive_sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'package.checksum_manifest_sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'package.sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'candidate_binary.name' -JsonType string -ExactValue 'vmcell.exe'),
    (New-ProvenanceField -Path 'candidate_binary.version' -JsonType string -ExactValue '0.4.1'),
    (New-ProvenanceField -Path 'candidate_binary.sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'windows.product' -JsonType string -ExactValue 'Windows Server 2022'),
    (New-ProvenanceField -Path 'windows.edition' -JsonType string -ExactValue 'Standard'),
    (New-ProvenanceField -Path 'windows.version' -JsonType string -ExactValue '21H2'),
    (New-ProvenanceField -Path 'windows.build' -JsonType string -ValueKind windows_server_2022_build),
    (New-ProvenanceField -Path 'windows.architecture' -JsonType string -ExactValue 'x86_64'),
    (New-ProvenanceField -Path 'hyperv.generation' -JsonType integer -ExactValue 2),
    (New-ProvenanceField -Path 'hyperv.secure_boot_enabled' -JsonType boolean -ExactValue $true),
    (New-ProvenanceField -Path 'hyperv.secure_boot_template' -JsonType string -ExactValue 'MicrosoftWindows'),
    (New-ProvenanceField -Path 'hyperv.powershell_direct' -JsonType string -ExactValue 'EXPECTED'),
    (New-ProvenanceField -Path 'hyperv.qemu_guest_agent' -JsonType string -ExactValue 'NOT_APPLICABLE'),
    (New-ProvenanceField -Path 'image_source.reference' -JsonType string -ValueKind source_reference),
    (New-ProvenanceField -Path 'image_source.evidence_id' -JsonType string -ValueKind opaque),
    (New-ProvenanceField -Path 'image_source.source_sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'image_source.acquisition_method' -JsonType string -AllowedValues @('OWNER_SUPPLIED_MEDIA', 'OWNER_BUILD_PIPELINE', 'OWNER_PREPARED_IMAGE')),
    (New-ProvenanceField -Path 'image_source.build_method' -JsonType string -AllowedValues @('SYSPREP_GENERALIZED', 'BASELINE_IMAGE', 'OWNER_ATTESTED_BUILD')),
    (New-ProvenanceField -Path 'vhdx.format' -JsonType string -ExactValue 'VHDX'),
    (New-ProvenanceField -Path 'vhdx.sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'vhdx.size_bytes' -JsonType integer -Positive $true),
    (New-ProvenanceField -Path 'vhdx.canonical_path_sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'vhdx.parentless' -JsonType boolean -ExactValue $true),
    (New-ProvenanceField -Path 'vhdx.attached' -JsonType boolean -ExactValue $false),
    (New-ProvenanceField -Path 'vhdx.detached' -JsonType boolean -ExactValue $true),
    (New-ProvenanceField -Path 'vhdx.immutable_owner_policy' -JsonType string -ExactValue 'OWNER_ATTESTED_READ_ONLY'),
    (New-ProvenanceField -Path 'vhdx.ordinary_non_reparse_evidence_id' -JsonType string -ValueKind opaque),
    (New-ProvenanceField -Path 'vhdx.backing_chain' -JsonType string -ExactValue 'NONE'),
    (New-ProvenanceField -Path 'vhdx.preparation_timestamp_utc' -JsonType string -ValueKind timestamp),
    (New-ProvenanceField -Path 'vhdx.preparation_evidence_id' -JsonType string -ValueKind opaque),
    (New-ProvenanceField -Path 'vhdx.secrets_present' -JsonType boolean -ExactValue $false),
    (New-ProvenanceField -Path 'vhdx.credentials_embedded' -JsonType string -AllowedValues @('OWNER_ATTESTED_NONE', 'UNKNOWN_REQUIRES_OWNER_ATTESTATION') -AllowUnknownRequiresOwnerAttestation $true),
    (New-ProvenanceField -Path 'creation.created_at_utc' -JsonType string -ValueKind timestamp),
    (New-ProvenanceField -Path 'creation.created_by_evidence_id' -JsonType string -ValueKind opaque),
    (New-ProvenanceField -Path 'immutability.owner_policy' -JsonType string -ExactValue 'OWNER_ATTESTED_READ_ONLY'),
    (New-ProvenanceField -Path 'immutability.verification_evidence_sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'admission_receipt.receipt_id' -JsonType string -ValueKind opaque),
    (New-ProvenanceField -Path 'admission_receipt.issued_at_utc' -JsonType string -ValueKind timestamp),
    (New-ProvenanceField -Path 'admission_receipt.sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'exclusive_window.eligible' -JsonType boolean -ExactValue $true),
    (New-ProvenanceField -Path 'exclusive_window.starts_at_utc' -JsonType string -ValueKind timestamp),
    (New-ProvenanceField -Path 'exclusive_window.ends_at_utc' -JsonType string -ValueKind timestamp),
    (New-ProvenanceField -Path 'exclusive_window.evidence_sha256' -JsonType string -ValueKind sha256),
    (New-ProvenanceField -Path 'license_evaluation.review_status' -JsonType string -ExactValue 'HUMAN_REVIEW_REQUIRED'),
    (New-ProvenanceField -Path 'license_evaluation.evidence_id' -JsonType string -ValueKind opaque)
  )
}

function Test-JsonObject {
  param([AllowNull()][object]$Value)

  return $null -ne $Value -and $Value -is [pscustomobject]
}

function Test-JsonInteger {
  param([AllowNull()][object]$Value)

  return $Value -is [sbyte] -or $Value -is [byte] -or $Value -is [int16] -or
    $Value -is [uint16] -or $Value -is [int32] -or $Value -is [uint32] -or
    $Value -is [int64] -or $Value -is [uint64]
}

function Get-JsonPathValue {
  param(
    [AllowNull()][object]$InputObject,
    [AllowEmptyString()][string]$Path
  )

  if ($Path.Length -eq 0) {
    return [pscustomobject]@{ exists = $null -ne $InputObject; value = $InputObject }
  }
  $current = $InputObject
  foreach ($segment in $Path.Split('.')) {
    if (-not (Test-JsonObject -Value $current)) {
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

function Test-ProvenancePlaceholder {
  param(
    [Parameter(Mandatory)][string]$Value,
    [Parameter(Mandatory)][bool]$AllowUnknownRequiresOwnerAttestation
  )

  $normalized = $Value.Trim()
  if ($AllowUnknownRequiresOwnerAttestation -and $normalized -ceq 'UNKNOWN_REQUIRES_OWNER_ATTESTATION') {
    return $false
  }
  return $normalized -cmatch '^(?i:REQUIRED_|TODO|TBD|FIXME|UNKNOWN)'
}

function Test-ProvenanceTimestamp {
  param([Parameter(Mandatory)][string]$Value)

  if ($Value -cnotmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,7})?Z$') {
    return $false
  }
  [DateTimeOffset]$parsed = [DateTimeOffset]::MinValue
  return [DateTimeOffset]::TryParse($Value, [Globalization.CultureInfo]::InvariantCulture,
    [Globalization.DateTimeStyles]::AssumeUniversal, [ref]$parsed)
}

function Test-HyperVImageProvenance {
  param(
    [Parameter(Mandatory)][AllowNull()][object]$Provenance,
    [AllowNull()][object]$ExpectedIdentity
  )

  $schema = @(Get-HyperVImageProvenanceSchema)
  $blockers = [System.Collections.Generic.List[string]]::new()
  $actions = [System.Collections.Generic.List[string]]::new()
  $parentFields = @{}
  foreach ($field in $schema) {
    $segments = $field.path.Split('.')
    for ($index = 0; $index -lt $segments.Count; $index++) {
      $parent = if ($index -eq 0) { '' } else { ($segments[0..($index - 1)] -join '.') }
      if (-not $parentFields.ContainsKey($parent)) {
        $parentFields[$parent] = [System.Collections.Generic.List[string]]::new()
      }
      if ($parentFields[$parent] -cnotcontains $segments[$index]) {
        $parentFields[$parent].Add($segments[$index])
      }
    }
  }
  foreach ($parent in $parentFields.Keys) {
    $entry = Get-JsonPathValue -InputObject $Provenance -Path $parent
    $parentLabel = if ($parent.Length -eq 0) { 'root' } else { $parent }
    if (-not $entry.exists) {
      $blockers.Add("provenance.required_object_missing.$parentLabel")
      continue
    }
    if (-not (Test-JsonObject -Value $entry.value)) {
      $blockers.Add("provenance.invalid_type.$parentLabel")
      continue
    }
    foreach ($property in $entry.value.PSObject.Properties.Name) {
      if ($parentFields[$parent] -cnotcontains $property) {
        $propertyPath = if ($parent.Length -eq 0) { $property } else { "$parent.$property" }
        $blockers.Add("provenance.unknown_property.$propertyPath")
      }
    }
  }
  foreach ($field in $schema) {
    $entry = Get-JsonPathValue -InputObject $Provenance -Path $field.path
    if (-not $entry.exists) {
      $blockers.Add("provenance.required_field_missing.$($field.path)")
      continue
    }
    if ($null -eq $entry.value) {
      $blockers.Add("provenance.null_value.$($field.path)")
      continue
    }
    $typeValid = switch ($field.json_type) {
      'string' { $entry.value -is [string] }
      'integer' { Test-JsonInteger -Value $entry.value }
      'boolean' { $entry.value -is [bool] }
    }
    if (-not $typeValid) {
      $blockers.Add("provenance.invalid_type.$($field.path)")
      continue
    }
    if ($field.json_type -eq 'string') {
      $value = [string]$entry.value
      if ([string]::IsNullOrWhiteSpace($value)) {
        $blockers.Add("provenance.empty_value.$($field.path)")
        continue
      }
      if (Test-ProvenancePlaceholder -Value $value -AllowUnknownRequiresOwnerAttestation $field.allow_unknown_requires_owner_attestation) {
        $blockers.Add("provenance.placeholder_value.$($field.path)")
        continue
      }
    }
    if ($field.has_exact_value -and $entry.value -cne $field.exact_value) {
      $blockers.Add("provenance.unsupported_value.$($field.path)")
      continue
    }
    if ($field.allowed_values.Count -gt 0 -and $field.allowed_values -cnotcontains $entry.value) {
      $blockers.Add("provenance.unsupported_value.$($field.path)")
      continue
    }
    if ($field.positive -and [Int64]$entry.value -le 0) {
      $blockers.Add("provenance.invalid_value.$($field.path)")
      continue
    }
    $kindValid = switch ($field.value_kind) {
      '' { $true }
      'sha256' { $entry.value -cmatch '^[0-9a-f]{64}$' }
      'sha40' { $entry.value -cmatch '^[0-9a-f]{40}$' }
      'opaque' { $entry.value -cmatch '^EVID-[A-Z0-9._-]{8,127}$' }
      'source_reference' { $entry.value -cmatch '^SRC-[A-Z0-9._-]{8,127}$' }
      'archive_name' { $entry.value -cmatch '^[A-Za-z0-9][A-Za-z0-9._-]{2,127}$' }
      'timestamp' { Test-ProvenanceTimestamp -Value $entry.value }
      'windows_server_2022_build' { $entry.value -cmatch '^20348\.\d+$' }
    }
    if (-not $kindValid) {
      $blockers.Add("provenance.invalid_value.$($field.path)")
    }
  }
  foreach ($relation in @(
      @{ left = 'admitted_for_candidate_sha'; right = 'candidate.sha' },
      @{ left = 'package.archive_sha256'; right = 'package.sha256' },
      @{ left = 'candidate.version'; right = 'candidate_binary.version' },
      @{ left = 'vhdx.attached'; right = 'vhdx.detached'; inverse_boolean = $true },
      @{ left = 'exclusive_window.starts_at_utc'; right = 'exclusive_window.ends_at_utc'; ordered_timestamps = $true }
    )) {
    $left = Get-JsonPathValue -InputObject $Provenance -Path $relation.left
    $right = Get-JsonPathValue -InputObject $Provenance -Path $relation.right
    if ($left.exists -and $right.exists) {
      $inverseBoolean = $relation.ContainsKey('inverse_boolean') -and [bool]$relation.inverse_boolean
      $orderedTimestamps = $relation.ContainsKey('ordered_timestamps') -and [bool]$relation.ordered_timestamps
      if ($inverseBoolean) {
        $matches = [bool]$left.value -eq -not [bool]$right.value
      } elseif ($orderedTimestamps) {
        $matches = $false
        try {
          if ($left.value -is [string] -and $right.value -is [string]) {
            $matches = [DateTimeOffset]::Parse($left.value) -lt [DateTimeOffset]::Parse($right.value)
          }
        } catch {}
      } else {
        $matches = $left.value -ceq $right.value
      }
      if (-not $matches) { $blockers.Add("provenance.identity_mismatch.$($relation.left)") }
    }
  }
  if ($null -ne $ExpectedIdentity) {
    if (-not (Test-JsonObject -Value $ExpectedIdentity)) {
      $blockers.Add('provenance.expected_identity_invalid')
    } else {
      foreach ($binding in @(
          @{ actual = 'candidate.sha'; expected = 'candidate_sha' },
          @{ actual = 'candidate.version'; expected = 'candidate_version' },
          @{ actual = 'candidate.frozen_release_ref'; expected = 'frozen_release_ref' },
          @{ actual = 'candidate.frozen_release_sha'; expected = 'frozen_release_sha' },
          @{ actual = 'package.archive_sha256'; expected = 'package_archive_sha256' },
          @{ actual = 'package.checksum_manifest_sha256'; expected = 'package_checksum_manifest_sha256' },
          @{ actual = 'package.sha256'; expected = 'package_sha256' },
          @{ actual = 'candidate_binary.sha256'; expected = 'candidate_binary_sha256' },
          @{ actual = 'vhdx.sha256'; expected = 'vhdx_sha256' },
          @{ actual = 'vhdx.size_bytes'; expected = 'vhdx_size_bytes' },
          @{ actual = 'vhdx.canonical_path_sha256'; expected = 'vhdx_canonical_path_sha256' }
        )) {
        $actual = Get-JsonPathValue -InputObject $Provenance -Path $binding.actual
        $expectedProperty = $ExpectedIdentity.PSObject.Properties[$binding.expected]
        if ($null -eq $expectedProperty) {
          $blockers.Add("provenance.expected_identity_missing.$($binding.expected)")
        } elseif (-not $actual.exists -or $actual.value -cne $expectedProperty.Value) {
          $blockers.Add("provenance.identity_mismatch.$($binding.actual)")
        }
      }
    }
  }
  $credentials = Get-JsonPathValue -InputObject $Provenance -Path 'vhdx.credentials_embedded'
  $requiresOwnerAttestation = $credentials.exists -and $credentials.value -is [string] -and
    $credentials.value.Trim() -ceq 'UNKNOWN_REQUIRES_OWNER_ATTESTATION'
  if ($requiresOwnerAttestation) {
    $actions.Add('obtain_owner_attestation.credentials_embedded')
  }
  if ($blockers.Count -gt 0) {
    $actions.Add('replace_invalid_provenance_with_complete_sanitized_evidence')
  }
  return [ordered]@{
    valid = $blockers.Count -eq 0
    required_field_count = $schema.Count
    blockers = @($blockers | Sort-Object -Unique)
    owner_actions = @($actions | Sort-Object -Unique)
    requires_owner_attestation = $requiresOwnerAttestation
  }
}

function Test-LiveProvenance {
  param(
    [Parameter(Mandatory)][object]$Provenance,
    [AllowNull()][object]$ExpectedIdentity
  )

  return Test-HyperVImageProvenance -Provenance $Provenance -ExpectedIdentity $ExpectedIdentity
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
    authorizing = $false
    real_platform_acceptance = 'not_started'
    support_status = 'untested'
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
    [Parameter(Mandatory)][bool]$HostObservation,
    [AllowNull()][object]$ProvenanceValidation
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
  if ($null -ne $ProvenanceValidation) {
    foreach ($blocker in @($ProvenanceValidation.blockers)) {
      $blockers.Add($blocker)
    }
    foreach ($action in @($ProvenanceValidation.owner_actions)) {
      $actions.Add($action)
    }
    if ([bool]$ProvenanceValidation.requires_owner_attestation) {
      $blockers.Add('owner_attestation.credentials_embedded_required')
      $actions.Add('obtain_owner_attestation.credentials_embedded')
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
    authorizing = $false
    real_platform_acceptance = 'not_started'
    support_status = 'untested'
    disposition = if ($eligible) { 'PREFLIGHT_ELIGIBLE' } else { 'BLOCKED' }
    observations = $orderedObservations
    blockers = @($blockers | Sort-Object -Unique)
    owner_actions = @($actions | Sort-Object -Unique)
    provenance_validation = if ($null -eq $ProvenanceValidation) {
      [ordered]@{ valid = $false; required_field_count = 0; blockers = @('provenance.unavailable') }
    } else {
      [ordered]@{
        valid = [bool]$ProvenanceValidation.valid
        required_field_count = [int]$ProvenanceValidation.required_field_count
        blockers = @($ProvenanceValidation.blockers | Sort-Object -Unique)
      }
    }
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
    }
  }
}

function Convert-FixtureToObservations {
  param([Parameter(Mandatory)][string]$Path)

  try {
    $bytes = [IO.File]::ReadAllBytes([IO.Path]::GetFullPath($Path))
    $text = [Text.Encoding]::UTF8.GetString($bytes)
    $convertFromJsonParameters = @{ ErrorAction = 'Stop' }
    if ($PSVersionTable.PSVersion -ge [Version]'7.5') {
      $convertFromJsonParameters.DateKind = 'String'
    }
    $fixture = $text | ConvertFrom-Json @convertFromJsonParameters
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
  $provenanceValidation = Test-HyperVImageProvenance `
    -Provenance (Get-ObjectProperty -InputObject $fixture -Name 'provenance') `
    -ExpectedIdentity (Get-ObjectProperty -InputObject $fixture -Name 'expected_identity')
  if (-not [bool]$provenanceValidation.valid -or [bool]$provenanceValidation.requires_owner_attestation) {
    $provenanceObservation = @($observations | Where-Object { $_.code -ceq 'image_provenance' })[0]
    $provenanceObservation.status = 'fail'
  }
  return [pscustomobject]@{
    observations = @($observations)
    digest = Get-Sha256Text -Text ([Convert]::ToBase64String($bytes))
    provenance_validation = $provenanceValidation
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

function Get-LiveExpectedProvenanceIdentity {
  $package = Get-OrdinaryPathItem -Path $CandidatePackagePath -RequireDirectory $false -Description 'candidate package path'
  $checksumManifest = Get-OrdinaryPathItem -Path $CandidateChecksumManifestPath -RequireDirectory $false -Description 'candidate checksum manifest path'
  $binary = Get-OrdinaryPathItem -Path $CandidateBinaryPath -RequireDirectory $false -Description 'candidate binary path'
  $vhdx = Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path'
  return [pscustomobject]@{
    candidate_sha = $CandidateSha
    candidate_version = '0.4.1'
    frozen_release_ref = 'release/v0.4.1'
    frozen_release_sha = '0e7fcf37f4310562d318f9d5c709ddf8e8ca1637'
    package_archive_sha256 = Get-Sha256File -Path $package.FullName
    package_checksum_manifest_sha256 = Get-Sha256File -Path $checksumManifest.FullName
    package_sha256 = Get-Sha256File -Path $package.FullName
    candidate_binary_sha256 = Get-Sha256File -Path $binary.FullName
    vhdx_sha256 = Get-Sha256File -Path $vhdx.FullName
    vhdx_size_bytes = [Int64]$vhdx.Length
    vhdx_canonical_path_sha256 = Get-Sha256Text -Text ([IO.Path]::GetFullPath($vhdx.FullName))
  }
}

function Get-LiveObservations {
  $provenanceSnapshot = $null
  $provenanceValidation = $null
  try {
    $provenanceSnapshot = Get-SafeProvenanceSnapshot -Path $ProvenancePath
    $expectedIdentity = Get-LiveExpectedProvenanceIdentity
    $provenanceValidation = Test-LiveProvenance -Provenance $provenanceSnapshot.value -ExpectedIdentity $expectedIdentity
  } catch {}

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
    $state = Get-OrdinaryPathItem -Path $StateRoot -RequireDirectory $true -Description 'state root'
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
    $vhdItem = Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path'
    $exists = -not $vhdItem.PSIsContainer -and $vhdItem.Extension -ieq '.vhdx'
    [pscustomobject]@{ status = if ($exists) { 'pass' } else { 'fail' }; evidence = @{ present = $exists } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_immutability' -Probe {
    $vhdItem = Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path'
    $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
    $readOnly = ($vhdItem.Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0
    $fixed = [string]$vhd.VhdType -ceq 'Fixed'
    [pscustomobject]@{ status = if ($readOnly -and $fixed) { 'pass' } else { 'fail' }; evidence = @{ readonly = $readOnly; fixed = $fixed } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_attachment' -Probe {
    Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path' | Out-Null
    $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
    $detached = -not [bool]$vhd.Attached
    [pscustomobject]@{ status = if ($detached) { 'pass' } else { 'fail' }; evidence = @{ detached = $detached } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_parent' -Probe {
    Get-OrdinaryPathItem -Path $VhdxPath -RequireDirectory $false -Description 'VHDX path' | Out-Null
    $vhd = Get-VHD -Path $VhdxPath -ErrorAction Stop
    $parentless = [string]::IsNullOrWhiteSpace([string]$vhd.ParentPath)
    [pscustomobject]@{ status = if ($parentless) { 'pass' } else { 'fail' }; evidence = @{ parentless = $parentless } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'image_provenance' -Probe {
    if ($null -eq $provenanceValidation) { throw 'provenance validation unavailable' }
    $valid = [bool]$provenanceValidation.valid -and -not [bool]$provenanceValidation.requires_owner_attestation
    [pscustomobject]@{ status = if ($valid) { 'pass' } else { 'fail' }; evidence = @{ valid = $valid; blocker_count = @($provenanceValidation.blockers).Count } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'candidate_hash' -Probe {
    if ($null -eq $provenanceSnapshot) { throw 'provenance snapshot unavailable' }
    $provenance = $provenanceSnapshot.value
    $matches = [string]$provenance.candidate.sha -ceq $CandidateSha
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'package_hash' -Probe {
    if ($null -eq $provenanceSnapshot) { throw 'provenance snapshot unavailable' }
    $provenance = $provenanceSnapshot.value
    $package = Get-OrdinaryPathItem -Path $CandidatePackagePath -RequireDirectory $false -Description 'candidate package path'
    $packageHash = Get-Sha256File -Path $package.FullName
    $matches = [string]$provenance.package.sha256 -ceq $packageHash
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'binary_hash' -Probe {
    if ($null -eq $provenanceSnapshot) { throw 'provenance snapshot unavailable' }
    $provenance = $provenanceSnapshot.value
    $binary = Get-OrdinaryPathItem -Path $CandidateBinaryPath -RequireDirectory $false -Description 'candidate binary path'
    $binaryHash = Get-Sha256File -Path $binary.FullName
    $matches = [string]$provenance.candidate_binary.sha256 -ceq $binaryHash
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'vhdx_hash' -Probe {
    if ($null -eq $provenanceSnapshot) { throw 'provenance snapshot unavailable' }
    $provenance = $provenanceSnapshot.value
    $vhdxHash = Get-Sha256File -Path $VhdxPath
    $matches = [string]$provenance.vhdx.sha256 -ceq $vhdxHash
    [pscustomobject]@{ status = if ($matches) { 'pass' } else { 'fail' }; evidence = @{ matches = $matches } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'admission_receipt' -Probe {
    if ($null -eq $provenanceSnapshot) { throw 'provenance snapshot unavailable' }
    $provenance = $provenanceSnapshot.value
    $issued = [DateTimeOffset]::Parse([string]$provenance.admission_receipt.issued_at_utc)
    $fresh = ([DateTimeOffset]::UtcNow - $issued).TotalHours -ge 0 -and
      ([DateTimeOffset]::UtcNow - $issued).TotalHours -le $MaximumReceiptAgeHours
    [pscustomobject]@{ status = if ($fresh) { 'pass' } else { 'fail' }; evidence = @{ fresh = $fresh } }
  }))
  $result.Add((Invoke-LiveObservation -Code 'exclusive_window' -Probe {
    if ($null -eq $provenanceSnapshot) { throw 'provenance snapshot unavailable' }
    $provenance = $provenanceSnapshot.value
    $eligible = $provenance.exclusive_window.eligible -is [bool] -and
      [bool]$provenance.exclusive_window.eligible -and
      [DateTimeOffset]::Parse([string]$provenance.exclusive_window.ends_at_utc) -gt [DateTimeOffset]::UtcNow
    [pscustomobject]@{ status = if ($eligible) { 'pass' } else { 'fail' }; evidence = @{ eligible = $eligible } }
  }))
  return [pscustomobject]@{
    observations = @($result)
    provenance_validation = $provenanceValidation
  }
}

if ($PSCmdlet.ParameterSetName -eq 'Fixture') {
  $fixture = Convert-FixtureToObservations -Path $FixturePath
  if ($null -eq $fixture) {
    New-InvalidFixtureResult -FailureCode 'fixture.schema_invalid' | ConvertTo-Json -Compress -Depth 12
  } else {
    New-PreflightResult -EvidenceSource 'fixture' -Observations $fixture.observations `
      -EvidenceSourceDigest $fixture.digest -HostObservation $false -ProvenanceValidation $fixture.provenance_validation |
      ConvertTo-Json -Compress -Depth 12
  }
} else {
  $liveEvidence = Get-LiveObservations
  New-PreflightResult -EvidenceSource 'live-read-only' -Observations $liveEvidence.observations `
    -EvidenceSourceDigest (Get-ObservationDigest -Observations $liveEvidence.observations) `
    -HostObservation $true -ProvenanceValidation $liveEvidence.provenance_validation |
    ConvertTo-Json -Compress -Depth 12
}
