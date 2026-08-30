# Windows Hyper-V R5 read-only preflight

`tools/windows-hyperv-preflight.ps1` is an admission evidence collector for
the v0.4.1 Windows Hyper-V / Windows guest / PowerShell Direct R5 packet. It is
not an authorization mechanism, does not claim functional acceptance, and does
not change the support matrix.

Every result has `authority: "none"`, `acceptance: false`, and
`real_platform_acceptance: "not_started"`. `PREFLIGHT_ELIGIBLE` only means that
the supplied or observed preconditions were complete and suitable for a
separate owner decision. It never grants permission to create, attach, start,
stop, modify, or remove anything.

## Modes

Fixture mode is the CI and regression-test mode. It reads only the supplied
fixture JSON and emits one deterministic JSON document on standard output. It
does not inspect the host, Hyper-V, services, VMs, disks, images, network,
runners, registry, GitHub, or Git state.

```powershell
pwsh -NoProfile -File .\tools\windows-hyperv-preflight.ps1 `
  -FixturePath .\tests\fixtures\hyperv-preflight\eligible.json
```

Live mode is observational only. It reads the candidate bindings, supplied
provenance, Windows/Hyper-V availability, foreign inventory, process activity,
storage facts, and VHDX metadata. It does not make a host or provider change.
Run it only in a separately authorized owner window and keep its raw output
outside the repository.

```powershell
pwsh -NoProfile -File .\tools\windows-hyperv-preflight.ps1 `
  -CandidateSha REQUIRED_EXACT_40_HEX_SHA `
  -CandidatePackagePath REQUIRED_PACKAGE_PATH `
  -CandidateBinaryPath REQUIRED_BINARY_PATH `
  -VhdxPath REQUIRED_VHDX_PATH `
  -ProvenancePath REQUIRED_PROVENANCE_PATH `
  -StateRoot REQUIRED_EXISTING_STATE_ROOT `
  -RuntimeRoot REQUIRED_EXISTING_RUNTIME_ROOT `
  -ReceiptPath REQUIRED_NEW_RECEIPT_PATH `
  -StateRequiredFreeBytes REQUIRED_POSITIVE_BYTES `
  -RuntimeRequiredFreeBytes REQUIRED_POSITIVE_BYTES `
  -ImageRequiredFreeBytes REQUIRED_POSITIVE_BYTES `
  -PackageRequiredFreeBytes REQUIRED_POSITIVE_BYTES `
  -BinaryRequiredFreeBytes REQUIRED_POSITIVE_BYTES `
  -ProvenanceRequiredFreeBytes REQUIRED_POSITIVE_BYTES `
  -RollbackMarginBytes REQUIRED_POSITIVE_BYTES
```

No fixture, CI result, or live preflight result is R5 real-platform acceptance.
That status remains `NOT_STARTED` until a distinct, explicitly authorized
real-platform goal completes.

## Evidence and fail-closed behavior

The result contains a fixed observation register covering elevation and local
administrators membership; Windows build and architecture; Hyper-V feature,
module, cmdlets, VMMS, and read access; VM and switch inventories; competing
virtualization writers and runner/Codex activity; independent state-root,
runtime-root, image, package, binary, and provenance storage roles; a separate
sanitized evidence-output role;
immutable VHDX state; provenance and candidate/package/binary/VHDX hashes;
receipt freshness; and exclusive-window eligibility.

`unavailable` is always emitted as an `evidence_gap.*` blocker. It is never
treated as pass. A failed precondition becomes `precondition_failed.*`; the
owner action names the evidence or external remediation needed. The result
stores only codes, statuses, and SHA-256 evidence digests—never raw host paths,
VM names, command lines, credentials, or process identities.

