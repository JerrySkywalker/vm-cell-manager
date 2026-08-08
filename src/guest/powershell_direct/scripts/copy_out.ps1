$session = $null
try {
  $vm = Enter-GuestAction $request.expected
  $null = Assert-ExpectedVm $request.expected
  $session = Open-GuestSession $vm
  $result = Invoke-Command -Session $session -ArgumentList @(
    [string]$request.cell_id,
    [string]$request.source,
    [uint64]$request.max_bytes
  ) -ScriptBlock {
    param($CellId, $RelativePath, $MaxBytes)
    $root = "C:\ProgramData\vmcell\cells\$CellId\workspace"
    $source = [IO.Path]::GetFullPath([IO.Path]::Combine($root, $RelativePath))
    if (-not $source.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) {
      throw 'GUEST_PATH_VIOLATION: source escaped workspace'
    }
    $current = $root
    foreach ($segment in @($RelativePath.Split('\'))) {
      $current = Join-Path $current $segment
      $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
      if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'GUEST_PATH_VIOLATION: source chain contains a reparse point'
      }
    }
    $item = Get-Item -LiteralPath $source -Force
    if ($item.PSIsContainer -or $item.Length -gt $MaxBytes) {
      throw 'GUEST_PATH_VIOLATION: source is not a bounded ordinary file'
    }
    $stream = [IO.File]::Open($source, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
      if ($stream.Length -gt $MaxBytes -or $stream.Length -gt [int]::MaxValue) {
        throw 'GUEST_PATH_VIOLATION: source exceeds copy limit'
      }
      $bytes = [byte[]]::new([int]$stream.Length)
      $offset = 0
      while ($offset -lt $bytes.Length) {
        $read = $stream.Read($bytes, $offset, $bytes.Length - $offset)
        if ($read -eq 0) { throw 'GUEST_PARTIAL_COPY: source changed during read' }
        $offset += $read
      }
    } finally { $stream.Dispose() }
    [pscustomobject]@{ content_base64 = [Convert]::ToBase64String($bytes); size = [uint64]$bytes.Length }
  }
  $result | ConvertTo-Json -Compress
} finally {
  Close-GuestSession $session
  Exit-GuestAction
}
