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

[pscustomobject]@{
  id = [string]$vm.Id
  name = [string]$vm.Name
} | ConvertTo-Json -Compress
