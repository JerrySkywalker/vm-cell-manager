Set-StrictMode -Version 3.0

$script:ForbiddenMutationCommands = @(
  'Enable-WindowsOptionalFeature', 'Disable-WindowsOptionalFeature',
  'Install-WindowsFeature', 'Uninstall-WindowsFeature',
  'Add-WindowsCapability', 'Remove-WindowsCapability',
  'Add-LocalGroupMember', 'Remove-LocalGroupMember',
  'Start-Service', 'Stop-Service', 'Restart-Service', 'Set-Service', 'New-Service', 'Remove-Service',
  'New-VM', 'Set-VM', 'Remove-VM', 'Start-VM', 'Stop-VM', 'Import-VM', 'Export-VM',
  'Rename-VM', 'Move-VM', 'Move-VMStorage', 'Set-VMFirmware',
  'Checkpoint-VM', 'Restore-VM', 'Suspend-VM', 'Resume-VM',
  'Add-VMHardDiskDrive', 'Set-VMHardDiskDrive', 'Remove-VMHardDiskDrive',
  'Add-VMNetworkAdapter', 'Set-VMNetworkAdapter', 'Remove-VMNetworkAdapter',
  'Connect-VMNetworkAdapter', 'Disconnect-VMNetworkAdapter',
  'Set-VMProcessor', 'Set-VMMemory',
  'New-VMSwitch', 'Set-VMSwitch', 'Remove-VMSwitch', 'Add-VMSwitchExtension', 'Remove-VMSwitchExtension',
  'New-VHD', 'Set-VHD', 'Remove-VHD', 'Resize-VHD', 'Mount-VHD', 'Dismount-VHD', 'Convert-VHD', 'Merge-VHD',
  'Initialize-Disk', 'Clear-Disk', 'Set-Disk', 'Format-Volume', 'Set-Volume', 'Dismount-Volume',
  'New-Partition', 'Set-Partition', 'Remove-Partition', 'New-StoragePool', 'Remove-StoragePool',
  'Set-Acl', 'Clear-Acl', 'icacls.exe', 'takeown.exe',
  'New-NetIPAddress', 'Set-NetIPAddress', 'Remove-NetIPAddress', 'New-NetRoute', 'Set-NetRoute', 'Remove-NetRoute',
  'Set-NetIPInterface', 'Restart-NetAdapter', 'Enable-NetAdapter', 'Disable-NetAdapter',
  'New-NetNat', 'Remove-NetNat', 'Set-NetFirewallProfile', 'New-NetFirewallRule', 'Set-NetFirewallRule', 'Remove-NetFirewallRule',
  'New-ItemProperty', 'Set-ItemProperty', 'Remove-ItemProperty', 'reg.exe',
  'Stop-Process', 'Start-Process', 'taskkill.exe', 'Stop-Computer', 'Restart-Computer', 'shutdown.exe', 'sc.exe'
)

$script:AliasMutationCommands = @('Set-Alias', 'New-Alias', 'Import-Alias', 'sal', 'nal', 'ipal')
$script:DynamicExecutionCommands = @('Invoke-Expression', 'iex', 'Invoke-Command', 'icm')
$script:PowerShellHosts = @('powershell', 'powershell.exe', 'pwsh', 'pwsh.exe')
$script:DynamicInvocationMembers = @('AddCommand', 'AddScript', 'InvokeScript')

function Add-VmcellMutationViolation {
  param(
    [Parameter(Mandatory)][object]$Violations,
    [Parameter(Mandatory)][string]$Code,
    [Parameter(Mandatory)][System.Management.Automation.Language.Ast]$Node
  )

  $Violations.Add([pscustomobject]@{
    code = $Code
    extent = $Node.Extent.Text
    offset = $Node.Extent.StartOffset
  })
}

