$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workflowPath = Join-Path $repositoryRoot '.github\workflows\linux-reliability.yml'
$campaignPath = Join-Path $repositoryRoot 'tools\reliability-campaign.json'
$campaignScriptPath = Join-Path $repositoryRoot 'tools\check-reliability-campaign.sh'
$campaignTestPath = Join-Path $repositoryRoot 'tools\test-reliability-campaign.sh'

foreach ($path in @($workflowPath, $campaignPath, $campaignScriptPath, $campaignTestPath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Linux reliability validation surface is missing: $path"
  }
}

$workflow = [IO.File]::ReadAllText($workflowPath)
$campaignScript = [IO.File]::ReadAllText($campaignScriptPath)
$campaignTest = [IO.File]::ReadAllText($campaignTestPath)
$campaignSurface = "$campaignScript`n$campaignTest"
$campaign = [IO.File]::ReadAllText($campaignPath) | ConvertFrom-Json

function Assert-LinuxReliabilityContract {
  param(
    [Parameter(Mandatory)] [string] $Workflow,
    [Parameter(Mandatory)] [string] $CampaignScript
  )

  $requiredWorkflow = [ordered]@{
    'manual and same-commit reusable triggers' = '(?ms)^on:\r?\n  workflow_dispatch:\r?\n    inputs:\r?\n      source_sha:\r?\n        description: [^\r\n]+\r?\n        required: true\r?\n        type: string\r?\n  workflow_call:\r?\n    inputs:\r?\n      source_sha:\r?\n        description: [^\r\n]+\r?\n        required: true\r?\n        type: string\r?\n\r?\npermissions:'
    'read-only repository permission' = '(?ms)^permissions:\r?\n  contents: read\r?\n\r?\nconcurrency:'
    'pinned hosted baseline' = '(?m)^    runs-on: ubuntu-24\.04\r?$'
    'canonical repository manual-job admission' = '(?m)^    if: github\.repository == ''JerrySkywalker/vm-cell-manager'' && github\.event_name == ''workflow_dispatch''\r?$'
    'bounded workflow duration' = '(?m)^    timeout-minutes: 15\r?$'
    'Rust 1.85 toolchain' = '(?m)^      RUSTUP_TOOLCHAIN: 1\.85\.0\r?$'
    'ephemeral Cargo home binding' = 'cargo_home="\$RUNNER_TEMP/vmcell-reliability-cargo"'
    'ephemeral Rustup home binding' = 'rustup_home="\$RUNNER_TEMP/vmcell-reliability-rustup"'
    'ephemeral Cargo target binding' = 'cargo_target="\$RUNNER_TEMP/vmcell-reliability-target"'
    'ephemeral environment propagation' = "printf 'CARGO_HOME=%s\\nRUSTUP_HOME=%s\\nCARGO_TARGET_DIR=%s\\n'"
    'exact checkout binding' = '(?m)^          ref: \$\{\{ inputs\.source_sha \}\}\r?$'
    'checkout credential isolation' = '(?m)^          persist-credentials: false\r?$'
    'checkout depth bound' = '(?m)^          fetch-depth: 1\r?$'
    'exact SHA proof' = 'test "\$actual_sha" = "\$SOURCE_SHA"'
    'hosted runner proof' = 'test "\$\{RUNNER_ENVIRONMENT:-\}" = ''github-hosted'''
    'canonical repository proof' = 'test "\$\{GITHUB_REPOSITORY:-\}" = ''JerrySkywalker/vm-cell-manager'''
    'manual dispatch proof' = 'test "\$\{GITHUB_EVENT_NAME:-\}" = ''workflow_dispatch'''
    'runner OS proof' = 'test "\$\{RUNNER_OS:-\}" = ''Linux'''
    'runner architecture proof' = 'test "\$\{RUNNER_ARCH:-\}" = ''X64'''
    'Linux x86_64 proof' = 'test "\$\(uname -m\)" = ''x86_64'''
    'Ubuntu 24.04 proof' = 'test "\$VERSION_ID" = ''24\.04'''
    'exact compiler identity' = 'test "\$\(rustc --version\)" = "\$EXPECTED_RUSTC"'
    'locked dependency fetch' = 'cargo fetch --locked'
    'bounded-capture fixture' = 'sh tools/test-reliability-campaign\.sh'
    'fixed campaign entry point' = 'sh tools/check-reliability-campaign\.sh'
    'hard campaign deadline' = 'timeout --kill-after=1s 600s sh tools/check-reliability-campaign\.sh'
    'fixed campaign manifest' = '--manifest tools/reliability-campaign\.json'
    'campaign exact source binding' = '--source-sha "\$SOURCE_SHA"'
    'campaign ephemeral receipt' = '--receipt "\$RUNNER_TEMP/vmcell-reliability-receipt\.json"'
    'post-campaign clean tree proof' = 'git diff --exit-code'
    'no checkout target proof' = 'test ! -e "\$GITHUB_WORKSPACE/target"'
  }

  foreach ($entry in $requiredWorkflow.GetEnumerator()) {
    if ($Workflow -notmatch $entry.Value) {
      throw "Linux reliability workflow lacks $($entry.Key)"
    }
  }

  if (@([regex]::Matches($Workflow, '(?m)^on:\r?$')).Count -ne 1) {
    throw 'Linux reliability workflow must have exactly one trigger declaration'
  }
  if (@([regex]::Matches($Workflow, '(?m)^permissions:\r?$')).Count -ne 1) {
    throw 'Linux reliability workflow must have exactly one permission declaration'
  }
  if ($Workflow -match '\$\{\{\s*runner\.') {
    throw 'Linux reliability workflow must not use runner context outside a runtime step'
  }
  $actions = @([regex]::Matches($Workflow, '(?m)^\s*uses:\s*([^\s]+)') | ForEach-Object {
    $_.Groups[1].Value
  })
  if ($actions.Count -ne 1 -or $actions[0] -ne 'actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803') {
    throw 'Linux reliability workflow must use only the pinned checkout action'
  }

  $requiredScript = [ordered]@{
    'fixed campaign contract' = '"contract": "vmcell\.reliability-campaign\.v1"'
    'strict manifest duplicate-key rejection' = 'duplicate JSON key'
    'strict source SHA length' = '\[ "\$\{#source_sha\}" -eq 40 \]'
    'bounded per-case deadline' = 'case_limit=120'
    'bounded campaign deadline' = 'campaign_limit=600'
    'bounded captured test output' = 'capture_limit=1048576'
    'draining bounded capture' = 'while True:'
    'overflow rejection' = 'if total > limit:'
    'private capture pipe' = 'mkfifo -m 600 "\$fifo"'
    'bounded test-process deadline' = 'timeout --kill-after=1s "\$case_limit"s cargo test'
    'executed ignored-test proof' = 'grep -F ''running 1 test'' "\$output" >/dev/null'
    'executed ignored-test result proof' = 'grep -F ''test result: ok\. 1 passed; 0 failed; 0 ignored;'' "\$output" >/dev/null'
    'ignored-test registration proof' = 'cargo test --locked --offline --test "\$target" "\$test_name" -- --list --ignored --exact'
    'exact ignored-test registration count' = 'grep -Fxc "\$test_name: test" "\$output"'
    'sanitized failed-case output' = 'printf ''reliability_case=%s status=failed\\n'' "\$case_index" >&2'
    'external temporary receipt containment' = 'receipt must be inside RUNNER_TEMP'
    'no-replace receipt publication' = 'os\.O_WRONLY \| os\.O_CREAT \| os\.O_EXCL'
    'non-authorizing receipt' = '"authorizing": False'
    'no real-platform acceptance receipt' = '"real_platform_acceptance": False'
    'no support promotion receipt' = '"support_promotion": "not_evaluated"'
    'fixed receipt contract' = '"contract": "vmcell\.reliability-extended-receipt\.v1"'
    'manifest-derived case rows' = 'for index, item in enumerate\(expected\["cases"\], start=1\)'
    'manifest-derived case execution' = 'while IFS="\$\(printf ''\\t''\)" read -r case_index target test_name; do'
    'ignored exact test invocation' = 'cargo test --locked --offline --test "\$target" "\$test_name" -- --ignored --exact'
    'source SHA receipt binding' = '"source_sha": source_sha'
    'manifest receipt binding' = '"manifest_sha256": manifest_sha256'
    'case-count receipt binding' = '"case_count": 5'
    'PASS terminal receipt' = '"result": "PASS"'
    'exact receipt comparison' = 'if value != \{'
  }
  foreach ($entry in $requiredScript.GetEnumerator()) {
    if ($CampaignScript -notmatch $entry.Value) {
      throw "Linux reliability campaign script lacks $($entry.Key)"
    }
  }

  if (@([regex]::Matches($CampaignScript, '(?m)^\s*run_case ')).Count -ne 1) {
    throw 'Linux reliability campaign must invoke cases only through the manifest-derived loop'
  }
  if (@([regex]::Matches($CampaignScript, '(?m)^\s*(?:exec )?timeout\b')).Count -ne 2 -or
      @([regex]::Matches($CampaignScript, '(?m)^\s*(?:exec )?timeout(?:\s+--kill-after=1s)?\s+"\$case_limit"s cargo test ')).Count -ne 2 -or
      @([regex]::Matches($CampaignScript, '(?m)^\s*(?:exec )?cargo test\b')).Count -ne 0 -or
      @([regex]::Matches($CampaignScript, '(?m)\bcargo test\b')).Count -ne 2) {
    throw 'Linux reliability campaign must have exactly the registration and execution test commands'
  }

  $workflowForbidden = [ordered]@{
    'automatic or external trigger' = '(?i)\b(?:push|pull_request|pull_request_target|schedule|repository_dispatch)\s*:'
    'write or identity-token permission' = '(?im)^\s*(?:[A-Za-z-]+\s*:\s*write|id-token\s*:)'
    'secret expression' = '\$\{\{\s*secrets\.'
    'cache or artifact action' = '(?i)actions/(?:cache|upload-artifact|download-artifact)@'
    'protected environment' = '(?im)^\s*environment\s*:'
  }
  foreach ($entry in $workflowForbidden.GetEnumerator()) {
    if ($Workflow -match $entry.Value) {
      throw "Linux reliability workflow contains forbidden $($entry.Key)"
    }
  }

  $executionSurface = "$Workflow`n$CampaignScript"
  $surfaceForbidden = [ordered]@{
    'canonical Linux gate invocation' = '(?i)tools/check-linux\.sh'
    'full workspace test coverage' = '(?i)cargo\s+test\b[^\r\n]*(?:--workspace|--all-targets|--all-features)'
    'check or Clippy gate' = '(?i)cargo\s+(?:check|clippy)\b'
    'package gate' = '(?i)(?:check-linux-package|package-linux|cargo\s+build\s+.*--release)'
    'privileged command' = '(?m)^\s*(?:sudo|su)\s+'
    'host package mutation' = '(?m)^\s*(?:apt|apt-get|dnf|yum|pacman|zypper)\s+'
    'KVM device mutation' = '(?i)(?:(?:>|>>)\s*/dev/kvm\b|\b(?:chmod|chown|chgrp|setfacl|rm|mv|mknod)\b[^\r\n]*/dev/kvm\b)'
    'kernel module mutation' = '(?im)^\s*(?:modprobe|insmod|rmmod)\s+'
    'network command' = '(?im)^\s*(?:curl|wget|ssh|scp|rsync|nc|ncat)\b'
    'container or host-service command' = '(?im)^\s*(?:docker|podman|systemctl|service)\b'
    'QEMU lifecycle command' = '(?i)\bqemu-system(?:-[A-Za-z0-9_]+)?\b'
    'vmcell lifecycle command' = '(?i)\bvmcell\s+(?:run|exec|copy-in|copy-out|destroy|gc)\b'
  }
  foreach ($entry in $surfaceForbidden.GetEnumerator()) {
    if ($executionSurface -match $entry.Value) {
      throw "Linux reliability execution surface contains forbidden $($entry.Key)"
    }
  }
}

