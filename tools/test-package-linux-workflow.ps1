$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workflowPath = Join-Path $repositoryRoot '.github\workflows\package-linux.yml'
$workflow = [IO.File]::ReadAllText($workflowPath)

$required = [ordered]@{
  'manual exact-SHA input' = '(?ms)^on:\r?\n  workflow_dispatch:\r?\n    inputs:\r?\n      source_sha:'
  'read-only permissions' = '(?ms)^permissions:\r?\n  contents: read\r?$'
  'exact-SHA concurrency' = 'group: vmcell-package-linux-\$\{\{ inputs\.source_sha \}\}'
  'hosted Ubuntu baseline' = '(?m)^    runs-on: ubuntu-24\.04\r?$'
  'Rust 1.85 toolchain' = '(?m)^      RUSTUP_TOOLCHAIN: 1\.85\.0\r?$'
  'pinned checkout' = 'actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803'
  'exact checkout source' = '(?m)^          ref: \$\{\{ inputs\.source_sha \}\}\r?$'
  'credential isolation' = '(?m)^          persist-credentials: false\r?$'
  'hosted runner proof' = 'test "\$\{RUNNER_ENVIRONMENT:-\}" = ''github-hosted'''
  'external Cargo target' = 'cargo_target="\$RUNNER_TEMP/vmcell-package-linux-target"'
  'locked offline build' = 'cargo build --locked --offline --release --bin vmcell'
  'candidate package validator' = 'python3 tools/test-linux-package\.py --binary "\$binary"'
  'candidate package builder' = 'python3 tools/package-linux\.py'
  'source SHA package binding' = '--source-commit "\$SOURCE_SHA"'
  'source epoch package binding' = '--source-date-epoch "\$source_date_epoch"'
  'checksum verification' = 'sha256sum --check SHA256SUMS\.txt'
  'read-only checkout proof' = 'git diff --exit-code'
  'pinned artifact upload' = 'actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a'
  'short evidence retention' = '(?m)^          retention-days: 14\r?$'
}
foreach ($entry in $required.GetEnumerator()) {
  if ($workflow -notmatch $entry.Value) {
    throw "Linux packaging workflow omitted $($entry.Key)"
  }
}

foreach ($forbidden in ([ordered]@{
  'automatic or untrusted trigger' = '(?im)^\s*(?:push|pull_request|pull_request_target|schedule|repository_dispatch)\s*:'
  'write or identity-token permission' = '(?im)^\s*(?:[A-Za-z-]+\s*:\s*write|id-token\s*:)'
  'secret or explicit token expression' = '\$\{\{\s*(?:secrets\.|github\.token)'
  'self-hosted runner' = '(?im)^\s*-\s*self-hosted\s*$'
  'provider or guest execution' = '(?i)\b(?:qemu-system|/dev/kvm|New-VM|Start-VM|Stop-VM|Remove-VM|vmcell\s+(?:run|exec|copy-in|copy-out|destroy))\b'
}).GetEnumerator()) {
  if ($workflow -match $forbidden.Value) {
    throw "Linux packaging workflow contains forbidden $($forbidden.Key)"
  }
}

$actions = @([regex]::Matches($workflow, '(?m)^\s*uses:\s*([^\s#]+)') |
  ForEach-Object { $_.Groups[1].Value })
if ($actions.Count -ne 2 -or
    $actions[0] -cne 'actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803' -or
    $actions[1] -cne 'actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a') {
  throw 'Linux packaging workflow action set drifted'
}

Write-Host 'Linux exact-source packaging workflow contract passed'
