$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$temporaryBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$temporaryRoot = [IO.Path]::GetFullPath((Join-Path $temporaryBase ('vmcell-whpx-preflight-' + [Guid]::NewGuid().ToString('N'))))
New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
  $stateRoot = Join-Path $temporaryRoot 'state'
  New-Item -ItemType Directory -Path $stateRoot | Out-Null
  $base = Join-Path $temporaryRoot 'linux.qcow2'
  [IO.File]::WriteAllBytes($base, [byte[]](0x51, 0x46, 0x49, 0xfb))
  $hash64 = 'a' * 64
  $fixture = [ordered]@{
    contract = 'vmcell.windows-whpx-preflight-fixture.v1'
    host_os = 'windows'
    host_architecture = 'x86_64'
    host_fingerprint_sha256 = $hash64
    qemu_system_version = 'QEMU emulator version 9.0.0 fixture'
    qemu_img_version = 'qemu-img version 9.0.0 fixture'
    qemu_system_sha256 = $hash64
    qemu_img_sha256 = $hash64
    accelerators = @('whpx', 'tcg')
    qemu_img_info = [ordered]@{
      format = 'qcow2'
      'virtual-size' = 4
    }
    foreign_process_count = 0
    foreign_process_fingerprint_sha256 = $hash64
  }
  $fixturePath = Join-Path $temporaryRoot 'fixture.json'
  $fixture | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $fixturePath -Encoding utf8NoBOM
  $receiptPath = Join-Path $temporaryRoot 'receipt.json'
  $head = (& git -C $repositoryRoot rev-parse HEAD).Trim()
  if ($LASTEXITCODE -ne 0) { throw 'could not resolve repository HEAD' }

  & (Join-Path $PSScriptRoot 'windows-whpx-preflight.ps1') `
    -RepositoryRoot $repositoryRoot `
    -CandidateSha $head `
    -StateRoot $stateRoot `
    -BaseImagePath $base `
    -OwnedNamespace 'vmcell-fixture-acceptance' `
    -ReceiptPath $receiptPath `
    -FixtureEvidencePath $fixturePath

  $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
  if ($receipt.contract -ne 'vmcell.windows-whpx-preflight.v1' -or
      $receipt.schema_version -ne 1 -or
      $receipt.authorizing -ne $false -or
      $receipt.mutation_performed -ne $false -or
      $receipt.real_platform_acceptance -ne $false -or
      $receipt.evidence_source -ne 'fixture' -or
      $receipt.provider_path.provider -ne 'qemu' -or
      $receipt.provider_path.accelerator -ne 'whpx' -or
      $receipt.provider_path.guest_os -ne 'linux' -or
      $receipt.provider_path.guest_transport -ne 'qga' -or
      $receipt.provider_path.support_status -ne 'untested' -or
      $receipt.whpx.functional_acceptance -ne 'not-proven' -or
      $receipt.writer_exclusivity.granted_by_preflight -ne $false -or
      $receipt.cleanup.policy -ne 'exact-owned-only') {
    throw 'preflight receipt contract was not conservative and non-authorizing'
  }
  $serialized = Get-Content -LiteralPath $receiptPath -Raw
  if ($serialized -match '(?i)password|credential|command_argv|secret') {
    throw 'preflight receipt contained a forbidden disclosure field'
  }

  $receiptHash = (Get-FileHash -LiteralPath $receiptPath -Algorithm SHA256).Hash
  $staleReceiptCaught = $false
  try {
    & (Join-Path $PSScriptRoot 'windows-whpx-preflight.ps1') `
      -RepositoryRoot $repositoryRoot `
      -CandidateSha $head `
      -StateRoot $stateRoot `
      -BaseImagePath $base `
      -OwnedNamespace 'vmcell-fixture-stale-receipt' `
      -ReceiptPath $receiptPath `
      -FixtureEvidencePath $fixturePath
  } catch {
    if ($_.Exception.Message -match '^preflight\.receipt_path_invalid: refusing to replace') {
      $staleReceiptCaught = $true
    } else {
      throw
    }
  }
  if (-not $staleReceiptCaught -or
      (Get-FileHash -LiteralPath $receiptPath -Algorithm SHA256).Hash -ne $receiptHash) {
    throw 'stale preflight receipt was not preserved fail-closed'
  }

  $dirtyRepository = Join-Path $temporaryRoot 'dirty-repository'
  & git clone --quiet --no-hardlinks $repositoryRoot $dirtyRepository
  if ($LASTEXITCODE -ne 0) { throw 'could not create dirty-repository fixture' }
  $origin = (& git -C $repositoryRoot remote get-url origin).Trim()
  & git -C $dirtyRepository remote set-url origin $origin
  if ($LASTEXITCODE -ne 0) { throw 'could not bind dirty-repository origin' }
  [IO.File]::WriteAllText((Join-Path $dirtyRepository 'untracked-evidence.txt'), 'dirty')
  $dirtyCaught = $false
  try {
    & (Join-Path $PSScriptRoot 'windows-whpx-preflight.ps1') `
      -RepositoryRoot $dirtyRepository `
      -CandidateSha $head `
      -StateRoot $stateRoot `
      -BaseImagePath $base `
      -OwnedNamespace 'vmcell-fixture-dirty-candidate' `
      -ReceiptPath (Join-Path $temporaryRoot 'dirty-must-not-exist.json') `
      -FixtureEvidencePath $fixturePath
  } catch {
    if ($_.Exception.Message -match '^preflight\.candidate_dirty:') {
      $dirtyCaught = $true
    } else {
      throw
    }
  }
  if (-not $dirtyCaught) { throw 'dirty candidate worktree did not fail closed' }

  $spoofedRepository = Join-Path $temporaryRoot 'spoofed-origin-repository'
  & git clone --quiet --no-hardlinks $repositoryRoot $spoofedRepository
  if ($LASTEXITCODE -ne 0) { throw 'could not create spoofed-origin fixture' }
  & git -C $spoofedRepository remote set-url origin 'https://evil.example/JerrySkywalker/vm-cell-manager.git'
  if ($LASTEXITCODE -ne 0) { throw 'could not bind spoofed origin' }
  $spoofedOriginCaught = $false
  try {
    & (Join-Path $PSScriptRoot 'windows-whpx-preflight.ps1') `
      -RepositoryRoot $spoofedRepository `
      -CandidateSha $head `
      -StateRoot $stateRoot `
      -BaseImagePath $base `
      -OwnedNamespace 'vmcell-fixture-spoofed-origin' `
      -ReceiptPath (Join-Path $temporaryRoot 'spoofed-must-not-exist.json') `
      -FixtureEvidencePath $fixturePath
  } catch {
    if ($_.Exception.Message -match '^preflight\.repository_invalid: origin') {
      $spoofedOriginCaught = $true
    } else {
      throw
    }
  }
  if (-not $spoofedOriginCaught) { throw 'spoofed repository origin did not fail closed' }

  $badFixture = $fixture.PSObject.Copy()
  $badFixture.accelerators = @('tcg')
  $badFixturePath = Join-Path $temporaryRoot 'fixture-without-whpx.json'
  $badFixture | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $badFixturePath -Encoding utf8NoBOM
  $caught = $false
  try {
    & (Join-Path $PSScriptRoot 'windows-whpx-preflight.ps1') `
      -RepositoryRoot $repositoryRoot `
      -CandidateSha $head `
      -StateRoot $stateRoot `
      -BaseImagePath $base `
      -OwnedNamespace 'vmcell-fixture-no-whpx' `
      -ReceiptPath (Join-Path $temporaryRoot 'must-not-exist.json') `
      -FixtureEvidencePath $badFixturePath
  } catch {
    if ($_.Exception.Message -match '^preflight\.whpx_unavailable:') {
      $caught = $true
    } else {
      throw
    }
  }
  if (-not $caught) { throw 'missing WHPX did not fail deterministically' }

  $templatePath = Join-Path $repositoryRoot 'docs\receipts\windows-whpx-acceptance-template.json'
  $template = Get-Content -LiteralPath $templatePath -Raw | ConvertFrom-Json
  if ($template.contract -ne 'vmcell.windows-whpx-acceptance.v1' -or
      $template.real_platform_acceptance -ne 'pending' -or
      $template.cleanup.policy -ne 'exact-owned-only') {
    throw 'Windows WHPX acceptance template contract was incomplete'
  }

  Write-Host 'Windows WHPX preflight fixture contract passed'
} finally {
  if (Test-Path -LiteralPath $temporaryRoot -PathType Container) {
    if (-not $temporaryRoot.StartsWith($temporaryBase, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($temporaryRoot) -notmatch '^vmcell-whpx-preflight-[0-9a-f]{32}$') {
      throw 'refusing to remove an unverified preflight fixture directory'
    }
    Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
  }
}