function Assert-RejectedReliabilityMutation {
  param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $Workflow,
    [Parameter(Mandatory)] [string] $CampaignScript
  )

  try {
    Assert-LinuxReliabilityContract -Workflow $Workflow -CampaignScript $CampaignScript
  } catch {
    return
  }
  throw "Linux reliability negative regression was accepted: $Name"
}

Assert-LinuxReliabilityContract -Workflow $workflow -CampaignScript $campaignSurface

$expectedCampaignProperties = @(
  'schema_version',
  'contract',
  'campaign_id',
  'rust_toolchain',
  'case_limit',
  'case_timeout_seconds',
  'campaign_timeout_seconds',
  'cases'
)
if ((@($campaign.PSObject.Properties.Name) -join '|') -ne ($expectedCampaignProperties -join '|')) {
  throw 'reliability campaign manifest properties changed unexpectedly'
}
if ($campaign.schema_version -ne 1 -or
    $campaign.contract -ne 'vmcell.reliability-campaign.v1' -or
    $campaign.campaign_id -ne 'r3-fixed-reliability-v1' -or
    $campaign.rust_toolchain -ne '1.85.0' -or
    $campaign.case_limit -ne 5 -or
    $campaign.case_timeout_seconds -ne 120 -or
    $campaign.campaign_timeout_seconds -ne 600 -or
    @($campaign.cases).Count -ne 5) {
  throw 'reliability campaign manifest binding changed unexpectedly'
}