function Get-VmcellPowerShellMutationViolations {
  param([Parameter(Mandatory)][System.Management.Automation.Language.Ast]$Ast)

  $violations = [System.Collections.Generic.List[object]]::new()
  $commands = @($Ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst]
  }, $true))

  foreach ($command in $commands) {
    $name = $command.GetCommandName()
    if ($command.InvocationOperator -ne [System.Management.Automation.Language.TokenKind]::Unknown) {
      Add-VmcellMutationViolation -Violations $violations -Code 'dynamic_call_operator' -Node $command
    }
    if (-not [string]::IsNullOrWhiteSpace($name)) {
      if ($script:ForbiddenMutationCommands -icontains $name) {
        Add-VmcellMutationViolation -Violations $violations -Code 'forbidden_mutation_command' -Node $command
      }
      if ($script:AliasMutationCommands -icontains $name) {
        Add-VmcellMutationViolation -Violations $violations -Code 'alias_dispatch' -Node $command
      }
      if ($script:DynamicExecutionCommands -icontains $name) {
        Add-VmcellMutationViolation -Violations $violations -Code 'dynamic_expression_dispatch' -Node $command
      }
      if ($script:PowerShellHosts -icontains $name) {
        Add-VmcellMutationViolation -Violations $violations -Code 'nested_powershell_host_dispatch' -Node $command
        foreach ($parameter in @($command.CommandElements | Where-Object {
          $_ -is [System.Management.Automation.Language.CommandParameterAst]
        })) {
          if ('EncodedCommand'.StartsWith($parameter.ParameterName, [StringComparison]::OrdinalIgnoreCase)) {
            Add-VmcellMutationViolation -Violations $violations -Code 'encoded_command_dispatch' -Node $command
          }
        }
      }
    }
  }

  foreach ($member in @($Ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.MemberExpressionAst]
  }, $true))) {
    $memberName = $member.Member.Extent.Text.Trim('''', '"')
    if ($script:DynamicInvocationMembers -icontains $memberName) {
      Add-VmcellMutationViolation -Violations $violations -Code 'dynamic_member_dispatch' -Node $member
    }
    if ($member.Expression -is [System.Management.Automation.Language.TypeExpressionAst] -and
        $member.Expression.TypeName.FullName -iin @('ScriptBlock', 'System.Management.Automation.ScriptBlock') -and
        $memberName -ieq 'Create') {
      Add-VmcellMutationViolation -Violations $violations -Code 'scriptblock_creation_dispatch' -Node $member
    }
    if ($member.Expression -is [System.Management.Automation.Language.TypeExpressionAst] -and
        $member.Expression.TypeName.FullName -iin @('PowerShell', 'System.Management.Automation.PowerShell') -and
        $memberName -ieq 'Create') {
      Add-VmcellMutationViolation -Violations $violations -Code 'nested_powershell_creation' -Node $member
    }
  }

  return @($violations | Sort-Object offset, code -Unique)
}

function Test-VmcellPowerShellMutationSurface {
  param(
    [Parameter(Mandatory, ParameterSetName = 'Path')][string]$Path,
    [Parameter(Mandatory, ParameterSetName = 'Text')][string]$Text,
    [Parameter(Mandatory, ParameterSetName = 'Text')][string]$Name
  )

  $tokens = $null
  $parseErrors = $null
  if ($PSCmdlet.ParameterSetName -eq 'Path') {
    $ast = [System.Management.Automation.Language.Parser]::ParseFile(
      [IO.Path]::GetFullPath($Path), [ref]$tokens, [ref]$parseErrors
    )
    $displayName = [IO.Path]::GetFullPath($Path)
  } else {
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($Text, [ref]$tokens, [ref]$parseErrors)
    $displayName = $Name
  }

  $violations = [System.Collections.Generic.List[object]]::new()
  foreach ($error in @($parseErrors)) {
    $violations.Add([pscustomobject]@{ code = 'parser_error'; extent = $error.Message; offset = $error.Extent.StartOffset })
  }
  if ($violations.Count -eq 0) {
    foreach ($violation in @(Get-VmcellPowerShellMutationViolations -Ast $ast)) {
      $violations.Add($violation)
    }
  }

  return [pscustomobject]@{
    name = $displayName
    safe = $violations.Count -eq 0
    violations = @($violations | Sort-Object offset, code -Unique)
  }
}

Export-ModuleMember -Function 'Test-VmcellPowerShellMutationSurface'
