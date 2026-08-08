$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
trap { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }

$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$vm = Get-VM -Id ([guid]$request.id) -ErrorAction Stop
Start-VM -VM $vm -ErrorAction Stop | Out-Null
[pscustomobject]@{ ok = $true } | ConvertTo-Json -Compress
