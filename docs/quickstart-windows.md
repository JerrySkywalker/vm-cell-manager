# Windows Human MVP Quick Start

> **Release gate:** this is the canonical `v0.1.0` workflow, but real Hyper-V
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

## 6. Verify the cleanup record

Copy the `CELL_ID` printed by the run progress/result and inspect it:

```powershell
& $vmcell inspect CELL_ID
```

The record must be `Destroyed`, provider reconciliation must show no live
owned VM, and the run output must have reported `cleanup=destroyed`. If the
record is retained or recovery-required, follow the reported classification;
do not delete Hyper-V objects manually by name.

## Current acceptance status

Repository-local tests cover orchestration, cleanup policies, credential
redaction, image drift, deterministic packaging, and human/JSON contracts with
mock providers and non-destructive subprocess tests. They are not proof of real
Hyper-V or PowerShell Direct behavior. The first public `v0.1.0` archive/tag is
permitted only after the frozen release candidate completes the dedicated-host
acceptance sequence in `docs/m1-hyperv-acceptance.md` plus its M2 guest-control
claims, with foreign state unchanged.
