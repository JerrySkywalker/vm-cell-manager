[CmdletBinding(DefaultParameterSetName = 'Live')]
param(
  [Parameter(Mandatory)]
  [string]$RepositoryRoot,

  [Parameter(Mandatory)]
  [ValidatePattern('^[0-9a-f]{40}$')]
  [string]$CandidateSha,

  [Parameter(Mandatory)]
  [string]$StateRoot,

  [Parameter(Mandatory)]
  [string]$BaseImagePath,

  [Parameter(Mandatory)]
  [ValidatePattern('^vmcell-[a-z0-9][a-z0-9-]{0,62}$')]
  [string]$OwnedNamespace,

  [Parameter(Mandatory)]
  [string]$ReceiptPath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$QemuSystemPath,

  [Parameter(Mandatory, ParameterSetName = 'Live')]
  [string]$QemuImgPath,

  [Parameter(Mandatory, ParameterSetName = 'Fixture')]
  [string]$FixtureEvidencePath,

  [ValidatePattern('^[A-Za-z0-9._:-]{1,128}$')]
  [string]$WriterExclusivityEvidence = 'not-proven',

  [ValidateSet('prepared-linux-x86_64-qga-enabled')]
  [string]$QgaAssumption = 'prepared-linux-x86_64-qga-enabled'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

function Get-Sha256Text {
  param([Parameter(Mandatory)][string]$Text)

  $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
  $hash = [Security.Cryptography.SHA256]::HashData($bytes)
  return [Convert]::ToHexString($hash).ToLowerInvariant()
}

function Assert-Sha256Value {
  param(
    [Parameter(Mandatory)][string]$Value,
    [Parameter(Mandatory)][string]$FailureCode
  )

  if ($Value -notmatch '^[0-9a-fA-F]{64}$') {
    throw "$FailureCode`: expected one SHA-256 value"
  }
  return $Value.ToLowerInvariant()
}

function Get-OrdinaryFile {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$FailureCode
  )

  try {
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
  } catch {
    throw "$FailureCode`: file was not found"
  }
  if (-not $item.PSIsContainer -and
      ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) {
    return $item
  }
  throw "$FailureCode`: path is not an ordinary file"
}

function Get-OrdinaryDirectory {
  param(
    [Parameter(Mandatory)][string]$Path,
    [Parameter(Mandatory)][string]$FailureCode
  )

  try {
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
  } catch {
    throw "$FailureCode`: directory was not found"
  }
  if ($item.PSIsContainer -and
      ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0) {
    return $item
  }
  throw "$FailureCode`: path is not an ordinary directory"
}

function Invoke-BoundedProbe {
  param(
    [Parameter(Mandatory)][string]$FilePath,
    [Parameter(Mandatory)][string[]]$Arguments,
    [Parameter(Mandatory)][string]$FailureCode
  )

  $start = [Diagnostics.ProcessStartInfo]::new()
  $start.FileName = $FilePath
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  foreach ($argument in $Arguments) {
    $start.ArgumentList.Add($argument)
  }

  $process = [Diagnostics.Process]::new()
  $process.StartInfo = $start
  try {
    if (-not $process.Start()) {
      throw "$FailureCode`: probe process did not start"
    }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(10000)) {
      $process.Kill($true)
      $process.WaitForExit()
      throw "$FailureCode`: probe timed out"
    }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    if ([Text.Encoding]::UTF8.GetByteCount($stdout) -gt 65536 -or
        [Text.Encoding]::UTF8.GetByteCount($stderr) -gt 65536) {
      throw "$FailureCode`: probe output exceeded 65536 bytes"
    }
    if ($process.ExitCode -ne 0) {
      throw "$FailureCode`: probe exited nonzero"
    }
    return [pscustomobject]@{
      stdout = $stdout
      stderr = $stderr
    }
  } finally {
    $process.Dispose()
  }
}