$actualCases = @($campaign.cases | ForEach-Object {
  "$($_.target)|$($_.test)|$($_.seed)"
})
$expectedCases = @(
  'reliability_harness|seeded_lifecycle_cases_are_reproducible_and_disjoint_from_normal_ci|6a09e667f3bcc909',
  'reliability_harness|bounded_minimizer_returns_a_real_rejected_transition_as_serialized_input|6a09e667f3bcc909',
  'reliability_model_matrix|run_selection_matrix_has_stable_outcomes_and_never_implicitly_selects_tcg|fixed-v1',
  'reliability_model_matrix|job_spec_plan_and_result_metadata_bind_provenance_without_authority_or_secrets|fixed-v1',
  'reliability_model_matrix|durable_correlation_schema_fence_is_property_exact|fixed-v1'
)
if (($actualCases -join '|') -ne ($expectedCases -join '|')) {
  throw 'reliability campaign manifest case allowlist changed unexpectedly'
}

Assert-RejectedReliabilityMutation -Name 'automatic push trigger' `
  -Workflow ($workflow -replace '  workflow_dispatch:', '  push: {}') -CampaignScript $campaignSurface
Assert-RejectedReliabilityMutation -Name 'public pull request trigger' `
  -Workflow ($workflow -replace '  workflow_dispatch:', "  workflow_dispatch:`n  pull_request: {}") -CampaignScript $campaignSurface