Every operational path is classified from the supplied path. The tool resolves
its volume, partition, and disk and requires an ordinary non-reparse item and
ancestry, `Fixed` volume type, NTFS, one unambiguous backing disk, an admitted
local bus (`ATA`, `SATA`, `SAS`, `SCSI`, `RAID`, `NVMe`, or `SCM`), and free
capacity greater than or equal to the role requirement plus the rollback
margin. Network, removable, `File Backed Virtual`, unknown, virtual, Storage
Spaces, non-NTFS, missing, reparse, ambiguous, and unavailable boundaries fail
closed. State and runtime roots are independent and may be on different
admitted local volumes.

Sanitized evidence output is not operational storage authority. Its parent
must already exist and be an ordinary non-reparse directory, while the receipt
leaf must not exist. Live mode writes exactly one sanitized receipt with
`FileMode.CreateNew`; it never overwrites. Fixture mode writes no receipt and
does not inspect the output filesystem. The tool also rejects a missing,
dynamic, writable, attached, differencing, or parented VHDX. It leaves all such
evidence untouched.
Before live observation uses provenance, it reads one UTF-8 JSON snapshot,
rejects a reparse-point file or parent, and compares the before/after content
digest. The state root, VHDX, candidate package, and candidate binary also
must each be ordinary non-reparse paths with non-reparse ancestors. A missing,
changed, or unsafe receipt or path becomes an evidence gap and can never
produce `PREFLIGHT_ELIGIBLE`. The live result digest is derived from its ordered
sanitized observations, not wall-clock time.

## Provenance template

Start with
[`windows-hyperv-image-provenance-template.json`](receipts/windows-hyperv-image-provenance-template.json).
The template deliberately contains placeholders, not evidence. A completed owner packet is
validated against the frozen v0.4.1 Windows candidate, package, checksum manifest, binary,
and `x86_64-pc-windows-msvc` target. It must also bind the Windows Server 2022 build, source,
VHDX digest, Generation 2,
Secure Boot `On` with `MicrosoftWindows`, and the remaining security properties,
parent/attachment state, creation time, immutability declaration, receipt, and
exclusive-window evidence before live observation can evaluate it.
The non-executing packet instructions are in
[`windows-hyperv-r5-image-preparation.md`](windows-hyperv-r5-image-preparation.md).

## Post-create, pre-start qualification

After a separately authorized owner action creates the exact stopped cell, and
before any start, use
`tools/windows-hyperv-stopped-cell-qualification.ps1` in a separately
authorized read-only owner window. Its Live parameter set binds an exact VM ID
and name, expected overlay and immutable parent paths, CPU count, startup
memory, Secure Boot template, and a new receipt path. It reads by VM ID and
requires:

- state `Off` and Hyper-V Generation 2;
- Secure Boot `On` with the exact expected template;
- exactly one disk at the exact expected overlay path;
- a differencing overlay with the exact admitted parent and no further parent
  chain;
- exact processor and startup-memory values; and
- zero network adapters.

Unavailable firmware, disk, resource, network, or identity evidence is a
blocking `evidence_gap.*`; a mismatch is `precondition_failed.*`. The helper
never starts, stops, creates, changes, or removes a VM, disk, adapter, feature,
service, runner, or process. A qualified stopped cell still grants no start
authority.

## Safety checks and CI

`tools/test-windows-hyperv-preflight.ps1` runs 78 deterministic checks while
retaining the original 39-case safety coverage. It includes every operational
path role on admitted local NTFS; split state/runtime volumes; missing,
reparse, network, removable, file-backed, ambiguous, non-NTFS, capacity, and
receipt boundaries; stopped-cell firmware/disk/network/resource failures;
malformed input; deterministic rendering; path/secret-like input redaction;
and guarded fixture-isolation processes. Its AST deny list and mutation probes reject
feature, membership, service, VM, switch, VHD, disk/partition, ACL, network,
registry, process, shutdown, restart, runner, and service mutation command
families in both qualification tools.

Windows CI invokes only that fixture/static/template contract. It does not run
the live mode, invoke Hyper-V, request elevation, create a VM, manipulate a
service or host feature, or produce real-platform acceptance evidence.
