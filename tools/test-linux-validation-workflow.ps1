$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workflowPath = Join-Path $repositoryRoot '.github\workflows\linux-validation.yml'
$gatePath = Join-Path $repositoryRoot 'tools\check-linux.sh'

if (-not (Test-Path -LiteralPath $workflowPath -PathType Leaf)) {
  throw 'Linux validation workflow is missing'
}
if (-not (Test-Path -LiteralPath $gatePath -PathType Leaf)) {
  throw 'Linux repository gate is missing'
}

$workflow = [IO.File]::ReadAllText($workflowPath)
$gate = [IO.File]::ReadAllText($gatePath)

function Assert-LinuxValidationContract {
  param(
    [Parameter(Mandatory)] [string] $Workflow,
    [Parameter(Mandatory)] [string] $Gate
  )

  $contract = "$Workflow`n$Gate"
  $requiredPatterns = [ordered]@{
    'dispatch-only trigger' = '(?ms)^on:\r?\n  workflow_dispatch:\r?\n    inputs:\r?\n      source_sha:\r?\n        description: [^\r\n]+\r?\n        required: true\r?\n        type: string\r?\n\r?\npermissions:'
    'single trigger declaration' = '(?m)^on:\r?$'
    'read-only repository permission' = '(?ms)^permissions:\r?\n  contents: read\r?\n\r?\nconcurrency:'
    'pinned hosted baseline' = '(?m)^    runs-on: ubuntu-24\.04\r?$'
    'Rust 1.85 toolchain' = '(?m)^      RUSTUP_TOOLCHAIN: 1\.85\.0\r?$'
    'bounded Cargo target' = '(?m)^      CARGO_TARGET_DIR: \$\{\{ runner\.temp \}\}/vmcell-cargo-target\r?$'
    'exact checkout binding' = '(?m)^          ref: \$\{\{ inputs\.source_sha \}\}\r?$'
    'checkout credential isolation' = '(?m)^          persist-credentials: false\r?$'
    'exact SHA proof' = 'test "\$actual_sha" = "\$SOURCE_SHA"'
    'hosted runner proof' = 'test "\$\{RUNNER_ENVIRONMENT:-\}" = ''github-hosted'''
    'external target proof' = 'test "\$CARGO_TARGET_DIR" = "\$RUNNER_TEMP/vmcell-cargo-target"'
    'locked metadata gate' = 'cargo metadata --locked'
    'locked check gate' = 'cargo check --locked (?:--workspace )?--all-targets --all-features'
    'locked Clippy gate' = 'cargo clippy --locked (?:--workspace )?--all-targets --all-features -- -D warnings'
    'locked test gate' = 'cargo test --locked (?:--workspace )?--all-targets --all-features'
    'locked doc-test gate' = 'cargo test --locked (?:--workspace )?--all-features --doc'
    'post-gate clean-tree proof' = 'git diff --exit-code'
    'no ignored checkout target proof' = 'test ! -e "\$GITHUB_WORKSPACE/target"'
  }

  foreach ($entry in $requiredPatterns.GetEnumerator()) {
    if ($contract -notmatch $entry.Value) {
      throw "Linux validation contract lacks $($entry.Key)"
    }
  }

  if (@([regex]::Matches($Workflow, '(?m)^on:\r?$')).Count -ne 1) {
    throw 'Linux validation workflow must have exactly one trigger declaration'
  }
  if (@([regex]::Matches($Workflow, '(?m)^permissions:\r?$')).Count -ne 1) {
    throw 'Linux validation workflow must have exactly one permission declaration'
  }

  $workflowForbidden = [ordered]@{
    'automatic or external trigger' = '(?i)\b(?:push|pull_request|pull_request_target|schedule|repository_dispatch)\s*:'
    'write permission' = '(?i)\b[A-Za-z-]+\s*:\s*write\b'
  }
  foreach ($entry in $workflowForbidden.GetEnumerator()) {
    if ($Workflow -match $entry.Value) {
      throw "Linux validation workflow contains forbidden $($entry.Key)"
    }
  }

  $executionForbidden = [ordered]@{
    'privileged command' = '(?m)^\s*(?:sudo|su)\s+'
    'host package mutation' = '(?m)^\s*(?:apt|apt-get|dnf|yum|pacman|zypper)\s+'
    'KVM device mutation' = '(?i)(?:(?:>|>>)\s*/dev/kvm\b|\b(?:chmod|chown|chgrp|setfacl|rm|mv|mknod)\b[^\r\n]*/dev/kvm\b)'
    'kernel module mutation' = '(?im)^\s*(?:modprobe|insmod|rmmod)\s+'
    'QEMU lifecycle command' = '(?i)\bqemu-system(?:-[A-Za-z0-9_]+)?\b'
    'vmcell lifecycle command' = '(?i)\bvmcell\s+(?:run|exec|copy-in|copy-out|destroy|gc)\b'
  }
  foreach ($entry in $executionForbidden.GetEnumerator()) {
    if ($contract -match $entry.Value) {
      throw "Linux validation execution surface contains forbidden $($entry.Key)"
    }
  }

  $gateCommands = @($Gate -split '\r?\n' | Where-Object {
    $_ -notmatch '^\s*(?:#|$)' -and $_ -notmatch '^\s*set\s+-eu\s*$'
  })
  $allowedGateCommands = @(
    '^rustc --version$',
    '^cargo --version$',
    '^cargo metadata --locked --offline --all-features --format-version 1 >/dev/null$',
    '^cargo fmt --all -- --check$',
    '^cargo check --locked --workspace --all-targets --all-features$',
    '^cargo clippy --locked --workspace --all-targets --all-features -- -D warnings$',
    '^cargo test --locked --workspace --all-targets --all-features$',
    '^cargo test --locked --workspace --all-features --doc$'
  )
  foreach ($command in $gateCommands) {
    if (-not @($allowedGateCommands | Where-Object { $command -match $_ })) {
      throw "Linux repository gate contains an unapproved command: $command"
    }
  }
}

