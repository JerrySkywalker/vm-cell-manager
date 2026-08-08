$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$existing = Get-VM -Name ([string]$request.name) -ErrorAction SilentlyContinue
if ($existing) {
  throw "Hyper-V VM name already exists: $($request.name)"
}

$vm = New-VM `
  -Name ([string]$request.name) `
  -Generation 2 `
  -Path ([string]$request.configuration_path) `
  -MemoryStartupBytes ([uint64]$request.memory_mib * 1MB) `
  -VHDPath ([string]$request.overlay_path) `
  -ErrorAction Stop

Get-VMNetworkAdapter -VM $vm -ErrorAction Stop |
  Remove-VMNetworkAdapter -Confirm:$false -ErrorAction Stop
Set-VMProcessor -VM $vm -Count ([uint16]$request.cpu_count) -ErrorAction Stop
Set-VM `
  -VM $vm `
  -Notes ([string]$request.ownership_marker) `
  -AutomaticCheckpointsEnabled $false `
  -ErrorAction Stop

$vm = Get-VM -Id $vm.Id -ErrorAction Stop
$configurationPath = if ($vm.ConfigurationLocation) {
  [string]$vm.ConfigurationLocation
} else {
  [string]$vm.Path
}

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