function Assert-SingleBoundedVersionLine {
  param(
    [Parameter(Mandatory)][string]$Text,
    [Parameter(Mandatory)][string]$Prefix,
    [Parameter(Mandatory)][string]$FailureCode
  )

  $line = @($Text -split "`r?`n" | Where-Object { $_.Length -gt 0 })[0]
  $hasControl = $null -ne $line -and
    @($line.ToCharArray() | Where-Object { [char]::IsControl($_) }).Count -ne 0
  if ($null -eq $line -or $line.Length -gt 512 -or $hasControl -or -not $line.StartsWith($Prefix)) {
    throw "$FailureCode`: version output was not recognized"
  }
  return $line
}

function Get-OptionalJsonProperty {
  param(
    [Parameter(Mandatory)][object]$InputObject,
    [Parameter(Mandatory)][string]$Name
  )

  $property = $InputObject.PSObject.Properties[$Name]
  if ($null -eq $property) { return $null }
  return $property.Value
}

function Get-ForeignQemuPrestate {
  $rows = @(Get-Process -Name 'qemu-system-*' -ErrorAction SilentlyContinue |
    ForEach-Object {
      $startTicks = try { $_.StartTime.ToUniversalTime().Ticks } catch { 0 }
      $pathDigest = try { Get-Sha256Text -Text $_.Path } catch { 'unavailable' }
      "$($_.Id)|$startTicks|$pathDigest"
    } | Sort-Object)
  return [pscustomobject]@{
    count = $rows.Count
    fingerprint_sha256 = Get-Sha256Text -Text ($rows -join "`n")
  }
}

$repository = Get-OrdinaryDirectory -Path $RepositoryRoot -FailureCode 'preflight.repository_invalid'
$state = Get-OrdinaryDirectory -Path $StateRoot -FailureCode 'preflight.state_root_invalid'
$base = Get-OrdinaryFile -Path $BaseImagePath -FailureCode 'preflight.image_variant_incompatible'
if ($base.Extension -ine '.qcow2') {
  throw 'preflight.image_variant_incompatible: prepared base must be QCOW2'
}

