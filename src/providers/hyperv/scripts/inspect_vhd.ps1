$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$vhd = Get-VHD -Path ([string]$request.path) -ErrorAction Stop

[pscustomobject]@{
  path = [string]$vhd.Path
  disk_format = ([string]$vhd.VhdFormat).ToLowerInvariant()
  disk_type = ([string]$vhd.VhdType).ToLowerInvariant()
  parent_path = if ($vhd.ParentPath) { [string]$vhd.ParentPath } else { $null }
  file_size = [uint64]$vhd.FileSize
  virtual_size = [uint64]$vhd.Size
} | ConvertTo-Json -Compress -Depth 4
