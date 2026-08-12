$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$helperPath = Join-Path $repositoryRoot 'tools\ci-timing.ps1'
$workflowPath = Join-Path $repositoryRoot '.github\workflows\ci.yml'

foreach ($path in @($helperPath, $workflowPath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Windows CI timing surface is missing: $path"
  }
}

$helper = [IO.File]::ReadAllText($helperPath)
$workflow = [IO.File]::ReadAllText($workflowPath)
$stages = @(
  'format',
  'powershell-static',
  'windows-preflight-contract',
  'linux-validation-contract',
  'linux-reliability-contract',
  'clippy',
  'test',
  'windows-package-contract'
)

foreach ($stage in $stages) {
  if ($helper -notmatch [regex]::Escape("'$stage'")) {
    throw "timing helper does not allowlist stage $stage"
  }
  if ($workflow -notmatch [regex]::Escape("Invoke-VmcellCiTimedStage -Stage '$stage'")) {
    throw "Windows CI does not instrument stage $stage"
  }
}

$requiredWorkflow = [ordered]@{
  'fixed timeout' = '(?m)^    timeout-minutes: 30\r?$'
  'read-only permission' = '(?ms)^permissions:\r?\n  contents: read\r?$'
  'exact checkout' = '(?m)^          ref: \$\{\{ github\.sha \}\}\r?$'
  'credential isolation' = '(?m)^          persist-credentials: false\r?$'
  'bounded target binding' = 'cargo_target_dir=\$target'
  'format command' = 'cargo fmt --all -- --check'
  'format native failure preservation' = 'Invoke-VmcellCiNativeCommand -Action \{ cargo fmt --all -- --check \}'
  'PowerShell static command' = '& \.\\tools\\check-powershell\.ps1'
  'Windows timing behavioral contract command' = '& \.\\tools\\test-ci-timing\.ps1'
  'Windows preflight contract command' = '& \.\\tools\\test-windows-whpx-preflight\.ps1'
  'Linux workflow contract command' = '& \.\\tools\\test-linux-validation-workflow\.ps1'
  'Linux reliability contract command' = '& \.\\tools\\test-linux-reliability-workflow\.ps1'
  'Clippy command' = 'cargo clippy --all-targets --all-features -- -D warnings'
  'Clippy native failure preservation' = 'Invoke-VmcellCiNativeCommand -Action \{ cargo clippy --all-targets --all-features -- -D warnings \}'
  'test command' = 'cargo test --all-targets --all-features'
  'test native failure preservation' = 'Invoke-VmcellCiNativeCommand -Action \{ cargo test --all-targets --all-features \}'
  'package build command' = 'cargo build --locked --release --bin vmcell'
  'package build failure preservation' = 'Invoke-VmcellCiNativeCommand -Action \{ cargo build --locked --release --bin vmcell \}'
  'package metadata failure preservation' = 'Invoke-VmcellCiNativeCommand -Action \{ cargo metadata --locked --no-deps --format-version 1 \}'
  'final non-blocking summary' = '(?ms)- name: Sanitized CI timing summary\r?\n        if: always\(\)\r?\n        shell: pwsh'
}
foreach ($entry in $requiredWorkflow.GetEnumerator()) {
  if ($workflow -notmatch $entry.Value) {
    throw "Windows CI timing contract lacks $($entry.Key)"
  }
}

$forbiddenWorkflow = [ordered]@{
  'timeout increase' = '(?m)^    timeout-minutes: (?:3[1-9]|[4-9][0-9]|[1-9][0-9]{2,})\r?$'
  'cache action' = '(?i)actions/cache@'
  'artifact action' = '(?i)actions/(?:upload|download)-artifact@'
  'write permission' = '(?i)\b[A-Za-z-]+\s*:\s*write\b'
  'untrusted pull request trigger' = '(?im)^\s*pull_request(?:_target)?\s*:'
  'timing action network call' = '(?i)(?:invoke-webrequest|invoke-restmethod|curl\.exe|\bwget\b)'
}
foreach ($entry in $forbiddenWorkflow.GetEnumerator()) {
  if ($workflow -match $entry.Value) {
    throw "Windows CI timing contract contains forbidden $($entry.Key)"
  }
}

