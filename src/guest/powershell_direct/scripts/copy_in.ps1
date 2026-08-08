$session = $null
try {
  $vm = Enter-GuestAction $request.expected
  $null = Assert-ExpectedVm $request.expected
  $session = Open-GuestSession $vm
  $result = Invoke-Command -Session $session -ArgumentList @(
    [string]$request.cell_id,
    [string]$request.operation_id,
    [string]$request.destination,
    [string]$request.content_base64,
    [string]$request.overwrite
  ) -ScriptBlock {
    param($CellId, $OperationId, $RelativePath, $ContentBase64, $Overwrite)
    function Assert-OrdinaryDirectory([string]$Path) {
      $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
      if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'GUEST_PATH_VIOLATION: guest directory is not ordinary'
      }
    }
    $root = "C:\ProgramData\vmcell\cells\$CellId\workspace"
    $current = 'C:\ProgramData'
    Assert-OrdinaryDirectory $current
    foreach ($segment in @('vmcell', 'cells', $CellId, 'workspace')) {
      $current = Join-Path $current $segment
      if (-not (Test-Path -LiteralPath $current)) { New-Item -ItemType Directory -Path $current | Out-Null }
      Assert-OrdinaryDirectory $current
    }
    $target = [IO.Path]::GetFullPath([IO.Path]::Combine($root, $RelativePath))
    if (-not $target.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) {
      throw 'GUEST_PATH_VIOLATION: destination escaped workspace'
    }
    $parent = Split-Path -Parent $target
    $relativeParent = $parent.Substring($root.Length).TrimStart('\')
    $current = $root
    foreach ($segment in @($relativeParent.Split('\') | Where-Object { $_ })) {
      $current = Join-Path $current $segment
      if (-not (Test-Path -LiteralPath $current)) { New-Item -ItemType Directory -Path $current | Out-Null }
      Assert-OrdinaryDirectory $current
    }
    if (Test-Path -LiteralPath $target) {
      $item = Get-Item -LiteralPath $target -Force
      if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'GUEST_PATH_VIOLATION: destination is not an ordinary file'
      }
      if ($Overwrite -ne 'replace') { throw 'GUEST_PATH_VIOLATION: destination already exists' }
    }
    $temporary = Join-Path $parent ('.vmcell-' + ([guid]$OperationId).ToString() + '.tmp')
    try {
      [IO.File]::WriteAllBytes($temporary, [Convert]::FromBase64String($ContentBase64))
      if (Test-Path -LiteralPath $target) {
        [IO.File]::Replace($temporary, $target, $null, $true)
      } else {
        [IO.File]::Move($temporary, $target)
      }
    } finally {
      if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
    [pscustomobject]@{ ok = $true }
  }
  $result | ConvertTo-Json -Compress
} finally {
  Close-GuestSession $session
  Exit-GuestAction
}
