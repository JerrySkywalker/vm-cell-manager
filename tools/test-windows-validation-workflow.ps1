$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workflowPath = Join-Path $repositoryRoot '.github\workflows\windows-validation.yml'
$dispatcherPath = Join-Path $repositoryRoot '.github\workflows\linux-validation.yml'
$selfHostedPath = Join-Path $repositoryRoot '.github\workflows\ci.yml'
$cargoPath = Join-Path $repositoryRoot 'Cargo.toml'

foreach ($path in @($workflowPath, $dispatcherPath, $selfHostedPath, $cargoPath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Windows correctness validation surface is missing: $path"
  }
}

$workflow = [IO.File]::ReadAllText($workflowPath)
$dispatcher = [IO.File]::ReadAllText($dispatcherPath)
$selfHosted = [IO.File]::ReadAllText($selfHostedPath)
$cargo = [IO.File]::ReadAllText($cargoPath)

function Assert-WindowsValidationContract {
  param(
    [Parameter(Mandatory)] [string] $Workflow,
    [Parameter(Mandatory)] [string] $Dispatcher,
    [Parameter(Mandatory)] [string] $SelfHosted,
    [Parameter(Mandatory)] [string] $Cargo
  )

  $required = [ordered]@{
    'manual exact-SHA trigger' = '(?ms)^on:\r?\n  workflow_dispatch:\r?\n    inputs:\r?\n      source_sha:\r?\n        description: [^\r\n]+\r?\n        required: true\r?\n        type: string'
    'same-commit reusable trigger' = '(?ms)^  workflow_call:\r?\n    inputs:\r?\n      source_sha:\r?\n        description: [^\r\n]+\r?\n        required: true\r?\n        type: string'
    'read-only repository permission' = '(?ms)^permissions:\r?\n  contents: read\r?$'
    'stable hosted Windows image' = '(?m)^    runs-on: windows-2025\r?$'
    'separate cold-VM correctness timeout' = '(?m)^    timeout-minutes: 45\r?$'
    'canonical repository admission' = '(?m)^    if: github\.repository == ''JerrySkywalker/vm-cell-manager'' && github\.event_name == ''workflow_dispatch''\r?$'
    'exact pinned checkout action' = '(?m)^        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7\.0\.1\r?$'
    'exact checkout input' = '(?m)^          ref: \$\{\{ inputs\.source_sha \}\}\r?$'
    'checkout credential isolation' = '(?m)^          persist-credentials: false\r?$'
    'checkout depth bound' = '(?m)^          fetch-depth: 1\r?$'
    'strict lowercase SHA input' = '-cnotmatch ''\^\[0-9a-f\]\{40\}\$'''
    'caller source binding' = 'SOURCE_SHA -cne \$env:WORKFLOW_SHA'
    'reusable workflow source binding' = 'SOURCE_SHA -cne \$env:JOB_WORKFLOW_SHA'
    'hosted runner proof' = 'RUNNER_ENVIRONMENT -cne ''github-hosted'''
    'Windows runner proof' = 'RUNNER_OS -cne ''Windows'''
    'x64 runner proof' = 'RUNNER_ARCH -cne ''X64'''
    'exact declared toolchain' = '(?m)^      RUSTUP_TOOLCHAIN: 1\.85\.0-x86_64-pc-windows-msvc\r?$'
    'exact rustc identity' = '(?m)^      EXPECTED_RUSTC: rustc 1\.85\.0 \(4d91de4e4 2025-02-17\)\r?$'
    'bounded hosted timing start' = 'VMCELL_HOSTED_STARTED_EPOCH=\$startedAt'
    'ephemeral Cargo home' = 'Join-Path \$env:RUNNER_TEMP ''vmcell-windows-cargo'''
    'ephemeral Rustup home' = 'Join-Path \$env:RUNNER_TEMP ''vmcell-windows-rustup'''
    'external Cargo target' = 'Join-Path \$env:RUNNER_TEMP ''vmcell-windows-target'''
    'locked dependency fetch' = 'cargo fetch --locked'
    'Cargo MSRV source binding' = "rust_version -cne '1\.85\.0'"
    'Cargo package source binding' = 'VMCELL_PACKAGE_VERSION=\$\(\$package\[0\]\.version\)'
    'format gate' = 'cargo fmt --all -- --check'
    'PowerShell safety gate' = '& \.\\tools\\check-powershell\.ps1'
    'Windows workflow contract gate' = '& \.\\tools\\test-windows-validation-workflow\.ps1'
    'Windows fixture preflight gate' = '& \.\\tools\\test-windows-whpx-preflight\.ps1'
    'Linux workflow contract gate' = '& \.\\tools\\test-linux-validation-workflow\.ps1'
    'R3 workflow contract gate' = '& \.\\tools\\test-linux-reliability-workflow\.ps1'
    'reliability closeout contract gate' = '& \.\\tools\\test-reliability-closeout\.ps1'
    'locked offline static check' = 'cargo check --locked --offline --workspace --all-targets --all-features'
    'locked offline Clippy' = 'cargo clippy --locked --offline --workspace --all-targets --all-features -- -D warnings'
    'locked offline full tests' = 'cargo test --locked --offline --workspace --all-targets --all-features'
    'locked offline doc tests' = 'cargo test --locked --offline --workspace --all-features --doc'
    'locked offline package build' = 'cargo build --locked --offline --release --bin vmcell'
    'package contract test' = '& \.\\tools\\test-windows-package\.ps1'
    'post-validation clean diff' = 'git diff --exit-code'
    'bounded sanitized receipt' = 'vmcell\.windows-correctness-receipt\.v1'
    'bounded sanitized timing' = 'elapsed_seconds=\$elapsed'
    'bounded hosted duration' = '\[Math\]::Min\(\[Math\]::Max\(\$elapsed, 0\), 2700\)'
    'repository-only meaning' = 'repository_correctness_claim=false r4_runner_health=false'
    'no real-platform or support meaning' = 'real_platform_acceptance=false support_promotion=false'
    'single-lane selector' = '(?ms)^      lane:\r?\n        description: [^\r\n]+\r?\n        required: true\r?\n        default: linux\r?\n        type: choice\r?\n        options:\r?\n          - linux\r?\n          - windows\r?\n          - reliability\r?$'
    'same-commit Windows dispatcher bridge' = '(?ms)^  windows-correctness:\r?\n    name: [^\r\n]+\r?\n    if: inputs\.lane == ''windows''\r?\n    uses: \./\.github/workflows/windows-validation\.yml\r?\n    with:\r?\n      source_sha: \$\{\{ inputs\.source_sha \}\}\r?$'
    'same-commit R3 dispatcher bridge' = '(?ms)^  linux-reliability:\r?\n    name: [^\r\n]+\r?\n    if: inputs\.lane == ''reliability''\r?\n    uses: \./\.github/workflows/linux-reliability\.yml\r?\n    with:\r?\n      source_sha: \$\{\{ inputs\.source_sha \}\}\r?$'
    'immutable self-hosted R4 timeout' = '(?ms)^  windows-core:\r?\n.*?^    timeout-minutes: 30\r?$'
    'declared package MSRV' = '(?m)^rust-version = "1\.85\.0"\r?$'
    'declared package version' = '(?m)^version = "0\.4\.0"\r?$'
  }
  $contract = "$Workflow`n$Dispatcher`n$SelfHosted`n$Cargo"
  foreach ($entry in $required.GetEnumerator()) {
    if ($contract -notmatch $entry.Value) {
      throw "Windows validation contract lacks $($entry.Key)"
    }
  }

  $actions = @([regex]::Matches($Workflow, '(?m)^\s*uses:\s*([^\s]+)') |
    ForEach-Object { $_.Groups[1].Value })
  if ($actions.Count -ne 1 -or
      $actions[0] -ne 'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1') {
    throw 'Windows validation must use only the pinned checkout action'
  }

  $workflowForbidden = [ordered]@{
    'automatic or external trigger' = '(?i)\b(?:push|pull_request|pull_request_target|schedule|repository_dispatch)\s*:'
    'write or identity-token permission' = '(?im)^\s*(?:[A-Za-z-]+\s*:\s*write|id-token\s*:)'
    'secret or explicit token expression' = '\$\{\{\s*(?:secrets\.|github\.token)'
    'cache or artifact action' = '(?i)actions/(?:cache|upload-artifact|download-artifact)@'
    'protected environment' = '(?im)^\s*environment\s*:'
    'self-hosted runner' = '(?im)^\s*runs-on:\s*self-hosted\s*$'
    'provider or guest lifecycle' = '(?i)\b(?:vmcell\s+(?:run|exec|copy-in|copy-out|destroy|gc)|qemu-system|New-VM|Start-VM|Stop-VM|Remove-VM)\b'
    'publication command' = '(?i)\b(?:cargo publish|gh release|wingetcreate|scoop bucket)\b'
  }
  foreach ($entry in $workflowForbidden.GetEnumerator()) {
    if ("$Workflow`n$Dispatcher" -match $entry.Value) {
      throw "Windows validation contains forbidden $($entry.Key)"
    }
  }
  if ($Dispatcher -match '(?im)^\s*secrets\s*:') {
    throw 'dispatcher must not pass secrets to reusable correctness jobs'
  }
}

