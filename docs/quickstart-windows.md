# Windows Human MVP Quick Start

> **Release gate:** the paths below use the frozen `v0.1.0` candidate as a
> concrete example. The same verified portable-archive workflow applies to a
> later accepted version, but real Hyper-V
> and PowerShell Direct acceptance is still pending. Do not run its mutating
> steps merely because a repository build or `vmcell doctor` succeeds. Use it
> only with an officially accepted release on the dedicated Windows host class
> named by that release's acceptance record.

This flow assumes:

- 64-bit Windows and a release-admitted Hyper-V host/window;
- the published portable ZIP and matching `SHA256SUMS.txt`;
- one prepared, ordinary, non-reparse Windows VHDX with no backing parent;
- a local Windows guest account that PowerShell Direct may use; and
- PowerShell 7 for the masked, stdin-only password step below.

VM Cell Manager does not build or modify the VHDX, enable Hyper-V, install a
driver/service, create a virtual switch, or change host networking.

## 1. Verify and install the portable binary

Download the release ZIP and checksum manifest into one directory, then set
the two paths below:

```powershell
$download = 'C:\Users\me\Downloads\vmcell-v0.1.0'
$archive = Join-Path $download 'vmcell-v0.1.0-windows-x86_64.zip'
$checksums = Join-Path $download 'SHA256SUMS.txt'
$installParent = Join-Path $env:LOCALAPPDATA 'Programs'

$expected = ((Get-Content -LiteralPath $checksums -Raw).Split(' ', 2)[0]).Trim()
$actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw 'vmcell archive checksum mismatch' }

Expand-Archive -LiteralPath $archive -DestinationPath $installParent
$vmcell = Join-Path $installParent 'vmcell-v0.1.0-windows-x86_64\vmcell.exe'
& $vmcell --version
```

The archive's `INSTALL.txt` describes PATH setup and removal. Removing the
program directory never implicitly removes vmcell state, images, cells, or
artifacts.

An accepted v0.2-or-later archive also contains generated PowerShell completion.
Load it for the current session without changing the profile:

```powershell
. (Join-Path (Split-Path -Parent $vmcell) 'completions\vmcell.ps1')
```

The complete candidate install, upgrade, rollback, and remove procedure is in
[`windows-install-upgrade-remove.md`](windows-install-upgrade-remove.md).

## 2. Inspect host capability

```powershell
& $vmcell doctor
```

`doctor` is read-only. A ready result reports observed provider capability; it
does not prove dedicated-host admission, writer exclusivity, image suitability,
or release acceptance. Stop if the release's separate host admission record
does not cover this machine and window.

## 3. Validate the prepared Windows VHDX

```powershell
$baseVhdx = 'C:\vmcell-images\windows-dev.vhdx'
& $vmcell image validate `
  --path $baseVhdx `
  --guest-os windows `
  --provider hyperv
if ($LASTEXITCODE -ne 0) { throw 'prepared VHDX is not usable' }
```

The read-only report must say `validation=usable`, identify `hyperv`, show
`expected_format=vhdx` and `observed_format=vhdx`, report no backing parent or
issues, and print the VHDX SHA-256.

## 4. Register and re-inspect immutable identity

```powershell
& $vmcell image add `
  --id windows-dev `
  --path $baseVhdx `
  --guest-os windows `
  --provider hyperv
if ($LASTEXITCODE -ne 0) { throw 'image registration failed' }

& $vmcell image inspect windows-dev
if ($LASTEXITCODE -ne 0) { throw 'registered image identity drifted' }
```

Registration records the canonical path, format, file size, and SHA-256.
`image inspect` repeats the provider/content proof before any cell is created.
Do not modify or replace the registered VHDX.

If this state root was used by the frozen v0.1 candidate, stop other vmcell
commands and run the v0.2 compatibility preflight before the first mutation:

```powershell
& $vmcell state check
if ($LASTEXITCODE -ne 0) {
  throw 'state is not compatible; keep it unchanged and follow docs/state-compatibility.md'
}
```

v0.1 and v0.2 share durable format 1, so compatible records are not migrated or
rewritten. An upgrade-required or integrity result is a hard stop, not a repair
invitation.

## 5. Run one command and observe automatic cleanup

The guest password is read from bounded stdin and never belongs on argv or in
an environment variable:

```powershell
$guestUser = 'Administrator'
$guestPassword = Read-Host -Prompt 'Guest password' -MaskInput
try {
  $guestPassword | & $vmcell run `
    --image windows-dev `
    --provider hyperv `
    --username $guestUser `
    --password-stdin `
    -- cmd.exe /d /c ver
  $runExit = $LASTEXITCODE
} finally {
  $guestPassword = $null
}
if ($runExit -ne 0) { throw "guest command failed with exit $runExit" }
```

Human output distinguishes image verification, cell creation, provider start,
guest readiness, guest stdout/stderr and exit status, cleanup, and the final
cell disposition. The default policy destroys the exact-owned cell after a
completed successful command. Unknown or ambiguous guest/provider state is
retained for explicit recovery and is never automatically replayed.

Ctrl-C is cooperative: vmcell observes it at the next bounded orchestration
stage. If guest transport was active, the operation and cell are retained for
inspection; if the command had already completed, its result remains durable
and the configured keep/cleanup policy applies. See
[`state-compatibility.md`](state-compatibility.md) for the recovery matrix.

## 6. Verify the cleanup record

Copy the `CELL_ID` printed by the run progress/result and inspect it:

```powershell
& $vmcell inspect CELL_ID
```

The record must be `Destroyed`, provider reconciliation must show no live
owned VM, and the run output must have reported `cleanup=destroyed`. If the
record is retained or recovery-required, follow the reported classification;
do not delete Hyper-V objects manually by name.

## 7. Open an optional retained shell session

For a daily-driver session, repeat step 5 with `--keep` and copy the new
`CELL_ID`. The cell must already be running, ready, and exact-owned. Supply the
password on bounded stdin; shell commands are read separately from the
attached Windows console:

```powershell
$guestPassword = Read-Host -Prompt 'Guest password' -MaskInput
try {
  $guestPassword | & $vmcell shell CELL_ID `
    --username $guestUser `
    --password-stdin
} finally {
  $guestPassword = $null
}
```

This is deliberately not a PTY. Every nonempty line starts one independent,
bounded `powershell.exe -Command` operation with a fresh ownership proof.
Guest stdin, `Read-Host`, full-screen programs, and persistent current
directory, environment, process, or PowerShell session state are unavailable.
Use `.help` for the local directives and `.exit`, EOF, or cooperative Ctrl-C
to leave the cell running.

A timeout, broken transport/session, ownership drift, or prior nonterminal
guest operation stops the shell without replay or automatic cleanup. Record
the reported operation ID and use `vmcell status` plus `vmcell operation
inspect` before deciding whether manual investigation is required. Never
repeat an uncertain command automatically. When the cell is proven safe to
remove, use the normal exact-owned lifecycle path:

```powershell
& $vmcell destroy CELL_ID
```

## Current acceptance status

Repository-local tests cover orchestration, cleanup policies, credential
redaction, image drift, deterministic packaging, and human/JSON contracts with
mock providers and non-destructive subprocess tests. They are not proof of real
Hyper-V or PowerShell Direct behavior. The first public `v0.1.0` archive/tag is
permitted only after the frozen release candidate completes the dedicated-host
acceptance sequence in `docs/m1-hyperv-acceptance.md` plus its M2 guest-control
claims, with foreign state unchanged.
