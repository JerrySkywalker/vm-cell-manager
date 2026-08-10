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
$contract = "$workflow`n$gate"

$requiredPatterns = [ordered]@{
  'dispatch-only trigger' = '(?ms)^on:\r?\n  workflow_dispatch:\r?\n    inputs:'
  'exact source input' = '(?m)^      source_sha:\r?$'
  'read-only repository permission' = '(?ms)^permissions:\r?\n  contents: read\r?$'
  'pinned hosted baseline' = '(?m)^    runs-on: ubuntu-24\.04\r?$'
  'Rust 1.85 toolchain' = '(?m)^      RUSTUP_TOOLCHAIN: 1\.85\.0\r?$'
  'exact checkout binding' = '(?m)^          ref: \$\{\{ inputs\.source_sha \}\}\r?$'
  'checkout credential isolation' = '(?m)^          persist-credentials: false\r?$'
  'exact SHA proof' = 'test "\$actual_sha" = "\$SOURCE_SHA"'
  'hosted runner proof' = 'test "\$\{RUNNER_ENVIRONMENT:-\}" = ''github-hosted'''
  'locked metadata gate' = 'cargo metadata --locked'
  'locked check gate' = 'cargo check --locked (?:--workspace )?--all-targets --all-features'
  'locked Clippy gate' = 'cargo clippy --locked (?:--workspace )?--all-targets --all-features -- -D warnings'
  'locked test gate' = 'cargo test --locked (?:--workspace )?--all-targets --all-features'
  'locked doc-test gate' = 'cargo test --locked (?:--workspace )?--all-features --doc'
  'post-gate clean-tree proof' = 'git diff --exit-code'
}

foreach ($entry in $requiredPatterns.GetEnumerator()) {
  if ($contract -notmatch $entry.Value) {
    throw "Linux validation workflow lacks $($entry.Key)"
  }
}

$forbiddenPatterns = [ordered]@{
  'automatic push trigger' = '(?m)^  push:\s*$'
  'automatic pull-request trigger' = '(?m)^  pull_request(?:_target)?:\s*$'
  'scheduled trigger' = '(?m)^  schedule:\s*$'
  'write permission' = '(?m)^\s+[A-Za-z-]+: write\s*$'
  'privileged command' = '(?m)^\s*(?:sudo|su)\s+'
  'host package mutation' = '(?m)^\s*(?:apt|apt-get|dnf|yum|pacman|zypper)\s+'
  'KVM device access' = '/dev/kvm'
  'QEMU lifecycle command' = '(?i)\bqemu-system(?:-[A-Za-z0-9_]+)?\b'
  'vmcell lifecycle command' = '(?i)\bvmcell\s+(?:run|exec|copy-in|copy-out|destroy|gc)\b'
}

foreach ($entry in $forbiddenPatterns.GetEnumerator()) {
  if ($workflow -match $entry.Value) {
    throw "Linux validation workflow contains forbidden $($entry.Key)"
  }
}

Write-Host 'Linux validation workflow safety contract passed'
