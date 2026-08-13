[CmdletBinding()]
param(
  [ValidateSet(
    'V041-R5-HYPERV-PSD-V1',
    'V041-R5-WHPX-QGA-V1',
    'V041-R5-KVM-QGA-V1',
    'V041-R5-JOBSPEC-OVERLAY-V1'
  )]
  [string]$PacketId,

  [ValidateSet('A4', 'A5')]
  [string]$Authority
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$contractPath = Join-Path $repositoryRoot 'docs\receipts\v041-r5-contract-rehearsal.json'
$expectedCandidate = '0e7fcf37f4310562d318f9d5c709ddf8e8ca1637'
$expectedTree = '18c2e81acc4db57e2275175b138d31049df000da'
$packetIds = @(
  'V041-R5-HYPERV-PSD-V1',
  'V041-R5-WHPX-QGA-V1',
  'V041-R5-KVM-QGA-V1',
  'V041-R5-JOBSPEC-OVERLAY-V1'
)
$commonBindings = @(
  'candidate',
  'package',
  'operator_authorization',
  'host',
  'execution_window',
  'writer_exclusivity',
  'immutable_image',
  'guest',
  'exact_owned_namespace',
  'foreign_prestate',
  'lifecycle_and_recovery',
  'cleanup_and_poststate',
  'terminal_result'
)
$expectedTuples = @{
  'V041-R5-HYPERV-PSD-V1' = 'windows|x86_64|hyperv|none|windows|x86_64|powershell-direct'
  'V041-R5-WHPX-QGA-V1' = 'windows|x86_64|qemu|whpx|linux|x86_64|qga'
  'V041-R5-KVM-QGA-V1' = 'linux|x86_64|qemu|kvm|linux|x86_64|qga'
  'V041-R5-JOBSPEC-OVERLAY-V1' = (@('inherited-from-one-exact-base-packet') * 7) -join '|'
}

function Copy-Contract {
  param([Parameter(Mandatory = $true)]$Value)

  return $Value | ConvertTo-Json -Depth 30 | ConvertFrom-Json
}

function Get-TupleKey {
  param([Parameter(Mandatory = $true)]$Tuple)

  return @(
    $Tuple.host_os,
    $Tuple.host_architecture,
    $Tuple.provider,
    $Tuple.accelerator,
    $Tuple.guest_os,
    $Tuple.guest_architecture,
    $Tuple.guest_transport
  ) -join '|'
}

function Assert-Contract {
  param([Parameter(Mandatory = $true)]$Contract)

  if ($Contract.schema_version -ne 1 -or
      $Contract.contract -cne 'vmcell.v041-r5-contract-rehearsal.v1' -or
      $Contract.authorizing -ne $false -or
      $Contract.real_platform_acceptance -cne 'not_executed' -or
      $Contract.dry_run_result -cne 'PASS' -or
      $Contract.r5_result -cne 'NOT_EXECUTED' -or
      $Contract.support_promotion -cne 'not_evaluated') {
    throw 'R5 rehearsal root contract mismatch'
  }
  if ($Contract.candidate.repository -cne 'JerrySkywalker/vm-cell-manager' -or
      $Contract.candidate.release_ref -cne 'release/v0.4.1' -or
      $Contract.candidate.sha -cne $expectedCandidate -or
      $Contract.candidate.tree -cne $expectedTree -or
      $Contract.candidate.version -cne '0.4.1' -or
      $Contract.candidate.immutable -ne $true) {
    throw 'R5 rehearsal candidate binding mismatch'
  }
  foreach ($field in @(
    'archive_name',
    'archive_sha256',
    'checksum_manifest_sha256',
    'binary_sha256',
    'source_commit',
    'source_date_epoch',
    'target_triple',
    'rustc_version',
    'cargo_version'
  )) {
    if ($Contract.package_binding_fields -cnotcontains $field) {
      throw "R5 rehearsal omitted package binding: $field"
    }
  }
  if (@($Contract.packets).Count -ne 4 -or
      @($Contract.packets.packet_id | Sort-Object -Unique).Count -ne 4) {
    throw 'R5 rehearsal must contain exactly four unique packets'
  }
  if ((@($Contract.packets.packet_id | Sort-Object) -join '|') -cne
      (@($packetIds | Sort-Object) -join '|')) {
    throw 'R5 rehearsal packet register mismatch'
  }

  foreach ($packet in $Contract.packets) {
    if ($packet.packet_status -cne 'NOT_EXECUTED' -or
        $packet.support_status -cne 'untested') {
      throw "R5 packet $($packet.packet_id) promoted execution or support"
    }
    if ((Get-TupleKey $packet.tuple) -cne $expectedTuples[$packet.packet_id]) {
      throw "R5 packet $($packet.packet_id) crossed its exact tuple"
    }
    if (@($packet.minimum_dedicated_host_prerequisites).Count -lt 7) {
      throw "R5 packet $($packet.packet_id) lacks minimum host prerequisites"
    }
    foreach ($binding in $commonBindings) {
      if ($packet.required_bindings -cnotcontains $binding) {
        throw "R5 packet $($packet.packet_id) omitted common binding: $binding"
      }
    }
    if ($packet.a4.authority -cne 'A4' -or
        $packet.a4.profile -cne 'PROTECTED_PREFLIGHT_V2' -or
        $packet.a4.mode -cne 'observe-only-preflight' -or
        $packet.a4.result_ceiling -cne 'PREFLIGHT_PASS' -or
        $packet.a4.provider_or_guest_execution -ne $false -or
        -not $packet.a4.one_command.Contains($packet.packet_id, [StringComparison]::Ordinal) -or
        -not $packet.a4.one_command.EndsWith('-Authority A4', [StringComparison]::Ordinal)) {
      throw "R5 packet $($packet.packet_id) has an invalid A4 handoff"
    }
    if ($packet.a5.authority -cne 'A5' -or
        $packet.a5.profile -cne 'PROTECTED_TRANSACTION_V2' -or
        $packet.a5.mode -cne 'authorized-real-run' -or
        $packet.a5.result_ceiling -cne 'PASS' -or
        $packet.a5.fresh_owner_goal_required -ne $true -or
        -not $packet.a5.one_command.Contains($packet.packet_id, [StringComparison]::Ordinal) -or
        -not $packet.a5.one_command.EndsWith('-Authority A5', [StringComparison]::Ordinal)) {
      throw "R5 packet $($packet.packet_id) has an invalid A5 handoff"
    }
  }

  $overlay = @($Contract.packets | Where-Object packet_id -eq 'V041-R5-JOBSPEC-OVERLAY-V1')
  foreach ($required in @(
    'exact_base_packet_id_and_sha256',
    'base_packet_terminal_result_PASS',
    'same_authorized_window',
    'job_spec_source_sha256',
    'two_fresh_execution_identity_sets',
    'job_result_and_operation_correlation'
  )) {
    if ($overlay.required_bindings -cnotcontains $required) {
      throw "JobSpec overlay omitted binding: $required"
    }
  }

  $truth = @{}
  foreach ($row in $Contract.terminal_truth_table) {
    $truth[$row.result] = $row
    if ($row.support_status -cne 'untested') {
      throw "terminal result $($row.result) promoted support"
    }
  }
  if (@($truth.Keys).Count -ne 5 -or
      $truth.PREFLIGHT_PASS.mode -cne 'observe-only-preflight' -or
      $truth.PREFLIGHT_PASS.real_platform_acceptance -cne 'pending' -or
      $truth.PASS.mode -cne 'authorized-real-run' -or
      $truth.PASS.real_platform_acceptance -cne 'completed' -or
      $truth.PARTIAL.real_platform_acceptance -cne 'pending' -or
      $truth.BLOCKED_EXTERNAL.real_platform_acceptance -cne 'pending' -or
      $truth.OWNER_DECISION_REQUIRED.real_platform_acceptance -cne 'pending') {
    throw 'R5 terminal truth table mismatch'
  }

  $serialized = $Contract | ConvertTo-Json -Depth 30 -Compress
  if ($serialized -match '(?i)"(?:password|credential|guest_output|command_argv)"\s*:') {
    throw 'R5 rehearsal contains a forbidden disclosure field'
  }
  foreach ($promotedStatus in @('supported', 'experimental')) {
    if ($serialized -match ('(?i)"support_status"\s*:\s*"' + $promotedStatus + '"')) {
      throw "R5 rehearsal promotes support status: $promotedStatus"
    }
  }
}

function Assert-RejectedMutation {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)]$Mutation
  )

  $rejected = $false
  try {
    Assert-Contract $Mutation
  } catch {
    $rejected = $true
  }
  if (-not $rejected) {
    throw "adversarial R5 mutation was accepted: $Name"
  }
}

