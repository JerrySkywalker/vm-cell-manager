$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workflowPath = Join-Path $repositoryRoot '.github\workflows\linux-validation.yml'
$gatePath = Join-Path $repositoryRoot 'tools\check-linux.sh'
$preflightPath = Join-Path $repositoryRoot 'tools\linux-kvm-preflight.sh'
$preflightTestPath = Join-Path $repositoryRoot 'tools\test-linux-kvm-preflight.sh'

if (-not (Test-Path -LiteralPath $workflowPath -PathType Leaf)) {
  throw 'Linux validation workflow is missing'
}
if (-not (Test-Path -LiteralPath $gatePath -PathType Leaf)) {
  throw 'Linux repository gate is missing'
}
if (-not (Test-Path -LiteralPath $preflightPath -PathType Leaf)) {
  throw 'Linux KVM preflight harness is missing'
}
if (-not (Test-Path -LiteralPath $preflightTestPath -PathType Leaf)) {
  throw 'Linux KVM preflight fixture test is missing'
}

$workflow = [IO.File]::ReadAllText($workflowPath)
$gate = [IO.File]::ReadAllText($gatePath)
$preflight = [IO.File]::ReadAllText($preflightPath)
$preflightTest = [IO.File]::ReadAllText($preflightTestPath)

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
    'bounded Cargo target' = 'cargo_target="\$RUNNER_TEMP/vmcell-cargo-target"'
    'Cargo target propagation' = 'printf ''CARGO_TARGET_DIR=%s\\n'' "\$cargo_target" >> "\$GITHUB_ENV"'
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
    'Linux preflight syntax gate' = 'sh -n tools/linux-kvm-preflight\.sh'
    'Linux preflight test syntax gate' = 'sh -n tools/test-linux-kvm-preflight\.sh'
    'Linux preflight fixture gate' = 'sh tools/test-linux-kvm-preflight\.sh'
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
    '^cargo test --locked --workspace --all-features --doc$',
    '^sh -n tools/linux-kvm-preflight\.sh$',
    '^sh -n tools/test-linux-kvm-preflight\.sh$',
    '^sh tools/test-linux-kvm-preflight\.sh$'
  )
  foreach ($command in $gateCommands) {
    if (-not @($allowedGateCommands | Where-Object { $command -match $_ })) {
      throw "Linux repository gate contains an unapproved command: $command"
    }
  }
}

