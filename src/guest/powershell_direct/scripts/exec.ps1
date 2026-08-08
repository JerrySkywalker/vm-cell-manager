$session = $null
try {
  $vm = Enter-GuestAction $request.expected
  $null = Assert-ExpectedVm $request.expected
  $session = Open-GuestSession $vm
  $result = Invoke-Command -Session $session -ArgumentList @(
    [string]$request.program,
    [string]$request.command_line,
    [uint64]$request.timeout_ms,
    [uint64]$request.max_output_bytes
  ) -ScriptBlock {
    param($Program, $CommandLine, $TimeoutMs, $MaxOutputBytes)
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Program
    $start.Arguments = $CommandLine
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false, $false)
    $start.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false, $false)
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    if (-not $process.Start()) { throw 'GUEST_SESSION_FAILED: guest process did not start' }
    $stdout = [System.Text.StringBuilder]::new()
    $stderr = [System.Text.StringBuilder]::new()
    $stdoutBuffer = [char[]]::new(4096)
    $stderrBuffer = [char[]]::new(4096)
    $stdoutEncoder = [System.Text.Encoding]::UTF8.GetEncoder()
    $stderrEncoder = [System.Text.Encoding]::UTF8.GetEncoder()
    $stdoutBytes = [uint64]0
    $stderrBytes = [uint64]0
    $stdoutDone = $false
    $stderrDone = $false
    $timedOut = $false
    $outputExceeded = $false
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $stdoutTask = $process.StandardOutput.ReadAsync($stdoutBuffer, 0, $stdoutBuffer.Length)
    $stderrTask = $process.StandardError.ReadAsync($stderrBuffer, 0, $stderrBuffer.Length)
    while (-not ($stdoutDone -and $stderrDone -and $process.HasExited)) {
      if (-not $stdoutDone -and $stdoutTask.IsCompleted) {
        $count = $stdoutTask.GetAwaiter().GetResult()
        if ($count -eq 0) {
          $stdoutDone = $true
        } else {
          $chunkBytes = [uint64]$stdoutEncoder.GetByteCount($stdoutBuffer, 0, $count, $false)
          if (($stdoutBytes + $stderrBytes + $chunkBytes) -gt $MaxOutputBytes) {
            $outputExceeded = $true
            try { $process.Kill() } catch {}
          } elseif (-not $outputExceeded) {
            $null = $stdout.Append($stdoutBuffer, 0, $count)
            $stdoutBytes += $chunkBytes
          }
          $stdoutTask = $process.StandardOutput.ReadAsync($stdoutBuffer, 0, $stdoutBuffer.Length)
        }
      }
      if (-not $stderrDone -and $stderrTask.IsCompleted) {
        $count = $stderrTask.GetAwaiter().GetResult()
        if ($count -eq 0) {
          $stderrDone = $true
        } else {
          $chunkBytes = [uint64]$stderrEncoder.GetByteCount($stderrBuffer, 0, $count, $false)
          if (($stdoutBytes + $stderrBytes + $chunkBytes) -gt $MaxOutputBytes) {
            $outputExceeded = $true
            try { $process.Kill() } catch {}
          } elseif (-not $outputExceeded) {
            $null = $stderr.Append($stderrBuffer, 0, $count)
            $stderrBytes += $chunkBytes
          }
          $stderrTask = $process.StandardError.ReadAsync($stderrBuffer, 0, $stderrBuffer.Length)
        }
      }
      if (-not $timedOut -and $clock.ElapsedMilliseconds -ge $TimeoutMs) {
        $timedOut = $true
        try { $process.Kill() } catch {}
      }
      if (-not ($stdoutDone -and $stderrDone -and $process.HasExited)) {
        Start-Sleep -Milliseconds 5
      }
    }
    $process.WaitForExit()
    if ($outputExceeded) { throw 'GUEST_OUTPUT_LIMIT: guest output exceeded the configured limit' }
    $stdoutText = $stdout.ToString()
    $stderrText = $stderr.ToString()
    $stdoutBytes = [uint64][System.Text.Encoding]::UTF8.GetByteCount($stdoutText)
    $stderrBytes = [uint64][System.Text.Encoding]::UTF8.GetByteCount($stderrText)
    if (($stdoutBytes + $stderrBytes) -gt $MaxOutputBytes) {
      throw 'GUEST_OUTPUT_LIMIT: guest output exceeded the configured limit'
    }
    [pscustomobject]@{
      exit_code = if ($timedOut) { -1 } else { [int]$process.ExitCode }
      stdout = $stdoutText
      stderr = $stderrText
      stdout_bytes = [uint64]$stdoutBytes
      stderr_bytes = [uint64]$stderrBytes
      timed_out = [bool]$timedOut
    }
  }
  $result | ConvertTo-Json -Compress -Depth 5
} finally {
  Close-GuestSession $session
  Exit-GuestAction
}
