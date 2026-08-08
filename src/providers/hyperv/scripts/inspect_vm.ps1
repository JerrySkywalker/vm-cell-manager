$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$vm = if ($request.lookup.kind -eq 'id') {
  Get-VM -Id ([guid]$request.lookup.value) -ErrorAction SilentlyContinue
} elseif ($request.lookup.kind -eq 'name') {
  Get-VM -Name ([string]$request.lookup.value) -ErrorAction SilentlyContinue
} else {
  throw "Unsupported VM lookup kind: $($request.lookup.kind)"
}

if (-not $vm) {
  [pscustomobject]@{ vm = $null } | ConvertTo-Json -Compress
  exit 0
}

$configurationPath = if ($vm.ConfigurationLocation) {
  [string]$vm.ConfigurationLocation
} else {
  [string]$vm.Path
}
$snapshot = [pscustomobject]@{
  id = [string]$vm.Id
  name = [string]$vm.Name
  power_state = ([string]$vm.State).ToLowerInvariant()
  ownership_marker = [string]$vm.Notes
  configuration_path = $configurationPath
  attached_disks = @(Get-VMHardDiskDrive -VM $vm -ErrorAction Stop | ForEach-Object { [string]$_.Path })
  network_adapter_count = [uint32]@(Get-VMNetworkAdapter -VM $vm -ErrorAction Stop).Count
  cpu_count = [uint16](Get-VMProcessor -VM $vm -ErrorAction Stop).Count
  memory_mib = [uint64]((Get-VMMemory -VM $vm -ErrorAction Stop).Startup / 1MB)
}
[pscustomobject]@{ vm = $snapshot } | ConvertTo-Json -Compress -Depth 6
