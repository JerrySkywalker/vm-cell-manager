$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$scriptRoots = @(
  (Join-Path $repositoryRoot 'src\providers\hyperv\scripts'),
  (Join-Path $repositoryRoot 'src\guest\powershell_direct\scripts')
)
$guestRoot = [IO.Path]::GetFullPath((Join-Path $repositoryRoot 'src\guest\powershell_direct\scripts'))
$hostGlobalCommands = @(
  'Enable-WindowsOptionalFeature',
  'Disable-WindowsOptionalFeature',
  'Enable-ComputerRestore',
  'Restart-Computer',
  'Stop-Computer',
  'New-VMSwitch',
  'Set-VMSwitch',
  'Remove-VMSwitch'
)
$guestProviderMutationCommands = @(
  'New-VM',
  'Set-VM',
  'Start-VM',
  'Stop-VM',
  'Remove-VM',
  'New-VHD',
  'Set-VHD',
  'Remove-VHD',
  'Add-VMHardDiskDrive',
  'Remove-VMHardDiskDrive',
  'Add-VMNetworkAdapter',
  'Remove-VMNetworkAdapter',
  'Set-VMProcessor',
  'Set-VMMemory'
)
$guestDynamicExecutionCommands = @(
  'Invoke-Expression',
  'iex'
)

$files = @($scriptRoots | ForEach-Object {
  Get-ChildItem -LiteralPath $_ -Filter '*.ps1' -File
})
$files += @(
  (Get-Item -LiteralPath (Join-Path $repositoryRoot 'tools\windows-whpx-preflight.ps1')),
  (Get-Item -LiteralPath (Join-Path $repositoryRoot 'tools\test-windows-whpx-preflight.ps1')),
  (Get-Item -LiteralPath (Join-Path $repositoryRoot 'tools\test-linux-validation-workflow.ps1')),
  (Get-Item -LiteralPath (Join-Path $repositoryRoot 'tools\test-linux-reliability-workflow.ps1')),
  (Get-Item -LiteralPath (Join-Path $repositoryRoot 'tools\ci-timing.ps1')),
  (Get-Item -LiteralPath (Join-Path $repositoryRoot 'tools\test-ci-timing.ps1'))
)
if ($files.Count -eq 0) { throw 'no PowerShell provider or guest scripts were found' }

foreach ($file in $files) {
  $tokens = $null
  $parseErrors = $null
  $ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $file.FullName,
    [ref]$tokens,
    [ref]$parseErrors
  )
  if ($parseErrors.Count -ne 0) {
    $messages = ($parseErrors | ForEach-Object { $_.Message }) -join '; '
    throw "PowerShell parse failure in $($file.FullName): $messages"
  }

  $commands = @($ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst]
  }, $true) | ForEach-Object { $_.GetCommandName() } | Where-Object { $_ })

  foreach ($forbidden in $hostGlobalCommands) {
    if ($commands -contains $forbidden) {
      throw "host-global command $forbidden is forbidden in $($file.FullName)"
    }
  }

  $fullName = [IO.Path]::GetFullPath($file.FullName)
  if ($fullName.StartsWith($guestRoot, [StringComparison]::OrdinalIgnoreCase)) {
    foreach ($forbidden in $guestProviderMutationCommands) {
      if ($commands -contains $forbidden) {
        throw "provider mutation command $forbidden is forbidden in guest shim $fullName"
      }
    }
    foreach ($forbidden in $guestDynamicExecutionCommands) {
      if ($commands -contains $forbidden) {
        throw "dynamic execution command $forbidden is forbidden in guest shim $fullName"
      }
    }
    $text = [IO.File]::ReadAllText($fullName)
    if ($text -match '(?i)\$env:' -or
        $text -match '(?i)GetEnvironmentVariable' -or
        $text -match '(?i)-VMName' -or
        $text -match '(?i)\bGet-VM\b[^\r\n|;]*\s-Name\b') {
      throw "guest shim contains a forbidden environment, secret, or name-authority channel: $fullName"
    }
  }
}

Write-Host "PowerShell AST/static safety passed for $($files.Count) scripts"
