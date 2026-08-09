$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$providerMutex = [System.Threading.Mutex]::new($false, 'Global\vmcell-hyperv-provider-v1')
$providerMutexHeld = $false

try {
  try { $providerMutexHeld = $providerMutex.WaitOne(0) } catch [System.Threading.AbandonedMutexException] { $providerMutexHeld = $true }
  if (-not $providerMutexHeld) {
    throw 'OWNERSHIP_CHANGED: another vmcell Hyper-V mutation is active'
  }
  $vhd = New-VHD `
    -Path ([string]$request.overlay_path) `
    -ParentPath ([string]$request.parent_path) `
    -Differencing `
    -ErrorAction Stop

  [pscustomobject]@{
    path = [string]$vhd.Path
    disk_format = ([string]$vhd.VhdFormat).ToLowerInvariant()
    disk_type = ([string]$vhd.VhdType).ToLowerInvariant()
    parent_path = if ($vhd.ParentPath) { [string]$vhd.ParentPath } else { $null }
    file_size = [uint64]$vhd.FileSize
    virtual_size = [uint64]$vhd.Size
  } | ConvertTo-Json -Compress -Depth 4
} finally {
  if ($providerMutexHeld) { $providerMutex.ReleaseMutex() }
  $providerMutex.Dispose()
}