$head = (& git -C $repository.FullName rev-parse HEAD 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or $head -ne $CandidateSha) {
  throw 'preflight.candidate_drift: repository HEAD did not match CandidateSha'
}
$origin = (& git -C $repository.FullName remote get-url origin 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or
    $origin -notmatch '(?i)^(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)JerrySkywalker/vm-cell-manager(?:\.git)?$') {
  throw 'preflight.repository_invalid: origin was not JerrySkywalker/vm-cell-manager'
}
$topLevel = (& git -C $repository.FullName rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or
    -not [IO.Path]::GetFullPath($topLevel).TrimEnd('\', '/').Equals(
      $repository.FullName.TrimEnd('\', '/'),
      [StringComparison]::OrdinalIgnoreCase
    )) {
  throw 'preflight.repository_invalid: RepositoryRoot was not the exact worktree root'
}
$worktreeStatus = @(& git -C $repository.FullName status --porcelain=v1 --untracked-files=all 2>$null)
if ($LASTEXITCODE -ne 0) {
  throw 'preflight.repository_invalid: worktree status was unavailable'
}
if ($worktreeStatus.Count -ne 0) {
  throw 'preflight.candidate_dirty: tracked or untracked worktree changes were present'
}

$source = 'live-read-only'
if ($PSCmdlet.ParameterSetName -eq 'Fixture') {
  $fixtureFile = Get-OrdinaryFile -Path $FixtureEvidencePath -FailureCode 'preflight.fixture_invalid'
  $fixture = Get-Content -LiteralPath $fixtureFile.FullName -Raw | ConvertFrom-Json
  if ($fixture.contract -ne 'vmcell.windows-whpx-preflight-fixture.v1') {
    throw 'preflight.fixture_invalid: contract mismatch'
  }
  $source = 'fixture'
  $hostOs = [string]$fixture.host_os
  $hostArch = [string]$fixture.host_architecture
  $hostFingerprint = Assert-Sha256Value -Value ([string]$fixture.host_fingerprint_sha256) -FailureCode 'preflight.fixture_invalid'
  $qemuVersion = [string]$fixture.qemu_system_version
  $qemuImgVersion = [string]$fixture.qemu_img_version
  $accelerators = @($fixture.accelerators | ForEach-Object { [string]$_ })
  $imageInfo = $fixture.qemu_img_info
  $qemuIdentity = [pscustomobject]@{
    path = 'fixture://qemu-system-x86_64.exe'
    sha256 = Assert-Sha256Value -Value ([string]$fixture.qemu_system_sha256) -FailureCode 'preflight.fixture_invalid'
  }
  $qemuImgIdentity = [pscustomobject]@{
    path = 'fixture://qemu-img.exe'
    sha256 = Assert-Sha256Value -Value ([string]$fixture.qemu_img_sha256) -FailureCode 'preflight.fixture_invalid'
  }
  $foreign = [pscustomobject]@{
    count = [int]$fixture.foreign_process_count
    fingerprint_sha256 = Assert-Sha256Value -Value ([string]$fixture.foreign_process_fingerprint_sha256) -FailureCode 'preflight.fixture_invalid'
  }
} else {
  if (-not $IsWindows) {
    throw 'preflight.architecture_mismatch: Windows x86_64 host required'
  }
  $hostOs = 'windows'
  $hostArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
  if ($hostArch -ne 'x64') {
    throw 'preflight.architecture_mismatch: Windows x86_64 host required'
  }
  $hostArch = 'x86_64'
  $machineName = [Environment]::MachineName
  if ([string]::IsNullOrWhiteSpace($machineName)) {
    throw 'preflight.host_identity_unavailable: machine identity was unavailable'
  }
  $hostFingerprintInput = "$machineName|$hostOs|$hostArch|$([Runtime.InteropServices.RuntimeInformation]::OSDescription)"
  $hostFingerprint = Get-Sha256Text -Text $hostFingerprintInput

  $qemu = Get-OrdinaryFile -Path $QemuSystemPath -FailureCode 'preflight.qemu_absent'
  $qemuImg = Get-OrdinaryFile -Path $QemuImgPath -FailureCode 'preflight.qemu_img_absent'
  $systemVersionProbe = Invoke-BoundedProbe -FilePath $qemu.FullName -Arguments @('--version') -FailureCode 'preflight.qemu_invalid'
  $imageVersionProbe = Invoke-BoundedProbe -FilePath $qemuImg.FullName -Arguments @('--version') -FailureCode 'preflight.qemu_img_invalid'
  $acceleratorProbe = Invoke-BoundedProbe -FilePath $qemu.FullName -Arguments @('-accel', 'help') -FailureCode 'preflight.whpx_probe_failed'
  $imageProbe = Invoke-BoundedProbe -FilePath $qemuImg.FullName -Arguments @('info', '--output=json', $base.FullName) -FailureCode 'preflight.image_variant_incompatible'

  $qemuVersion = Assert-SingleBoundedVersionLine -Text $systemVersionProbe.stdout -Prefix 'QEMU emulator version' -FailureCode 'preflight.qemu_invalid'
  $qemuImgVersion = Assert-SingleBoundedVersionLine -Text $imageVersionProbe.stdout -Prefix 'qemu-img version' -FailureCode 'preflight.qemu_img_invalid'
  $accelerators = @($acceleratorProbe.stdout -split "`r?`n" |
    ForEach-Object { $_.Trim() } |
    Where-Object { $_ -in @('whpx', 'kvm', 'hvf', 'tcg') } |
    Sort-Object -Unique)
  try {
    $imageInfo = $imageProbe.stdout | ConvertFrom-Json
  } catch {
    throw 'preflight.image_variant_incompatible: qemu-img info was invalid JSON'
  }
  $qemuIdentity = [pscustomobject]@{
    path = $qemu.FullName
    sha256 = (Get-FileHash -LiteralPath $qemu.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  }
  $qemuImgIdentity = [pscustomobject]@{
    path = $qemuImg.FullName
    sha256 = (Get-FileHash -LiteralPath $qemuImg.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  }
  $foreign = Get-ForeignQemuPrestate
}

if ($hostOs -ne 'windows' -or $hostArch -ne 'x86_64') {
  throw 'preflight.architecture_mismatch: Windows x86_64 host required'
}
if ($accelerators -notcontains 'whpx') {
  throw 'preflight.whpx_unavailable: QEMU did not advertise WHPX'
}
$imageFormat = Get-OptionalJsonProperty -InputObject $imageInfo -Name 'format'
$backingFile = Get-OptionalJsonProperty -InputObject $imageInfo -Name 'backing-filename'
$fullBackingFile = Get-OptionalJsonProperty -InputObject $imageInfo -Name 'full-backing-filename'
if ([string]$imageFormat -ne 'qcow2' -or
    $null -ne $backingFile -or
    $null -ne $fullBackingFile) {
  throw 'preflight.image_variant_incompatible: base must be standalone QCOW2'
}

$baseSha256 = (Get-FileHash -LiteralPath $base.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
$receipt = [ordered]@{
  schema_version = 1
  contract = 'vmcell.windows-whpx-preflight.v1'
  authorizing = $false
  mutation_performed = $false
  real_platform_acceptance = $false
  status = 'preflight-observed'
  evidence_source = $source
  repository = [ordered]@{
    slug = 'JerrySkywalker/vm-cell-manager'
    candidate_sha = $CandidateSha
  }
  host = [ordered]@{
    os = 'windows'
    architecture = 'x86_64'
    fingerprint_sha256 = $hostFingerprint
  }
  provider_path = [ordered]@{
    provider = 'qemu'
    accelerator = 'whpx'
    guest_os = 'linux'
    guest_architecture = 'x86_64'
    guest_transport = 'qga'
    support_status = 'untested'
  }
  qemu_system = [ordered]@{
    path = $qemuIdentity.path
    version = $qemuVersion
    sha256 = $qemuIdentity.sha256
  }
  qemu_img = [ordered]@{
    path = $qemuImgIdentity.path
    version = $qemuImgVersion
    sha256 = $qemuImgIdentity.sha256
  }
  whpx = [ordered]@{
    advertised = $true
    functional_acceptance = 'not-proven'
  }
  state_root = [ordered]@{
    path = $state.FullName
    ordinary_directory = $true
  }
  immutable_base = [ordered]@{
    path = $base.FullName
    format = 'qcow2'
    sha256 = $baseSha256
    size_bytes = [int64]$base.Length
    backing_parent = $null
  }
  qga = [ordered]@{
    assumption = $QgaAssumption
    readiness = 'not-proven'
  }
  foreign_qemu_prestate = $foreign
  writer_exclusivity = [ordered]@{
    evidence = $WriterExclusivityEvidence
    granted_by_preflight = $false
  }
  ownership = [ordered]@{
    exact_owned_namespace = $OwnedNamespace
    process_identity_required = @('canonical-executable', 'executable-sha256', 'pid', 'start-token', 'launch-digest', 'qmp-definition')
  }
  cleanup = [ordered]@{
    policy = 'exact-owned-only'
    rollback_evidence = 'not-run'
    foreign_qemu_poststate = $null
    immutable_base_post_sha256 = $null
  }
}

$receiptFile = [IO.Path]::GetFullPath($ReceiptPath)
$receiptDirectory = Split-Path -Parent $receiptFile
if (-not (Test-Path -LiteralPath $receiptDirectory -PathType Container)) {
  throw 'preflight.receipt_path_invalid: parent directory does not exist'
}
if (Test-Path -LiteralPath $receiptFile) {
  throw 'preflight.receipt_path_invalid: refusing to replace an existing receipt'
}
$temporaryReceipt = Join-Path $receiptDirectory ('.' + [IO.Path]::GetFileName($receiptFile) + '.' + [Guid]::NewGuid().ToString('N') + '.tmp')
try {
  $receipt | ConvertTo-Json -Depth 10 |
    Set-Content -LiteralPath $temporaryReceipt -Encoding utf8NoBOM
  Move-Item -LiteralPath $temporaryReceipt -Destination $receiptFile
} finally {
  if (Test-Path -LiteralPath $temporaryReceipt -PathType Leaf) {
    Remove-Item -LiteralPath $temporaryReceipt -Force
  }
}

Write-Host "Windows WHPX preflight observed; receipt=$receiptFile authority=none acceptance=false"
