$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workflowRoot = Join-Path $repositoryRoot '.github\workflows'
$lockPath = Join-Path $repositoryRoot 'Cargo.lock'
$denyPath = Join-Path $repositoryRoot 'deny.toml'

$workflows = @(Get-ChildItem -LiteralPath $workflowRoot -File -Filter '*.yml')
if ($workflows.Count -eq 0) {
  throw 'release supply-chain check found no workflows'
}
foreach ($workflow in $workflows) {
  $text = [IO.File]::ReadAllText($workflow.FullName)
  if ($text -match '(?im)^\s*pull_request_target\s*:' -or
      $text -match '(?im)^\s*id-token\s*:' -or
      $text -match '(?im)^\s*[A-Za-z-]+\s*:\s*write\s*$' -or
      $text -match '\$\{\{\s*(?:secrets\.|github\.token)') {
    throw "workflow has a forbidden trusted-release authority surface: $($workflow.Name)"
  }
  if ($text -match '(?im)^\s*pull_request\s*:' -and
      $text -match '(?im)^\s*-\s*self-hosted\s*$') {
    throw "untrusted pull requests must not reach a self-hosted runner: $($workflow.Name)"
  }
  $uses = @([regex]::Matches($text, '(?m)^\s*uses:\s*([^\s#]+)') |
    ForEach-Object { $_.Groups[1].Value })
  foreach ($use in $uses) {
    if ($use.StartsWith('./', [StringComparison]::Ordinal)) {
      continue
    }
    if ($use -cnotmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[0-9a-f]{40}$') {
      throw "external workflow action is not pinned to one commit: $use"
    }
  }
}

$lockText = [IO.File]::ReadAllText($lockPath)
$blocks = @($lockText -split '(?m)^\[\[package\]\]\s*$' | Select-Object -Skip 1)
if ($blocks.Count -eq 0) {
  throw 'Cargo.lock contains no package blocks'
}
$registryPackages = 0
foreach ($block in $blocks) {
  $source = [regex]::Match($block, '(?m)^source = "([^"]+)"$')
  if (-not $source.Success) {
    continue
  }
  $registryPackages++
  if ($source.Groups[1].Value -cne 'registry+https://github.com/rust-lang/crates.io-index') {
    throw "Cargo.lock contains a Git or unknown package source: $($source.Groups[1].Value)"
  }
  if ($block -cnotmatch '(?m)^checksum = "[0-9a-f]{64}"$') {
    throw 'Cargo.lock registry package omitted an exact checksum'
  }
}
if ($registryPackages -ne 102) {
  throw "candidate lock graph count changed unexpectedly: $registryPackages"
}

$denyText = [IO.File]::ReadAllText($denyPath)
foreach ($required in @(
  'wildcards = "deny"',
  'unknown-registry = "deny"',
  'unknown-git = "deny"',
  '"MPL-2.0"'
)) {
  if (-not $denyText.Contains($required, [StringComparison]::Ordinal)) {
    throw "dependency policy omitted required rule: $required"
  }
}

Write-Host "Release supply-chain contract passed for $($workflows.Count) workflows and $registryPackages checksum-bound registry packages"
