$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$matrixPath = Join-Path $repositoryRoot 'docs\frozen-candidate-correction-matrix.md'
$acceptancePath = Join-Path $repositoryRoot 'docs\release-acceptance-matrix.md'

foreach ($path in @($matrixPath, $acceptancePath)) {
  if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Frozen correction contract surface is missing: $path"
  }
}

$matrix = [IO.File]::ReadAllText($matrixPath)
$acceptance = [IO.File]::ReadAllText($acceptancePath)

function Assert-FrozenCorrectionContract {
  param(
    [Parameter(Mandatory)] [string] $Matrix,
    [Parameter(Mandatory)] [string] $Acceptance
  )

  $requiredMatrix = [ordered]@{
    'v0.1 source' = '32f4adad3881c5248c6c8c5d47982368b7b55799'
    'v0.2 source' = 'ed2ed31ae2f0182fc1626321b81e86d09db378c2'
    'v0.3 source' = 'd0af04b2e84cf2226628173d2ed0d295aed01f2b'
    'v0.4 source' = 'c741be99ef4632b436f394f1c53b71ed57d0d2d9'
    'Windows QEMU direct clamp' = 'f10ea52a9dddf62adf115225ae0f9d83b5f298da'
    'state-root correction' = '06934a5b8ce93b80f5fb2b1fc7353a070751e784'
    'state-root Windows follow-up' = '97e5054f4c7a7d231c2c97c9c6615574f7b83299'
    'artifact-phase correction' = 'e50e1759cb3a1c003230d1de45a5a64c6f6283ce'
    'manual-port feasibility' = 'Q, S, and A can each be semantically reimplemented on\s+an old line, but none is a clean evidence-preserving cherry-pick'
    'Windows Job Object floor' = 'atomic Windows Job Object plus empty'
    'option A' = 'A — separate historical backports'
    'option B' = 'B — one consolidated corrective candidate from current `dev`'
    'option C' = 'C — defer the first public corrective candidate until v0\.5'
    'recommended option B' = '\| \*\*recommended\*\* \|'
    'renewed package evidence' = 'Rebuilt Windows and Linux portable packages'
    'renewed CI evidence' = 'hosted Windows x64 and Linux x64 repository-correctness PASS'
    'renewed R5 evidence' = 'New R5 receipts for each tuple actually proposed'
    'owner decision gate' = '`SELECTED`, `DEFERRED`, or `REJECTED`'
    'no version authority' = 'must not bump a version'
  }
  foreach ($entry in $requiredMatrix.GetEnumerator()) {
    if ($Matrix -notmatch $entry.Value) {
      throw "Frozen correction matrix lacks $($entry.Key)"
    }
  }

  $frozenRows = @($Acceptance -split '\r?\n' | Where-Object {
    $_ -match '^\| v0\.[1-4]\.0 `(?:32f4adad|ed2ed31a|d0af04b2|c741be99)'
  })
  if ($frozenRows.Count -ne 5 -or
      @($frozenRows | Where-Object { $_ -notmatch '`RETIRED_CORRECTION_REQUIRED`' }).Count -ne 0) {
    throw 'Every frozen v0.1-v0.4 tuple must have retired/correction-required current disposition'
  }
  if ($Acceptance -notmatch 'frozen-candidate-correction-matrix\.md' -or
      $Acceptance -notmatch 'not to imply current promotability') {
    throw 'Release acceptance register must link and defer to the correction matrix'
  }

  $forbidden = [ordered]@{
    'version bump authority' = '(?i)version\s*=\s*["'']0\.[589]\.'
    'new release ref authority' = '(?i)create\s+release/v0\.'
    'support promotion' = '(?im)^\s*(?:support|real[- ]platform)\s*(?:status\s*)?[:=]\s*(?:supported|accepted|promoted)\s*$'
  }
  foreach ($entry in $forbidden.GetEnumerator()) {
    if ("$Matrix`n$Acceptance" -match $entry.Value) {
      throw "Frozen correction contract contains forbidden $($entry.Key)"
    }
  }
}

function Assert-RejectedFrozenCorrectionMutation {
  param(
    [Parameter(Mandatory)] [string] $Name,
    [Parameter(Mandatory)] [string] $Matrix,
    [Parameter(Mandatory)] [string] $Acceptance
  )
  try {
    Assert-FrozenCorrectionContract -Matrix $Matrix -Acceptance $Acceptance
  } catch {
    return
  }
  throw "Frozen correction negative regression was accepted: $Name"
}

Assert-FrozenCorrectionContract -Matrix $matrix -Acceptance $acceptance
Assert-RejectedFrozenCorrectionMutation -Name 'missing exact fix SHA' `
  -Matrix ($matrix -replace 'f10ea52a9dddf62adf115225ae0f9d83b5f298da', ('0' * 40)) `
  -Acceptance $acceptance
Assert-RejectedFrozenCorrectionMutation -Name 'pending frozen candidate' `
  -Matrix $matrix -Acceptance ($acceptance -replace '`RETIRED_CORRECTION_REQUIRED`', '`PENDING_REAL_PLATFORM_GATE`')
Assert-RejectedFrozenCorrectionMutation -Name 'missing consolidated recommendation' `
  -Matrix ($matrix -replace '\| \*\*recommended\*\* \|', '| undecided |') -Acceptance $acceptance
Assert-RejectedFrozenCorrectionMutation -Name 'version authority' `
  -Matrix "$matrix`nversion = `"0.8.0`"" -Acceptance $acceptance

Write-Host 'Frozen candidate correction matrix contract passed'
