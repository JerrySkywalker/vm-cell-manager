# Windows QEMU/WHPX Human MVP

This is the canonical Windows QEMU path for a prepared Linux x86_64 QCOW2
guest with QGA. QEMU is the provider, WHPX is its accelerator, and QGA is the
Linux guest transport. There is no separate WHPX provider or Windows-specific
lifecycle state machine.

> **Acceptance gate:** repository tests, fake QMP/QGA peers, the preflight
> receipt, and a successful `doctor` do not accept a real QEMU/WHPX host. The
> mutating commands below are an operator walkthrough for a future separately
> authorized, dedicated Windows acceptance host. They must not be run merely
> because this repository-local slice is green. Windows-guest QGA is explicitly
> unsupported.

VM Cell Manager does not install QEMU, enable Windows features, change host
networking, create services, or silently choose TCG. It consumes an existing
QEMU installation and a prepared immutable QCOW2 base.

## 1. Read-only host and state diagnostics

Use an already admitted ordinary, non-reparse state root. These commands do
not create a VM:

```powershell
$vmcell = 'C:\Program Files\vmcell\vmcell.exe'
$stateRoot = 'C:\vmcell-acceptance\state'
& $vmcell --state-root $stateRoot state check
& $vmcell --state-root $stateRoot doctor
& $vmcell --state-root $stateRoot status
```

The QEMU probe binds the canonical `qemu-system-x86_64.exe` and `qemu-img.exe`
found on `PATH`, requires recognizable bounded version output, and reports the
native Windows accelerator explicitly. A ready QEMU installation that does
not advertise WHPX remains incapable of a WHPX plan; it is never treated as
permission to use TCG.

## 2. Validate and register a prepared Linux QCOW2

Validation is read-only and rejects a non-QCOW2 image, an existing backing
parent, path ambiguity, or guest/provider mismatch:

```powershell
$base = 'C:\vmcell-acceptance\images\linux-qga.qcow2'
& $vmcell --state-root $stateRoot image validate `
  --path $base `
  --guest-os linux `
  --guest-arch x86-64 `
  --provider qemu
```

On a separately authorized host window, register the immutable identity and
revalidate it before use:

```powershell
& $vmcell --state-root $stateRoot image add `
  --id linux-qga `
  --path $base `
  --guest-os linux `
  --guest-arch x86-64 `
  --provider qemu
& $vmcell --state-root $stateRoot image validate --id linux-qga --provider qemu
& $vmcell --state-root $stateRoot image inspect linux-qga
```

Registration records metadata and hashes; QEMU later receives a new writable
QCOW2 overlay with exactly that immutable base as its parent.

## 3. Resolve the plan before mutation

The normal provider-neutral intent needs no provider flag when exactly one
compatible path survives:

```powershell
& $vmcell --state-root $stateRoot run --image linux-qga --plan-only
& $vmcell --json --state-root $stateRoot run --image linux-qga --plan-only
```

For the Windows portable path, an explicit equivalent is:

```powershell
& $vmcell --state-root $stateRoot run `
  --image linux-qga `
  --provider qemu `
  --accelerator whpx `
  --plan-only
```

The plan must say `provider=qemu`, `accelerator=whpx`, `transport=qga`,
`guest=linux/x86_64`, `support=untested`, and `authority=none`. It grants no
mutation authority. The engine reloads the image and provider evidence before
mutation, so executable, accelerator, architecture, variant, or transport
drift fails closed.

TCG is never implicit, never configuration-authorized, and never a fallback
after WHPX failure. Its development-only path requires both
`--accelerator tcg` and `--allow-tcg` on that invocation.

## 4. Run and guest-action concepts

The following commands mutate a real QEMU guest and remain acceptance-gated.
The first command uses the normal disposable run/cleanup policy:

```powershell
& $vmcell --state-root $stateRoot run `
  --image linux-qga `
  --provider qemu `
  --accelerator whpx `
  -- /usr/bin/uname -a
```

For diagnosis, `--keep` retains the exact-owned cell. Use the reported CellId
for the provider-neutral guest and artifact surface:

```powershell
$cellId = '00000000-0000-0000-0000-000000000000'
& $vmcell --state-root $stateRoot inspect $cellId
& $vmcell --state-root $stateRoot exec $cellId -- /usr/bin/id
& $vmcell --state-root $stateRoot copy-in $cellId `
  --source C:\vmcell-acceptance\input.txt `
  --destination input.txt
