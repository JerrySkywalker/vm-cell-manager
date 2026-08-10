# Windows Daily Driver

This is the canonical repository-local v0.2 Windows workflow. It joins the
portable installation, state compatibility, image, run, retained-cell,
diagnostic, shell, recovery, cleanup, and upgrade contracts into one sequence.

> **Release gate:** repository-local tests and core CI do not accept a real
> Hyper-V host, PowerShell Direct session, VHDX, or guest account. Run the
> mutating steps only with an accepted binary on the dedicated Windows host,
> image, state root, and exclusive provider-writer window named by a separate
> platform-acceptance record. `doctor=ready` is necessary but not sufficient.

The walkthrough assumes PowerShell 7 (for `Read-Host -MaskInput`), one prepared
ordinary non-reparse Windows VHDX with no backing parent, and a Windows guest
account admitted for PowerShell Direct.

## 1. Install the portable archive

Set the accepted version and downloaded files, verify the checksum, and keep
the archive's versioned top-level directory:

```powershell
$version = '0.2.0'
$download = Join-Path $HOME 'Downloads\vmcell'
$archive = Join-Path $download "vmcell-v$version-windows-x86_64.zip"
$checksums = Join-Path $download 'SHA256SUMS.txt'
$extractDir = Join-Path $env:LOCALAPPDATA "Programs\vmcell\$version"

$expected = ((Get-Content -LiteralPath $checksums -Raw).Split(' ', 2)[0]).Trim()
$actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw 'vmcell archive checksum mismatch' }

Expand-Archive -LiteralPath $archive -DestinationPath $extractDir
$installDir = Join-Path $extractDir "vmcell-v$version-windows-x86_64"
$vmcell = Join-Path $installDir 'vmcell.exe'
& $vmcell --version
& $vmcell --help
```

Optionally load generated completion for this PowerShell session:

```powershell
. (Join-Path $installDir 'completions\vmcell.ps1')
```

The archive and these commands install no driver, service, host feature,
virtual switch, package-manager entry, or completion profile line.

## 2. Bind the admitted state root and inspect capability

Use the exact ordinary, non-reparse, ACL-exclusive state root admitted for the
host. Before the first v0.2 mutation against an existing root, check its
durable format without contacting a provider:

```powershell
$stateRoot = 'C:\Users\me\vmcell-state'
& $vmcell --state-root $stateRoot state check
if ($LASTEXITCODE -ne 0) { throw 'state compatibility is not proven' }

$doctor = & $vmcell --json --state-root $stateRoot doctor | ConvertFrom-Json
$hyperv = @($doctor.providers | Where-Object {
  $_.name -eq 'hyperv' -and
  $_.available -eq $true -and
  $_.status -eq 'ready'
})
if ($LASTEXITCODE -ne 0 -or $doctor.status -ne 'ready' -or $hyperv.Count -ne 1) {
  throw 'Hyper-V provider capability is not ready'
}
```

An absent root reports `empty` without creating it. Format-1 v0.1 records are
read in place without rewrite. `vmcell.state.upgrade_required`, integrity
failure, or missing platform admission is a hard stop, not permission to edit
state JSON or provider objects manually.

## 3. Validate and register the immutable image

VM Cell Manager consumes a prepared VHDX; it does not build, mount, provision,
or modify the base:

```powershell
$baseVhdx = 'C:\vmcell-images\windows-dev.vhdx'
& $vmcell --state-root $stateRoot image validate `
  --path $baseVhdx --guest-os windows --provider hyperv
if ($LASTEXITCODE -ne 0) { throw 'prepared VHDX validation failed' }

& $vmcell --state-root $stateRoot image add `
  --id windows-dev --path $baseVhdx --guest-os windows --provider hyperv
if ($LASTEXITCODE -ne 0) { throw 'image registration failed' }

& $vmcell --state-root $stateRoot image list
& $vmcell --state-root $stateRoot image inspect windows-dev
```

The record binds canonical path, format, size, and SHA-256. Keep the VHDX
immutable. A later drift result blocks new cells and is not auto-repaired.

## 4. Run with retain-on-failure

Never place the guest password on argv or in an environment variable. The
example deliberately returns 23 so `--keep-on-failure` leaves an exact-owned
cell for diagnosis:

