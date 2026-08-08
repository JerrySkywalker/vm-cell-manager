$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$required = @(
  'Get-VM',
  'Get-VMHost',
  'Get-VHD',
  'New-VHD',
  'New-VM',
  'Get-VMHardDiskDrive',
  'Get-VMNetworkAdapter',
  'Remove-VMNetworkAdapter',
  'Get-VMProcessor',
  'Set-VMProcessor',
  'Get-VMMemory',
  'Set-VM',
  'Start-VM',
  'Stop-VM',
  'Remove-VM'
)
$missing = @($required | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) })
$available = $missing.Count -eq 0
$detail = if (-not $available) {
  'Missing Hyper-V PowerShell commands: ' + ($missing -join ', ')
} else {
  try {
    Get-VMHost -ErrorAction Stop | Out-Null
    'Hyper-V host and PowerShell lifecycle commands are available'
  } catch {
    $available = $false
    'Hyper-V host is not available to this identity: ' + $_.Exception.Message
  }
}

[pscustomobject]@{
  available = $available
  detail = $detail
  missing_commands = $missing
} | ConvertTo-Json -Compress -Depth 4
