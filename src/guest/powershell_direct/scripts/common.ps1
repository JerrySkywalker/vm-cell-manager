$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

function Read-ExactBytes([System.IO.BinaryReader]$Reader, [uint32]$Length) {
  $bytes = $Reader.ReadBytes([int]$Length)
  if ($bytes.Length -ne $Length) { throw 'GUEST_SESSION_FAILED: truncated stdin frame' }
  return $bytes
}

$reader = [System.IO.BinaryReader]::new([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8, $false)
$actionLength = $reader.ReadUInt32()
if ($actionLength -gt 400000000) { throw 'GUEST_SESSION_FAILED: action frame exceeds limit' }
$request = [System.Text.Encoding]::UTF8.GetString((Read-ExactBytes $reader $actionLength)) | ConvertFrom-Json
$usernameLength = $reader.ReadUInt32()
if ($usernameLength -gt 256) { throw 'GUEST_SESSION_FAILED: username frame exceeds limit' }
$guestUsername = [System.Text.Encoding]::UTF8.GetString((Read-ExactBytes $reader $usernameLength))
$passwordLength = $reader.ReadUInt32()
if ($passwordLength -gt 4096) { throw 'GUEST_SESSION_FAILED: password frame exceeds limit' }
$guestPasswordBytes = Read-ExactBytes $reader $passwordLength
$reader.Dispose()

$providerMutex = [System.Threading.Mutex]::new($false, 'Global\vmcell-hyperv-provider-v1')
$providerMutexHeld = $false

function ConvertTo-PathIdentity([string]$Value) {
  $identity = $Value.Replace('/', '\')
  if ($identity.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) {
    $identity = '\\' + $identity.Substring(8)
  } elseif ($identity.StartsWith('\\?\', [StringComparison]::OrdinalIgnoreCase)) {
    $identity = $identity.Substring(4)
  }
  return $identity.TrimEnd('\').ToLowerInvariant()
}

function Assert-ExpectedVm($Expected) {
  $vm = Get-VM -Id ([guid]$Expected.id) -ErrorAction Stop
  $configurationPath = if ($vm.ConfigurationLocation) { [string]$vm.ConfigurationLocation } else { [string]$vm.Path }
  $disks = @(Get-VMHardDiskDrive -VM $vm -ErrorAction Stop | ForEach-Object { [string]$_.Path })
  $adapters = @(Get-VMNetworkAdapter -VM $vm -ErrorAction Stop)
  $processor = Get-VMProcessor -VM $vm -ErrorAction Stop
  $memory = Get-VMMemory -VM $vm -ErrorAction Stop
  $expectedDisks = @($Expected.attached_disks)
  if ([string]$vm.Id -ne [string]$Expected.id -or
      [string]$vm.Name -ne [string]$Expected.name -or
      [string]$vm.Notes -ne [string]$Expected.ownership_marker -or
      ([string]$vm.State).ToLowerInvariant() -ne 'running' -or
      [string]$Expected.power_state -ne 'running' -or
      (ConvertTo-PathIdentity $configurationPath) -ne (ConvertTo-PathIdentity ([string]$Expected.configuration_path)) -or
      $disks.Count -ne 1 -or $expectedDisks.Count -ne 1 -or
      (ConvertTo-PathIdentity $disks[0]) -ne (ConvertTo-PathIdentity ([string]$expectedDisks[0])) -or
      $adapters.Count -ne [uint32]$Expected.network_adapter_count -or
      $adapters.Count -ne 0 -or
      [uint16]$processor.Count -ne [uint16]$Expected.cpu_count -or
      [uint64]($memory.Startup / 1MB) -ne [uint64]$Expected.memory_mib) {
    throw 'OWNERSHIP_CHANGED: Hyper-V VM precondition changed before guest action'
  }
  return $vm
}

function Enter-GuestAction($Expected) {
  try { $script:providerMutexHeld = $providerMutex.WaitOne(0) }
  catch [System.Threading.AbandonedMutexException] { throw 'GUEST_UNKNOWN: abandoned provider mutex' }
  if (-not $script:providerMutexHeld) { throw 'OWNERSHIP_CHANGED: another vmcell provider action is active' }
  return Assert-ExpectedVm $Expected
}

function Open-GuestSession($Vm) {
  $secure = [System.Security.SecureString]::new()
  $passwordChars = [System.Text.Encoding]::UTF8.GetChars($script:guestPasswordBytes)
  foreach ($character in $passwordChars) { $secure.AppendChar($character) }
  $secure.MakeReadOnly()
  [Array]::Clear($script:guestPasswordBytes, 0, $script:guestPasswordBytes.Length)
  [Array]::Clear($passwordChars, 0, $passwordChars.Length)
  $credential = [System.Management.Automation.PSCredential]::new($guestUsername, $secure)
  try {
    return New-PSSession -VMId ([guid]$Vm.Id) -Credential $credential -ErrorAction Stop
  } catch {
    if ($_.Exception -is [System.UnauthorizedAccessException] -or
        [string]$_.CategoryInfo.Category -eq 'AuthenticationError' -or
        [string]$_.FullyQualifiedErrorId -match 'Credential|Authentication') {
      throw 'GUEST_AUTHENTICATION_FAILED: PowerShell Direct rejected the credential'
    }
    throw 'GUEST_SESSION_FAILED: PowerShell Direct session could not be created'
  } finally {
    if ($script:guestPasswordBytes) {
      [Array]::Clear($script:guestPasswordBytes, 0, $script:guestPasswordBytes.Length)
    }
    if ($passwordChars) { [Array]::Clear($passwordChars, 0, $passwordChars.Length) }
    $credential = $null
    if ($secure) { $secure.Dispose() }
    $secure = $null
  }
}

function Close-GuestSession($Session) {
  if ($Session) { Remove-PSSession -Session $Session -ErrorAction SilentlyContinue }
}

function Exit-GuestAction {
  if ($providerMutexHeld) { $providerMutex.ReleaseMutex(); $script:providerMutexHeld = $false }
  $providerMutex.Dispose()
}