$contract = Get-Content -LiteralPath $contractPath -Raw | ConvertFrom-Json
Assert-Contract $contract

$candidateDrift = Copy-Contract $contract
$candidateDrift.candidate.sha = 'ffffffffffffffffffffffffffffffffffffffff'
Assert-RejectedMutation -Name 'candidate drift' -Mutation $candidateDrift

$crossTuple = Copy-Contract $contract
$crossTuple.packets[1].tuple.accelerator = 'kvm'
Assert-RejectedMutation -Name 'cross-tuple substitution' -Mutation $crossTuple

$preflightPromotion = Copy-Contract $contract
$preflightPromotion.packets[0].a4.result_ceiling = 'PASS'
Assert-RejectedMutation -Name 'preflight relabelled PASS' -Mutation $preflightPromotion

$supportPromotion = Copy-Contract $contract
$supportPromotion.packets[2].support_status = 'experimental'
Assert-RejectedMutation -Name 'support promotion' -Mutation $supportPromotion

$overlayWithoutPass = Copy-Contract $contract
$overlayWithoutPass.packets[3].required_bindings = @(
  $overlayWithoutPass.packets[3].required_bindings |
    Where-Object { $_ -cne 'base_packet_terminal_result_PASS' }
)
Assert-RejectedMutation -Name 'overlay without exact PASS base' -Mutation $overlayWithoutPass

$authorityCross = Copy-Contract $contract
$authorityCross.packets[0].a5.authority = 'A4'
Assert-RejectedMutation -Name 'A4 substituted for A5' -Mutation $authorityCross

if ($PacketId) {
  if (-not $Authority) {
    throw '-Authority A4 or A5 is required with -PacketId'
  }
  $packet = @($contract.packets | Where-Object packet_id -eq $PacketId)
  $handoff = if ($Authority -ceq 'A4') { $packet.a4 } else { $packet.a5 }
  [ordered]@{
    schema_version = 1
    contract = 'vmcell.v041-r5-owner-handoff.v1'
    authorizing = $false
    packet_id = $packet.packet_id
    candidate = $contract.candidate
    requested_authority = $handoff.authority
    required_profile = $handoff.profile
    mode = $handoff.mode
    result_ceiling = $handoff.result_ceiling
    packet_status = $packet.packet_status
    support_status = $packet.support_status
    minimum_dedicated_host_prerequisites = $packet.minimum_dedicated_host_prerequisites
    required_bindings = $packet.required_bindings
    next_process_required = $true
    current_process_must_not_contact_host = $true
    current_process_must_not_execute_provider_or_guest = $true
  } | ConvertTo-Json -Depth 12
} else {
  Write-Host 'v0.4.1 R5 four-packet contract and adversarial rehearsal passed'
}