Assert-RejectedReliabilityMutation -Name 'write permission' `
  -Workflow ($workflow -replace 'contents: read', 'contents: write') -CampaignScript $campaignSurface
Assert-RejectedReliabilityMutation -Name 'secret expression' `
  -Workflow "$workflow`n      TOKEN: `${{ secrets.UNSAFE }}" -CampaignScript $campaignSurface
Assert-RejectedReliabilityMutation -Name 'canonical gate reuse' `
  -Workflow $workflow -CampaignScript "$campaignSurface`ntools/check-linux.sh"
Assert-RejectedReliabilityMutation -Name 'privileged command' `
  -Workflow $workflow -CampaignScript "$campaignSurface`nsudo true"
Assert-RejectedReliabilityMutation -Name 'host package mutation' `
  -Workflow $workflow -CampaignScript "$campaignSurface`napt-get install qemu"
Assert-RejectedReliabilityMutation -Name 'KVM permission repair' `
  -Workflow $workflow -CampaignScript "$campaignSurface`nchmod 0666 /dev/kvm"
Assert-RejectedReliabilityMutation -Name 'QEMU lifecycle' `
  -Workflow $workflow -CampaignScript "$campaignSurface`nqemu-system-x86_64 --version"
Assert-RejectedReliabilityMutation -Name 'vmcell lifecycle' `
  -Workflow $workflow -CampaignScript "$campaignSurface`nvmcell run --image test -- true"
Assert-RejectedReliabilityMutation -Name 'network command' `
  -Workflow $workflow -CampaignScript "$campaignSurface`ncurl https://example.invalid"
Assert-RejectedReliabilityMutation -Name 'extra reliability case' `
  -Workflow $workflow -CampaignScript "$campaignSurface`nrun_case 6 extra extra"
Assert-RejectedReliabilityMutation -Name 'extra direct cargo test' `
  -Workflow $workflow -CampaignScript "$campaignSurface`ntimeout 1s cargo test --test extra -- --ignored --exact"
Assert-RejectedReliabilityMutation -Name 'runner expression context' `
  -Workflow "$workflow`n      CARGO_HOME: `${{ runner.temp }}/unsafe" -CampaignScript $campaignSurface
Assert-RejectedReliabilityMutation -Name 'removed hard campaign deadline' `
  -Workflow ($workflow -replace 'timeout --kill-after=1s 600s sh ', 'sh ') -CampaignScript $campaignSurface

Write-Host 'Linux reliability workflow safety contract passed'