function Assert-RejectedWindowsMutation {
  param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $Workflow,
    [Parameter(Mandatory)] [string] $Dispatcher,
    [Parameter(Mandatory)] [string] $SelfHosted
  )
  try {
    Assert-WindowsValidationContract -Workflow $Workflow -Dispatcher $Dispatcher `
      -SelfHosted $SelfHosted -Cargo $cargo
  } catch {
    return
  }
  throw "Windows validation negative regression was accepted: $Name"
}

Assert-WindowsValidationContract -Workflow $workflow -Dispatcher $dispatcher `
  -SelfHosted $selfHosted -Cargo $cargo

Assert-RejectedWindowsMutation -Name 'automatic pull request trigger' `
  -Workflow ($workflow -replace '  workflow_call:', "  pull_request_target: {}`n  workflow_call:") `
  -Dispatcher $dispatcher -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'write permission' `
  -Workflow ($workflow -replace 'contents: read', 'contents: write') `
  -Dispatcher $dispatcher -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'secret expression' `
  -Workflow "$workflow`n      TOKEN: `${{ secrets.UNSAFE }}" `
  -Dispatcher $dispatcher -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'cache action' `
  -Workflow "$workflow`n      - uses: actions/cache@v4" `
  -Dispatcher $dispatcher -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'self-hosted correctness runner' `
  -Workflow ($workflow -replace 'runs-on: windows-2025', 'runs-on: self-hosted') `
  -Dispatcher $dispatcher -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'hosted timeout drift' `
  -Workflow ($workflow -replace 'timeout-minutes: 45', 'timeout-minutes: 90') `
  -Dispatcher $dispatcher -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'removed doc tests' `
  -Workflow ($workflow -replace 'cargo test --locked --offline --workspace --all-features --doc', 'cargo test --locked --workspace') `
  -Dispatcher $dispatcher -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'secret inheritance bridge' `
  -Workflow $workflow -Dispatcher "$dispatcher`n    secrets: inherit" -SelfHosted $selfHosted
Assert-RejectedWindowsMutation -Name 'R4 timeout inflation' `
  -Workflow $workflow -Dispatcher $dispatcher `
  -SelfHosted ($selfHosted -replace 'timeout-minutes: 30', 'timeout-minutes: 31')

Write-Host 'Windows hosted correctness workflow safety contract passed'
