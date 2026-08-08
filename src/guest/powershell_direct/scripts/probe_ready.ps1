$session = $null
try {
  $vm = Enter-GuestAction $request.expected
  $null = Assert-ExpectedVm $request.expected
  try {
    $session = Open-GuestSession $vm
    [pscustomobject]@{ status = 'ready' } | ConvertTo-Json -Compress
  } catch {
    if ($_.Exception.Message.StartsWith('GUEST_AUTHENTICATION_FAILED:')) {
      [pscustomobject]@{ status = 'authentication_failed' } | ConvertTo-Json -Compress
    } elseif ($_.Exception.Message.StartsWith('GUEST_NOT_READY:')) {
      [pscustomobject]@{ status = 'guest_not_ready' } | ConvertTo-Json -Compress
    } elseif ($_.Exception.Message.StartsWith('GUEST_SESSION_FAILED:')) {
      [pscustomobject]@{ status = 'session_failed' } | ConvertTo-Json -Compress
    } else {
      [pscustomobject]@{ status = 'session_failed' } | ConvertTo-Json -Compress
    }
  }
} finally {
  Close-GuestSession $session
  Exit-GuestAction
}