function Assert-RejectedMutation {
  param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $Workflow,
    [Parameter(Mandatory)] [string] $Gate
  )

  try {
    Assert-LinuxValidationContract -Workflow $Workflow -Gate $Gate
  } catch {
    return
  }
  throw "Linux validation negative regression was accepted: $Name"
}

Assert-LinuxValidationContract -Workflow $workflow -Gate $gate

Assert-RejectedMutation -Name 'automatic push' `
  -Workflow ($workflow -replace '  workflow_dispatch:', '  push: {}') -Gate $gate
Assert-RejectedMutation -Name 'inline pull request trigger' `
  -Workflow ($workflow -replace '  workflow_dispatch:', "  workflow_dispatch:`n  pull_request: {}") -Gate $gate
Assert-RejectedMutation -Name 'write permission' `
  -Workflow ($workflow -replace 'contents: read', 'contents: write') -Gate $gate
Assert-RejectedMutation -Name 'privileged gate command' `
  -Workflow $workflow -Gate "$gate`nsudo true"
Assert-RejectedMutation -Name 'host package mutation' `
  -Workflow $workflow -Gate "$gate`napt-get install qemu"
Assert-RejectedMutation -Name 'KVM device mutation' `
  -Workflow $workflow -Gate "$gate`nchmod 0666 /dev/kvm"
Assert-RejectedMutation -Name 'kernel module mutation' `
  -Workflow $workflow -Gate "$gate`nmodprobe kvm"
Assert-RejectedMutation -Name 'QEMU lifecycle' `
  -Workflow $workflow -Gate "$gate`nqemu-system-x86_64 --version"
Assert-RejectedMutation -Name 'vmcell lifecycle' `
  -Workflow $workflow -Gate "$gate`nvmcell run --image test -- true"

Write-Host 'Linux validation workflow safety contract passed'
