$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workflowPath = Join-Path $repositoryRoot '.github\workflows\linux-validation.yml'
$gatePath = Join-Path $repositoryRoot 'tools\check-linux.sh'
$preflightPath = Join-Path $repositoryRoot 'tools\linux-kvm-preflight.sh'
$preflightTestPath = Join-Path $repositoryRoot 'tools\test-linux-kvm-preflight.sh'
$packageGatePath = Join-Path $repositoryRoot 'tools\check-linux-package.sh'
$packageScriptPath = Join-Path $repositoryRoot 'tools\package-linux.py'
$packageTestPath = Join-Path $repositoryRoot 'tools\test-linux-package.py'
$packageLayoutPath = Join-Path $repositoryRoot 'packaging\linux\vmcell-portable-layout.py'

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
foreach ($path in @($packageGatePath, $packageScriptPath, $packageTestPath, $packageLayoutPath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Linux package validation surface is missing: $path"
  }
}

$workflow = [IO.File]::ReadAllText($workflowPath)
$gate = [IO.File]::ReadAllText($gatePath)
$preflight = [IO.File]::ReadAllText($preflightPath)
$preflightTest = [IO.File]::ReadAllText($preflightTestPath)
$packageGate = [IO.File]::ReadAllText($packageGatePath)
$packageScript = [IO.File]::ReadAllText($packageScriptPath)
$packageTest = [IO.File]::ReadAllText($packageTestPath)
$packageLayout = [IO.File]::ReadAllText($packageLayoutPath)
$packageSurface = "$packageGate`n$packageScript`n$packageTest`n$packageLayout"