& $vmcell --state-root $stateRoot copy-out $cellId --source output.txt
& $vmcell --state-root $stateRoot artifact collect $cellId --path output.txt
& $vmcell --state-root $stateRoot operation list $cellId
```

QGA is credentialless but is not authority-free. Every action requires an
engine-issued guest authority plus a fresh complete QMP proof of the exact
running, networkless VM. A timeout after QGA dispatch is an unknown guest
effect and is never replayed automatically.

## 5. Inspect, reconcile, and exact-owned cleanup

```powershell
& $vmcell --state-root $stateRoot status
& $vmcell --state-root $stateRoot inspect $cellId
& $vmcell --state-root $stateRoot reconcile $cellId
& $vmcell --state-root $stateRoot stop $cellId
& $vmcell --state-root $stateRoot destroy $cellId
& $vmcell --state-root $stateRoot destroy $cellId
```

Process mutation requires the persisted canonical executable hash, PID, start
token, launch digest, and a matching QMP definition. Overlay cleanup also
requires the exact base/overlay identity. PID, process name, pipe name, socket,
or QEMU name alone never authorizes stop, kill, adoption, or deletion. Any
drift is retained for manual review.

## Diagnostics and operator response

| Observation | Stable distinction | Response |
|---|---|---|
| QEMU system executable absent | QEMU probe `unavailable`; system binary absent | Install nothing automatically; satisfy the external host prerequisite. |
| `qemu-img` absent | QEMU probe `unavailable`; image binary absent | Supply an admitted matching toolchain. |
| WHPX unavailable | native accelerator `whpx unavailable` or `vmcell.run_plan.accelerator_unavailable` | Do not fall back to TCG. |
| Host/guest architecture differs | `vmcell.run_plan.architecture_mismatch` | Select a matching prepared variant. |
| QCOW2/provider variant differs | `vmcell.run_plan.incompatible_image_variant` or image-integrity failure | Register the correct immutable QEMU variant. |
| QGA endpoint absent or not ready | `vmcell.guest.not_ready` | Preserve timeout/non-replay semantics; inspect the guest manually. |
| QMP connect, frame, or command failure | `vmcell.provider.timeout`, `vmcell.provider.invalid_response`, or `vmcell.provider.command_failed` | Reconcile; do not infer ownership from the pipe or PID. |
| Process or QMP definition drift | `vmcell.ownership.changed` | Retain the cell and perform manual review. |
| Unknown guest effect or cleanup ambiguity | retained/manual-review state | Never replay the action or delete unproven resources. |

Human and versioned JSON modes use the same stable error taxonomy. Raw QEMU
diagnostics, guest output beyond declared bounds, credentials, and command
secrets are not added to the run-plan contract.

## Non-mutating dedicated-host preflight

[`windows-whpx-preflight.ps1`](../tools/windows-whpx-preflight.ps1) performs
only read-only executable/version/hash, accelerator-advertisement, QCOW2-info,
base-hash, state-root, repository, and foreign-process observations. It writes
one requested receipt file but starts no VM, creates no overlay, connects to no
QMP/QGA endpoint, and grants no provider authority:

```powershell
& .\tools\windows-whpx-preflight.ps1 `
  -RepositoryRoot $PWD `
  -CandidateSha 'REQUIRED_EXACT_40_HEX_SHA' `
  -StateRoot $stateRoot `
  -BaseImagePath $base `
  -QemuSystemPath 'C:\Program Files\qemu\qemu-system-x86_64.exe' `
  -QemuImgPath 'C:\Program Files\qemu\qemu-img.exe' `
  -OwnedNamespace 'vmcell-whpx-acceptance-001' `
  -WriterExclusivityEvidence 'REQUIRED_EXTERNAL_EVIDENCE_ID' `
  -ReceiptPath 'C:\vmcell-acceptance\receipts\whpx-preflight.json'
```

The repository root must be the exact clean worktree at `CandidateSha`, with no
tracked or untracked changes. Otherwise no receipt is written.

WHPX advertisement is capability evidence, not a functional VM acceptance.
The future authorized run must bind the preflight receipt into
[`windows-whpx-acceptance-template.json`](receipts/windows-whpx-acceptance-template.json)
and add writer-exclusivity, real QMP/QGA lifecycle, pre/post foreign-state,
base re-hash, exact cleanup, rollback, and retained/manual-review evidence.

The fixture mode used by repository CI is labeled `evidence_source=fixture`,
`authorizing=false`, `mutation_performed=false`, and
`real_platform_acceptance=false`; it cannot promote the support matrix.
