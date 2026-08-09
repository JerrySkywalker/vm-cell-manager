$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$expected = $request.expected
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

try {
try { $providerMutexHeld = $providerMutex.WaitOne(0) } catch [System.Threading.AbandonedMutexException] { $providerMutexHeld = $true }
if (-not $providerMutexHeld) {
  throw 'OWNERSHIP_CHANGED: another vmcell Hyper-V mutation is active'
}
$vm = Get-VM -Id ([guid]$expected.id) -ErrorAction Stop
$configurationPath = if ($vm.ConfigurationLocation) { [string]$vm.ConfigurationLocation } else { [string]$vm.Path }
$disks = @(Get-VMHardDiskDrive -VM $vm -ErrorAction Stop | ForEach-Object { [string]$_.Path })
$adapters = @(Get-VMNetworkAdapter -VM $vm -ErrorAction Stop)
$processor = Get-VMProcessor -VM $vm -ErrorAction Stop
$memory = Get-VMMemory -VM $vm -ErrorAction Stop
$expectedDisks = @($expected.attached_disks)

if ([string]$vm.Id -ne [string]$expected.id -or
    [string]$vm.Name -ne [string]$expected.name -or
    [string]$vm.Notes -ne [string]$expected.ownership_marker -or
    ([string]$vm.State).ToLowerInvariant() -ne 'off' -or
    (ConvertTo-PathIdentity $configurationPath) -ne (ConvertTo-PathIdentity ([string]$expected.configuration_path)) -or
    $disks.Count -ne $expectedDisks.Count -or
    $disks.Count -ne 1 -or
    (ConvertTo-PathIdentity $disks[0]) -ne (ConvertTo-PathIdentity ([string]$expectedDisks[0])) -or
    ($adapters.Count -ne [uint32]$expected.network_adapter_count -and $adapters.Count -ne 0) -or
    ([uint16]$processor.Count -ne [uint16]$expected.cpu_count -and
      [uint16]$processor.Count -ne [uint16]$request.cpu_count) -or
    [uint64]($memory.Startup / 1MB) -ne [uint64]$expected.memory_mib) {
  throw 'OWNERSHIP_CHANGED: Hyper-V VM ownership precondition changed before configuration'
}

if ($adapters.Count -eq 1) {
  $adapters | Remove-VMNetworkAdapter -Confirm:$false -ErrorAction Stop
}
Set-VMProcessor -VM $vm -Count ([uint16]$request.cpu_count) -ErrorAction Stop

$vm = Get-VM -Id $vm.Id -ErrorAction Stop
$configurationPath = if ($vm.ConfigurationLocation) { [string]$vm.ConfigurationLocation } else { [string]$vm.Path }
[pscustomobject]@{
  id = [string]$vm.Id
  name = [string]$vm.Name
  power_state = ([string]$vm.State).ToLowerInvariant()
  ownership_marker = [string]$vm.Notes
  configuration_path = $configurationPath
  attached_disks = @(Get-VMHardDiskDrive -VM $vm -ErrorAction Stop | ForEach-Object { [string]$_.Path })
  network_adapter_count = [uint32]@(Get-VMNetworkAdapter -VM $vm -ErrorAction Stop).Count
  cpu_count = [uint16](Get-VMProcessor -VM $vm -ErrorAction Stop).Count
  memory_mib = [uint64]((Get-VMMemory -VM $vm -ErrorAction Stop).Startup / 1MB)
} | ConvertTo-Json -Compress -Depth 5
} finally {
  if ($providerMutexHeld) { $providerMutex.ReleaseMutex() }
  $providerMutex.Dispose()
}
