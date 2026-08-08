$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$required = @(
  'Get-VM',
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
$detail = if ($available) {
  'Hyper-V PowerShell lifecycle commands detected'
} else {
  'Missing Hyper-V PowerShell commands: ' + ($missing -join ', ')
}

[pscustomobject]@{
  available = $available
  detail = $detail
  missing_commands = $missing
} | ConvertTo-Json -Compress -Depth 4