$forbiddenHelper = [ordered]@{
  'global strict-mode side effect' = '(?i)set-strictmode'
  'host process inspection' = '(?i)\b(?:get-process|get-ciminstance|get-wmiobject|start-process|stop-process)\b'
  'environment dump' = '(?i)\b(?:get-childitem\s+env:|dir\s+env:)'
  'network invocation' = '(?i)\b(?:invoke-webrequest|invoke-restmethod|curl\.exe|\bwget\b)'
  'raw error forwarding' = '(?i)(?:\$_|exception\.message|errorrecord)'
  'credential channel' = '(?i)(?:token|password|secret|credential)'
  'path or argv emission' = '(?i)(?:write-host|write-output|write-error)'
}
foreach ($entry in $forbiddenHelper.GetEnumerator()) {
  if ($helper -match $entry.Value) {
    throw "Windows CI timing helper contains forbidden $($entry.Key)"
  }
}

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("vmcell-ci-timing-" + [guid]::NewGuid().ToString('N'))
$summaryPath = Join-Path $temporaryRoot 'summary.md'
$hadRunnerTemp = Test-Path Env:RUNNER_TEMP
$hadSummary = Test-Path Env:GITHUB_STEP_SUMMARY
$previousRunnerTemp = $env:RUNNER_TEMP
$previousSummary = $env:GITHUB_STEP_SUMMARY

try {
  [IO.Directory]::CreateDirectory($temporaryRoot) | Out-Null
  $env:RUNNER_TEMP = $temporaryRoot
  $env:GITHUB_STEP_SUMMARY = $summaryPath

  . $helperPath
  Invoke-VmcellCiTimedStage -Stage format -Action {}

  $nativeFailureObserved = $false
  try {
    Invoke-VmcellCiNativeCommand -Action { & $env:ComSpec /d /c exit 23 }
  } catch {
    $nativeFailureObserved = $true
  }
  if (-not $nativeFailureObserved) {
    throw 'timing helper failed to preserve an immediate native command failure'
  }

  $failedActionObserved = $false
  try {
    Invoke-VmcellCiTimedStage -Stage clippy -Action { throw 'sensitive failure text must never reach the timing summary' }
  } catch {
    $failedActionObserved = $true
  }
  if (-not $failedActionObserved) {
    throw 'timing helper failed to preserve the wrapped command failure'
  }

  Write-VmcellCiTimingRecord -Stage test -State started -TimestampUtc ([datetime]::UtcNow) -DurationMilliseconds 0
  [IO.File]::WriteAllText((Join-Path $temporaryRoot 'vmcell-ci-timing-invalid.json'), '{not-json}', [Text.UTF8Encoding]::new($false))

  Write-VmcellCiTimingSummary
  $summary = [IO.File]::ReadAllText($summaryPath)
  if ($summary -match [regex]::Escape($temporaryRoot) -or
      $summary -match 'sensitive failure text') {
    throw 'timing summary leaked a path or wrapped failure detail'
  }

  foreach ($line in @($summary -split '\r?\n' | Where-Object { $_ })) {
    if ($line -notmatch '^vmcell\.windows-timing-summary\.v1 stage=(?:format|powershell-static|windows-preflight-contract|linux-validation-contract|linux-reliability-contract|clippy|test|windows-package-contract|checkout-and-setup) state=(?:started|completed|failed|uninstrumented) timestamp_utc=[0-9T:.+\-Z]+ duration_ms=[0-9]{1,7}$') {
      throw 'timing summary did not retain its fixed sanitized shape'
    }
  }
  foreach ($expectedRecord in @(
      'stage=format state=completed',
      'stage=clippy state=failed',
      'stage=test state=started',
      'stage=checkout-and-setup state=uninstrumented'
    )) {
    if ($summary -notmatch [regex]::Escape($expectedRecord)) {
      throw 'timing summary did not aggregate the expected sanitized record'
    }
  }
  if ($summary -match 'invalid') {
    throw 'timing summary accepted a malformed timing record'
  }
} finally {
  if ($hadRunnerTemp) { $env:RUNNER_TEMP = $previousRunnerTemp } else { Remove-Item Env:RUNNER_TEMP -ErrorAction SilentlyContinue }
  if ($hadSummary) { $env:GITHUB_STEP_SUMMARY = $previousSummary } else { Remove-Item Env:GITHUB_STEP_SUMMARY -ErrorAction SilentlyContinue }
  Remove-Item -LiteralPath $temporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host 'Windows CI timing safety contract passed'
