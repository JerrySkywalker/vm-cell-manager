$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$closeoutPath = Join-Path $repositoryRoot 'docs\reliability-closeout.md'
$manifestPath = Join-Path $repositoryRoot 'tools\reliability-campaign.json'
$dispatcherPath = Join-Path $repositoryRoot '.github\workflows\linux-validation.yml'

foreach ($path in @($closeoutPath, $manifestPath, $dispatcherPath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Reliability closeout surface is missing: $path"
  }
}

$closeout = [IO.File]::ReadAllText($closeoutPath)
$dispatcher = [IO.File]::ReadAllText($dispatcherPath)
$manifestHash = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()

$requiredCloseout = [ordered]@{
  'version-neutral authority boundary' = 'This closeout is version-neutral\.'
  'no version authority' = 'does not set version 0\.8\.0'
  'all packet rows' = '(?s)\| A \|.*\| B \|.*\| C \|.*\| D \|.*\| E \|.*\| F \|.*\| G \|'
  'repository-local status vocabulary' = 'COMPLETE_REPOSITORY_LOCAL'
  'terminal G qualification' = 'PENDING_TERMINAL_QUALIFICATION'
  'manifest digest binding' = [regex]::Escape($manifestHash)
  'fixed seed protocol' = '6a09e667f3bcc909'
  'fixed vector protocol' = '`fixed-v1`'
  'minimizer bound' = 'at most 31 non-canonical'
  'case count' = 'five cases'
  'case timeout' = '120 seconds'
  'campaign timeout' = '600 seconds'
  'job timeout' = '15 minutes'
  'capture bound' = '1,048,576 captured bytes'
  'output draining' = 'drains the complete stream'
  'R0 through R5' = '(?s)\| R0 \|.*\| R1 \|.*\| R2 \|.*\| R3 \|.*\| R4 \|.*\| R5 \|'
  'no lower-tier support promotion' = 'R0-R4 never promote support'
  'equal-user limitation' = 'same effective user and write access'
  'Windows Job Object residual' = 'atomic\s+Windows Job Object binding and empty-tree proof'
  'v0.1 frozen SHA' = '32f4adad3881c5248c6c8c5d47982368b7b55799'
  'v0.2 frozen SHA' = 'ed2ed31ae2f0182fc1626321b81e86d09db378c2'
  'v0.3 frozen SHA' = 'd0af04b2e84cf2226628173d2ed0d295aed01f2b'
  'v0.4 frozen SHA' = 'c741be99ef4632b436f394f1c53b71ed57d0d2d9'
  'no-rewrite compatibility' = 'without creating, backfilling, or rewriting'
  'upgrade-required behavior' = 'vmcell\.state\.upgrade_required'
  'disposable correctness topology' = 'GitHub-hosted Windows/Linux x64 jobs are disposable repository-correctness'
  'shared R4 topology' = 'current shared self-hosted Windows runner is R4-only'
  'dedicated R5 topology' = 'Dedicated real Windows/Linux/macOS hosts are R5'
  'future dedicated runner labels' = '`self-hosted`, `windows`, `x64`, `vmcell`, `r4-dedicated`'
  'no current runner mutation' = 'not authorized here'
  'skipped or canceled non-evidence' = 'Skipped jobs and runs canceled before a terminal selected job are\s+never PASS evidence'
}

foreach ($entry in $requiredCloseout.GetEnumerator()) {
  if ($closeout -notmatch $entry.Value) {
    throw "Reliability closeout lacks $($entry.Key)"
  }
}

$expectedConcurrency = '(?ms)^concurrency:\r?\n  group: vmcell-repository-validation-\$\{\{ inputs\.lane \}\}-\$\{\{ inputs\.source_sha \}\}\r?\n  cancel-in-progress: false\r?$'
if ($dispatcher -notmatch $expectedConcurrency) {
  throw 'Repository validation concurrency must be isolated by lane and exact source SHA'
}

$forbidden = [ordered]@{
  'support promotion' = '(?im)^\s*(?:support|real[- ]platform)\s*(?:status\s*)?[:=]\s*(?:supported|accepted|promoted)\s*$'
  'release authority' = '(?i)release/v0\.8\.0|version\s*=\s*["'']0\.8\.0["'']'
  'automatic runner mutation' = '(?i)\b(?:systemctl|sc\.exe|Restart-Service|Stop-Process|qemu-system|Start-VM)\b'
}
foreach ($entry in $forbidden.GetEnumerator()) {
  if ($closeout -match $entry.Value) {
    throw "Reliability closeout contains forbidden $($entry.Key)"
  }
}

Write-Host "Reliability closeout contract passed manifest_sha256=$manifestHash"