function Assert-LinuxValidationContract {
  param(
    [Parameter(Mandatory)] [string] $Workflow,
    [Parameter(Mandatory)] [string] $Gate,
    [Parameter(Mandatory)] [string] $ExecutionSurface
  )

  $contract = "$Workflow`n$Gate`n$ExecutionSurface"
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
    'Linux package syntax gate' = 'sh -n tools/check-linux-package\.sh'
    'Linux package contract gate' = 'sh tools/check-linux-package\.sh'
    'external package build target' = 'CARGO_TARGET_DIR must be bound outside the checkout'
    'locked release package build' = 'cargo build --locked --release --bin vmcell'
    'Linux package validation' = 'python3 tools/test-linux-package\.py --binary "\$CARGO_TARGET_DIR/release/vmcell"'
    'Linux package non-root proof' = 'Linux portable-package smoke must run as an unprivileged identity'
    'package assembly entry point' = 'tools/package-linux\.py'
    'package atomic no-replace publication' = '(?s)renameat2\(\s*parent_descriptor,\s*os\.fsencode\(stage\.name\),\s*parent_descriptor,\s*os\.fsencode\(output\.name\),\s*1,'
    'package private output parent' = 'output parent must be current-user-owned and not group/world writable'
    'package source epoch binding' = 'source date epoch must equal the declared commit timestamp'
    'package source version binding' = 'package version must equal the exact Cargo source identity'
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

  $packageForbidden = [ordered]@{
    'Python privilege escalation token' = '(?i)["''](?:sudo|su)["'']'
    'Python host package manager token' = '(?i)["''](?:apt|apt-get|dnf|yum|pacman|zypper)["'']'
    'Python KVM device access' = '(?i)/dev/kvm'
    'Python QEMU process access' = '(?i)qemu-system(?:-[A-Za-z0-9_]+)?'
    'Python vmcell lifecycle argument' = '(?i)["''](?:run|exec|copy-in|copy-out|destroy|gc)["'']'
  }
  foreach ($entry in $packageForbidden.GetEnumerator()) {
    if ($ExecutionSurface -match $entry.Value) {
      throw "Linux package execution surface contains forbidden $($entry.Key)"
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
    '^sh tools/test-linux-kvm-preflight\.sh$',
    '^sh -n tools/check-linux-package\.sh$',
    '^sh tools/check-linux-package\.sh$'
  )
  foreach ($command in $gateCommands) {
    if (-not @($allowedGateCommands | Where-Object { $command -match $_ })) {
      throw "Linux repository gate contains an unapproved command: $command"
    }
  }

  $packageGateCommands = @($packageGate -split '\r?\n' | Where-Object {
    $_ -notmatch '^\s*(?:#|$)' -and $_ -notmatch '^\s*set\s+-eu\s*$'
  })
  $allowedPackageGateCommands = @(
    '^: "\$\{CARGO_TARGET_DIR:\?CARGO_TARGET_DIR must be bound outside the checkout\}"$',
    '^cargo build --locked --release --bin vmcell$',
    '^python3 tools/test-linux-package\.py --binary "\$CARGO_TARGET_DIR/release/vmcell"$'
  )
  foreach ($command in $packageGateCommands) {
    if (-not @($allowedPackageGateCommands | Where-Object { $command -match $_ })) {
      throw "Linux package gate contains an unapproved command: $command"
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
    'atomic exact-target receipt publication' = 'publish_receipt_noreplace "\$receipt_temp" "\$receipt_path"'
    'atomic no-replace syscall' = 'renameat2\(-100, os\.fsencode\(sys\.argv\[1\]\), -100, os\.fsencode\(sys\.argv\[2\]\), 1\)'
    'terminal receipt commit' = '(?ms)publish_receipt_noreplace "\$receipt_temp" "\$receipt_path" \|\| fail[^\r\n]+\r?\nreceipt_temp=\r?\ntrap - EXIT HUP INT TERM\r?\nprintf[^\r\n]+\|\| :\r?\nexit 0'
    'strict receipt JSON validation' = 'json\.load\(open\(sys\.argv\[1\], "r", encoding="utf-8"\)\)'
    'state-root receipt exclusion' = 'receipt must be outside the vmcell state root'
    'strict qemu-img JSON validation' = 'json\.loads\(sys\.argv\[1\]\)'
    'captured initial Git status' = 'repository_status=\$\(git -C "\$repository_path" status'
    'captured final Git status' = 'final_status=\$\(git -C "\$repository_path" status'
    'captured runtime enumeration' = 'unsorted_rows=\$\(find "\$runtime_path"'
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
    'receipt overwrite publication' = '(?m)^\s*(?:mv\s+--|ln\s+--)\s+"\$receipt_temp"\s+"\$receipt_path"\s*$'
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
    [Parameter(Mandatory)] [string] $Gate,
    [Parameter(Mandatory)] [string] $ExecutionSurface
  )

  try {
    Assert-LinuxValidationContract -Workflow $Workflow -Gate $Gate -ExecutionSurface $ExecutionSurface
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

Assert-LinuxValidationContract -Workflow $workflow -Gate $gate -ExecutionSurface $packageSurface
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
Assert-RejectedPreflightMutation -Name 'directory-following receipt link' `
  -Preflight "$preflight`nln -- `"`$receipt_temp`" `"`$receipt_path`"" -FixtureTest $preflightTest
Assert-RejectedPreflightMutation -Name 'live fixture test invocation' `
  -Preflight $preflight -FixtureTest "$preflightTest`n--qemu-system /usr/bin/qemu-system-x86_64"

Assert-RejectedMutation -Name 'automatic push' `
  -Workflow ($workflow -replace '  workflow_dispatch:', '  push: {}') -Gate $gate -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'inline pull request trigger' `
  -Workflow ($workflow -replace '  workflow_dispatch:', "  workflow_dispatch:`n  pull_request: {}") -Gate $gate -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'write permission' `
  -Workflow ($workflow -replace 'contents: read', 'contents: write') -Gate $gate -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'privileged gate command' `
  -Workflow $workflow -Gate "$gate`nsudo true" -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'host package mutation' `
  -Workflow $workflow -Gate "$gate`napt-get install qemu" -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'KVM device mutation' `
  -Workflow $workflow -Gate "$gate`nchmod 0666 /dev/kvm" -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'kernel module mutation' `
  -Workflow $workflow -Gate "$gate`nmodprobe kvm" -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'QEMU lifecycle' `
  -Workflow $workflow -Gate "$gate`nqemu-system-x86_64 --version" -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'vmcell lifecycle' `
  -Workflow $workflow -Gate "$gate`nvmcell run --image test -- true" -ExecutionSurface $packageSurface
Assert-RejectedMutation -Name 'invoked package privilege escalation' `
  -Workflow $workflow -Gate $gate -ExecutionSurface "$packageSurface`nsubprocess.run(['sudo', 'true'])"
Assert-RejectedMutation -Name 'invoked package host mutation' `
  -Workflow $workflow -Gate $gate -ExecutionSurface "$packageSurface`nsubprocess.run(['apt-get', 'install', 'qemu'])"
Assert-RejectedMutation -Name 'invoked package KVM access' `
  -Workflow $workflow -Gate $gate -ExecutionSurface "$packageSurface`nopen('/dev/kvm', 'wb')"
Assert-RejectedMutation -Name 'invoked package provider lifecycle' `
  -Workflow $workflow -Gate $gate -ExecutionSurface "$packageSurface`nsubprocess.run(['vmcell', 'run'])"

Write-Host 'Linux validation workflow safety contract passed'
