$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$expected = $request.expected

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
      ([string]$vm.State).ToLowerInvariant() -ne [string]$Expected.power_state -or
      (ConvertTo-PathIdentity $configurationPath) -ne (ConvertTo-PathIdentity ([string]$Expected.configuration_path)) -or
      $disks.Count -ne $expectedDisks.Count -or
      $disks.Count -ne 1 -or
      (ConvertTo-PathIdentity $disks[0]) -ne (ConvertTo-PathIdentity ([string]$expectedDisks[0])) -or
      $adapters.Count -ne [uint32]$Expected.network_adapter_count -or
      [uint16]$processor.Count -ne [uint16]$Expected.cpu_count -or
      [uint64]($memory.Startup / 1MB) -ne [uint64]$Expected.memory_mib) {
    throw 'OWNERSHIP_CHANGED: Hyper-V VM ownership precondition changed before start'
  }
  return $vm
}

$vm = Assert-ExpectedVm $expected
Start-VM -VM $vm -ErrorAction Stop | Out-Null
[pscustomobject]@{ ok = $true } | ConvertTo-Json -Compress