function Assert-LinuxPreflightContract {
  param(
    [Parameter(Mandatory)] [string] $Preflight,
    [Parameter(Mandatory)] [string] $FixtureTest
  )

  $required = [ordered]@{
    'non-authorizing receipt' = '''  "authorizing": false,'''
    'no-mutation receipt' = '''  "mutation_performed": false,'''
    'no real acceptance receipt' = '''  "real_platform_acceptance": false,'''
    'conservative support status' = '''    "support_status": "untested"'''
    'read-only KVM open' = 'exec 9<>/dev/kvm'
    'opened KVM descriptor identity' = 'stat -Lc ''%d:%i:%t:%T'' -- "/proc/\$\$/fd/9"'
    'current KVM path revalidation' = '\[ "\$kvm_before" = "\$kvm_opened" \] && \[ "\$kvm_before" = "\$kvm_current" \]'
    'bounded probe timeout' = 'timeout -k 1s 10s "\$@"'
    'bounded probe file limit' = 'ulimit -f 128'
    'private receipt parent' = '\[ "\$receipt_parent_mode" = 700 \]'
    'runtime prestate fingerprint' = '"runtime_prestate_fingerprint_sha256"'
    'atomic no-clobber receipt publication' = 'ln -- "\$receipt_temp" "\$receipt_path"'
    'final source drift check' = 'preflight\.candidate_drift: HEAD changed during preflight'
    'final runtime drift check' = 'preflight\.runtime_drift: runtime tree changed during preflight'
    'QEMU version probe' = '"\$qemu_system_path" --version'
    'QEMU accelerator probe' = '"\$qemu_system_path" -accel help'
    'qemu-img version probe' = '"\$qemu_img_path" --version'
    'qemu-img info probe' = '"\$qemu_img_path" info --output=json "\$base_path"'
    'fixture-only test mode' = '--fixture-evidence "\$fixture"'
  }
  $combined = "$Preflight`n$FixtureTest"
  foreach ($entry in $required.GetEnumerator()) {
    if ($combined -notmatch $entry.Value) {
      throw "Linux KVM preflight contract lacks $($entry.Key)"
    }
  }

  if (@([regex]::Matches($Preflight, 'run_bounded_probe ''[^'']+'' "\$qemu_system_path"')).Count -ne 2 -or
      @([regex]::Matches($Preflight, 'run_bounded_probe ''[^'']+'' "\$qemu_img_path"')).Count -ne 2) {
    throw 'Linux KVM preflight may invoke each QEMU tool only through its two declared read-only probes'
  }
  if ($FixtureTest -match '--qemu-system|--qemu-img') {
    throw 'Linux KVM preflight repository test must use fixture evidence only'
  }
  if (@([regex]::Matches($Preflight, '<>/dev/kvm')).Count -ne 1) {
    throw 'Linux KVM preflight must contain exactly one read-write no-ioctl KVM open'
  }

  $forbidden = [ordered]@{
    'privileged command' = '(?m)^\s*(?:sudo|su)\s+'
    'host package mutation' = '(?m)^\s*(?:apt|apt-get|dnf|yum|pacman|zypper)\s+'
    'KVM repair or mutation' = '(?i)\b(?:chmod|chown|chgrp|setfacl|rm|mv|mknod)\b[^\r\n]*/dev/kvm\b|(?<!<)(?:>|>>)\s*/dev/kvm\b'
    'kernel module mutation' = '(?im)^\s*(?:modprobe|insmod|rmmod)\s+'
    'QEMU lifecycle option' = '(?i)"\$qemu_system_path"\s+(?:-S|-machine|-drive|-qmp|-chardev|-device|-daemonize)\b'
    'vmcell lifecycle' = '(?i)\bvmcell\s+(?:run|exec|copy-in|copy-out|destroy|gc)\b'
    'receipt overwrite publication' = '(?m)^\s*mv\s+--\s+"\$receipt_temp"\s+"\$receipt_path"\s*$'
  }
  foreach ($entry in $forbidden.GetEnumerator()) {
    if ($combined -match $entry.Value) {
      throw "Linux KVM preflight contains forbidden $($entry.Key)"
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

function Assert-RejectedPreflightMutation {
  param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $Preflight,
    [Parameter(Mandatory)] [string] $FixtureTest
  )

  try {
    Assert-LinuxPreflightContract -Preflight $Preflight -FixtureTest $FixtureTest
  } catch {
    return
  }
  throw "Linux KVM preflight negative regression was accepted: $Name"
}

Assert-LinuxValidationContract -Workflow $workflow -Gate $gate
Assert-LinuxPreflightContract -Preflight $preflight -FixtureTest $preflightTest

Assert-RejectedPreflightMutation -Name 'privileged command' `
  -Preflight "$preflight`nsudo true" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'host package mutation' `
  -Preflight "$preflight`napt-get install qemu" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'KVM permission repair' `
  -Preflight "$preflight`nchmod 0666 /dev/kvm" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'kernel module mutation' `
  -Preflight "$preflight`nmodprobe kvm" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'QEMU lifecycle' `
  -Preflight "$preflight`n`"`$qemu_system_path`" -S" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'vmcell lifecycle' `
  -Preflight "$preflight`nvmcell run --image test -- true" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'second KVM open' `
  -Preflight "$preflight`nexec 8<>/dev/kvm" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'receipt overwrite publication' `
  -Preflight "$preflight`nmv -- `"`$receipt_temp`" `"`$receipt_path`"" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'live fixture test invocation' `
  -Preflight $preflight -FixtureTest "$preflightTest`n--qemu-system /usr/bin/qemu-system-x86_64"

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
