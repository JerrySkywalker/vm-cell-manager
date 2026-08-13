$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$providerPath = Join-Path $repositoryRoot 'src\providers\qemu\mod.rs'
$jobPath = Join-Path $repositoryRoot 'src\providers\qemu\windows_job.rs'
$adrPath = Join-Path $repositoryRoot 'docs\adr\0017-atomic-windows-qemu-job-containment.md'
$walkthroughPath = Join-Path $repositoryRoot 'docs\windows-qemu-whpx.md'
$receiptPath = Join-Path $repositoryRoot 'docs\receipts\windows-whpx-acceptance-template.json'

foreach ($path in @($providerPath, $jobPath, $adrPath, $walkthroughPath, $receiptPath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Windows Job containment surface is missing: $path"
  }
}

$provider = [IO.File]::ReadAllText($providerPath)
$job = [IO.File]::ReadAllText($jobPath)
$adr = [IO.File]::ReadAllText($adrPath)
$walkthrough = [IO.File]::ReadAllText($walkthroughPath)
$receipt = [IO.File]::ReadAllText($receiptPath) | ConvertFrom-Json

function Assert-WindowsJobContainment {
  param(
    [Parameter(Mandatory)] [string] $Provider,
    [Parameter(Mandatory)] [string] $Job,
    [Parameter(Mandatory)] [string] $Adr,
    [Parameter(Mandatory)] [string] $Walkthrough,
    [Parameter(Mandatory)] $Receipt
  )

  $providerRequired = [ordered]@{
    'schema 2' = 'const QEMU_CONFIG_SCHEMA: u32 = 2;'
    'legacy schema reader' = 'const QEMU_LEGACY_CONFIG_SCHEMA: u32 = 1;'
    'durable Job field' = 'process_job_name: Option<String>'
    'durable command receipt' = 'command_sha256: String'
    'pinned executable' = 'pinned_ordinary_file_sha256'
    'write/delete denial' = 'QemuFileSharePolicy::DenyWriteAndDelete'
    'intent before spawn' = '(?s)process_job_name = new_qemu_job_name\(\).*spawn_pending = true.*replace_config.*spawn_vm'
    'receipt before activation' = '(?s)process_executable_sha256 = Some.*spawn_pending = false.*replace_config.*activate_vm'
    'receipt keyed tree proof' = 'process_tree_absence_proven\('
    'crash recovery observation' = 'process_tree_owned_nonempty\('
    'legacy failure clamp' = 'windows_legacy_receipt_is_fail_closed_without_job_containment'
  }
  foreach ($entry in $providerRequired.GetEnumerator()) {
    if ($Provider -notmatch $entry.Value) {
      throw "Windows Job provider contract lacks $($entry.Key)"
    }
  }

  $jobRequired = [ordered]@{
    'atomic Job list' = 'PROC_THREAD_ATTRIBUTE_JOB_LIST'
    'suspended creation' = 'CREATE_SUSPENDED'
    'resume after receipt' = 'ResumeThread'
    'pre-persistence kill guard' = 'JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE'
    'named collision rejection' = 'ERROR_ALREADY_EXISTS'
    'query-only inherited receipt' = 'OpenJobObjectW\(JOB_OBJECT_QUERY, 1'
    'exact Job membership' = 'IsProcessInJob'
    'accounting empty proof' = 'JobObjectBasicAccountingInformation'
    'PID-list empty proof' = 'JobObjectBasicProcessIdList'
    'exact owned termination' = 'terminate_exact_job'
    'command buffer binding' = 'Windows QEMU command buffer drifted from its durable launch digest'
    'tri-state Job observation' = 'ExactJobObservation::NonEmpty'
    'no QEMU test process' = 'current_exe\(\)'
    'descendant regression' = 'exited_leader_does_not_hide_a_live_descendant'
    'collision regression' = 'job_name_collision_is_rejected'
  }
  foreach ($entry in $jobRequired.GetEnumerator()) {
    if ($Job -notmatch $entry.Value) {
      throw "Windows Job implementation contract lacks $($entry.Key)"
    }
  }

  if ($Job -match 'AssignProcessToJobObject') {
    throw 'Windows QEMU launch must not fall back to post-spawn Job assignment'
  }
  if ($Provider -notmatch '(?s)validate_live_process_receipt.*execute\("quit".*terminate_process_tree') {
    throw 'Windows QEMU stop must validate the live receipt and prefer graceful quit before exact Job termination'
  }
  if ($Job -match 'PROC_THREAD_ATTRIBUTE_PARENT_PROCESS' -or
      $Provider -match 'run_windows_job_internal_mode_from_env') {
    throw 'Windows QEMU launch must not expose an alternate-parent broker or internal environment mode'
  }
  if ($Adr -notmatch 'No QEMU instruction runs outside\s+containment' -or
      $Adr -notmatch 'does not establish WHPX') {
    throw 'Windows Job ADR lacks atomic or evidence boundary semantics'
  }
  if ($Walkthrough -notmatch 'Legacy schema-1 Windows receipts are readable but never infer Job' -or
      $Walkthrough -notmatch 'repository fixtures establish\s+the containment mechanism, not WHPX or real-platform acceptance') {
    throw 'Windows QEMU walkthrough lacks compatibility or non-acceptance boundary'
  }
  if ($Receipt.ownership.windows_job.config_schema -ne 2 -or
      $Receipt.ownership.windows_job.atomic_create_method -cne
        'PROC_THREAD_ATTRIBUTE_JOB_LIST_CREATE_SUSPENDED' -or
      $Receipt.ownership.windows_job.terminal_active_processes -cne 'REQUIRED_ZERO' -or
      $Receipt.ownership.windows_job.terminal_process_id_count -cne 'REQUIRED_ZERO') {
    throw 'Windows WHPX packet lacks structured Job/empty-tree requirements'
  }
}

function Assert-RejectedWindowsJobMutation {
  param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $Provider,
    [Parameter(Mandatory)] [string] $Job
  )
  try {
    Assert-WindowsJobContainment `
      -Provider $Provider -Job $Job -Adr $adr -Walkthrough $walkthrough -Receipt $receipt
  } catch {
    return
  }
  throw "Windows Job containment negative regression was accepted: $Name"
}

Assert-WindowsJobContainment `
  -Provider $provider -Job $job -Adr $adr -Walkthrough $walkthrough -Receipt $receipt
Assert-RejectedWindowsJobMutation -Name 'active creation' -Provider $provider `
  -Job ($job -replace 'CREATE_SUSPENDED', 'CREATE_UNSUSPENDED')
Assert-RejectedWindowsJobMutation -Name 'post-spawn assignment' -Provider $provider `
  -Job "$job`nAssignProcessToJobObject(job, process);"
Assert-RejectedWindowsJobMutation -Name 'missing durable field' `
  -Provider ($provider -replace 'process_job_name: Option<String>', 'legacy_group: Option<String>') `
  -Job $job

Write-Host 'Windows QEMU Job containment contract passed'
