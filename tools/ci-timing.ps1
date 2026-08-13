$script:VmcellCiTimingContract = 'vmcell.windows-timing-summary.v1'
$script:VmcellCiTimingStages = @(
  'format',
  'powershell-static',
  'windows-preflight-contract',
  'linux-validation-contract',
  'linux-reliability-contract',
  'clippy',
  'test',
  'windows-package-contract'
)

function Get-VmcellCiTimingRoot {
  if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
    return $null
  }

  try {
    return [IO.Path]::GetFullPath($env:RUNNER_TEMP)
  } catch {
    return $null
  }
}

function Write-VmcellCiTimingRecord {
  param(
    [Parameter(Mandatory)]
    [ValidateSet(
      'format',
      'powershell-static',
      'windows-preflight-contract',
      'linux-validation-contract',
      'linux-reliability-contract',
      'clippy',
      'test',
      'windows-package-contract'
    )]
    [string] $Stage,
    [Parameter(Mandatory)]
    [ValidateSet('started', 'completed', 'failed')]
    [string] $State,
    [Parameter(Mandatory)]
    [datetime] $TimestampUtc,
    [Parameter(Mandatory)]
    [long] $DurationMilliseconds
  )

  $root = Get-VmcellCiTimingRoot
  if ($null -eq $root -or -not [IO.Directory]::Exists($root)) {
    return
  }

  $safeDuration = [Math]::Min([Math]::Max($DurationMilliseconds, 0), 1800000)
  $record = [ordered]@{
    schema_version = 1
    contract = $script:VmcellCiTimingContract
    stage = $Stage
    state = $State
    timestamp_utc = $TimestampUtc.ToUniversalTime().ToString('O', [Globalization.CultureInfo]::InvariantCulture)
    duration_ms = $safeDuration
  }
  try {
    $path = Join-Path $root "vmcell-ci-timing-$Stage.json"
    $json = $record | ConvertTo-Json -Compress
    [IO.File]::WriteAllText($path, $json + [Environment]::NewLine, [Text.UTF8Encoding]::new($false))

  } catch {
    # Observability is deliberately non-blocking for the canonical CI gate.
  }
}

function Invoke-VmcellCiNativeCommand {
  param(
    [Parameter(Mandatory)]
    [scriptblock] $Action
  )

  & $Action
  if ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {
    throw 'CI native command failed'
  }
}

function Invoke-VmcellCiTimedStage {
  param(
    [Parameter(Mandatory)]
    [ValidateSet(
      'format',
      'powershell-static',
      'windows-preflight-contract',
      'linux-validation-contract',
      'linux-reliability-contract',
      'clippy',
      'test',
      'windows-package-contract'
    )]
    [string] $Stage,
    [Parameter(Mandatory)]
    [scriptblock] $Action
  )

  $startedAt = [datetime]::UtcNow
  Write-VmcellCiTimingRecord -Stage $Stage -State started -TimestampUtc $startedAt -DurationMilliseconds 0

  try {
    & $Action
  } catch {
    $elapsed = [long]([datetime]::UtcNow - $startedAt).TotalMilliseconds
    Write-VmcellCiTimingRecord -Stage $Stage -State failed -TimestampUtc ([datetime]::UtcNow) -DurationMilliseconds $elapsed
    throw
  }

  $elapsed = [long]([datetime]::UtcNow - $startedAt).TotalMilliseconds
  Write-VmcellCiTimingRecord -Stage $Stage -State completed -TimestampUtc ([datetime]::UtcNow) -DurationMilliseconds $elapsed
}

function Write-VmcellCiTimingSummary {
  if ([string]::IsNullOrWhiteSpace($env:GITHUB_STEP_SUMMARY)) {
    return
  }

  try {
    $lines = [Collections.Generic.List[string]]::new()
    $root = Get-VmcellCiTimingRoot
    if ($null -ne $root -and [IO.Directory]::Exists($root)) {
      foreach ($stage in $script:VmcellCiTimingStages) {
        $path = Join-Path $root "vmcell-ci-timing-$stage.json"
        if (-not [IO.File]::Exists($path)) {
          continue
        }

        try {
          $record = [IO.File]::ReadAllText($path, [Text.UTF8Encoding]::new($false)) | ConvertFrom-Json
          if ($record.contract -ne $script:VmcellCiTimingContract -or
              $record.stage -ne $stage -or
              $record.state -notin @('started', 'completed', 'failed') -or
              $record.schema_version -ne 1) {
            continue
          }
          $parsedTimestamp = [datetime]::MinValue
          if (-not [datetime]::TryParse(
              [string] $record.timestamp_utc,
              [Globalization.CultureInfo]::InvariantCulture,
              [Globalization.DateTimeStyles]::RoundtripKind,
              [ref] $parsedTimestamp)) {
            continue
          }
          $duration = [long] $record.duration_ms
          if ($duration -lt 0 -or $duration -gt 1800000) {
            continue
          }
          $lines.Add("$($script:VmcellCiTimingContract) stage=$stage state=$($record.state) timestamp_utc=$($parsedTimestamp.ToUniversalTime().ToString('O', [Globalization.CultureInfo]::InvariantCulture)) duration_ms=$duration")
        } catch {
          # A partial or malformed timing record is diagnostic-only and ignored.
        }
      }
    }
    $lines.Add("$($script:VmcellCiTimingContract) stage=checkout-and-setup state=uninstrumented timestamp_utc=$([datetime]::UtcNow.ToString('O', [Globalization.CultureInfo]::InvariantCulture)) duration_ms=0")
    [IO.File]::AppendAllText(
      $env:GITHUB_STEP_SUMMARY,
      ($lines -join [Environment]::NewLine) + [Environment]::NewLine,
      [Text.UTF8Encoding]::new($false)
    )
  } catch {
    # The timing summary must not change the result of the canonical CI gate.
  }
}