```powershell
$guestUser = 'Administrator'
$guestPassword = Read-Host -Prompt 'Guest password' -MaskInput
try {
  $guestPassword | & $vmcell --state-root $stateRoot run `
    --image windows-dev `
    --provider hyperv `
    --keep-on-failure `
    --username $guestUser `
    --password-stdin `
    -- cmd.exe /d /c exit 23
  $runExit = $LASTEXITCODE
} finally {
  $guestPassword = $null
}
```

Record the printed `CELL_ID` and, if present, `OPERATION_ID`. A nonzero guest
exit is a completed guest result, not automatically an unknown side effect.
Timeout, transport loss, interruption after transport activation, or ownership
drift is different: vmcell retains the nonterminal operation and cell, never
replays the command, and refuses automatic cleanup.

## 5. Inspect retained and uncertain work

Use the provider-tolerant summary before choosing a mutation:

```powershell
& $vmcell --state-root $stateRoot status
& $vmcell --state-root $stateRoot inspect CELL_ID
& $vmcell --state-root $stateRoot operation list CELL_ID
& $vmcell --state-root $stateRoot operation inspect OPERATION_ID
```

`status` keeps durable evidence visible even when Hyper-V is unavailable. Its
cleanup guidance is non-authorizing: `manual_review` means stop and investigate;
it never means delete a VM by name. A `TransportActive` operation remains
uncertain and is not made safe merely by displaying it.

## 6. Use the retained line-oriented shell

Only an already running, ready, exact-owned Hyper-V Windows cell is admitted:

```powershell
$guestPassword = Read-Host -Prompt 'Guest password' -MaskInput
try {
  $guestPassword | & $vmcell --state-root $stateRoot shell CELL_ID `
    --username $guestUser --password-stdin
} finally {
  $guestPassword = $null
}
```

This is not a PTY. Every nonempty line starts one independent bounded
`powershell.exe -Command` operation with fresh ownership proof. There is no
guest stdin, `Read-Host`, full-screen control, or persistent cwd, environment,
process, or PowerShell session. `.exit`, EOF, or cooperative Ctrl-C leaves the
cell running. Any timeout, broken transport, drift, or prior nonterminal
operation stops without replay or cleanup.

## 7. Reconcile, then clean up only proven ownership

Reconciliation re-observes durable/provider state; operation reconciliation
never replays guest work:

```powershell
& $vmcell --state-root $stateRoot operation reconcile OPERATION_ID
& $vmcell --state-root $stateRoot reconcile CELL_ID
& $vmcell --state-root $stateRoot status
```

Follow the reported required action. If the cell is exact-owned and cleanup is
proved safe, use the normal lifecycle authority:

```powershell
& $vmcell --state-root $stateRoot destroy CELL_ID
& $vmcell --state-root $stateRoot inspect CELL_ID
```

`gc` is an explicit alternative only for expired exact-owned cells; it skips
unknown guest work. Never remove a Hyper-V VM, runtime path, or overlay by name
outside vmcell to make a report look clean.

When no non-destroyed cell depends on an image, metadata may be retired without
touching the base bytes:

```powershell
& $vmcell --state-root $stateRoot image dependencies windows-dev
& $vmcell --state-root $stateRoot image unregister windows-dev
```

The unregister report must say `bytes_deleted=false`. Keep or delete the VHDX
only through a separate owner decision outside this metadata command.

## 8. Upgrade or remove the binary

For an upgrade, stop concurrent vmcell commands, preserve the old program
directory and a recoverable state-root copy, verify and extract the new archive
to a different versioned directory, then run the new binary's `state check`
before any mutation. On upgrade-required or integrity failure, leave the root
unchanged and return to the binary that owns its schema. There is no implicit
migration, repair, or downgrade.

Removing the portable program directory, PATH entry, or completion profile
line never removes state, images, cells, artifacts, VMs, or host features. See
[`windows-install-upgrade-remove.md`](windows-install-upgrade-remove.md) for the
full bounded procedure and [`state-compatibility.md`](state-compatibility.md)
for the crash/upgrade recovery matrix.

## Evidence boundary

Repository-local CI covers this command graph with mocks, fault injection,
subprocess crash checks, deterministic packaging, and redaction tests. It does
not prove real Hyper-V lifecycle, PowerShell Direct, guest credentials, VHDX
immutability, writer exclusivity, or cleanup on a Windows acceptance host.
