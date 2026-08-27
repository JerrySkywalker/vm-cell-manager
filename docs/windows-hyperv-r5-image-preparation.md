# Windows Hyper-V R5 image-preparation packet

This document defines only the evidence packet that a separately authorized
owner may prepare before an R5 Windows Hyper-V acceptance window. It does not
authorize image preparation, provider access, elevation, VM/VHDX operations,
network configuration, or a real-platform run.

## Required packet

Begin with
[`windows-hyperv-image-provenance-template.json`](receipts/windows-hyperv-image-provenance-template.json).
Replace each `REQUIRED_` placeholder only with sanitized evidence from the
authorized owner process. Keep raw host paths, ISO locations, product keys,
tokens, account identifiers, command lines, and credentials outside the
repository and outside this template.

The completed packet must bind one exact candidate SHA and version, package and
binary digests, Windows edition/build/architecture, a sanitized image-source
reference and digest, and a fixed detached parentless VHDX digest. It also
records non-authorizing immutability, receipt-freshness, and exclusive-window
evidence. `authority` remains `none`, `acceptance` remains `false`,
`real_platform_acceptance` remains `not_started`, and support remains
`untested`.

## Evidence boundary

The packet is evidence input, not a launch configuration. Its completed copy
must be stored under an ordinary non-reparse parent chosen by the authorized
owner. The preflight reads a single UTF-8 snapshot only after confirming that
the file and every parent are not reparse points; it compares content hashes
before and after the read to reject a replacement race.

Any absent field, stale receipt, unsafe parent, changed snapshot, missing
digest, or incomplete exclusivity evidence is an evidence gap. It must remain
blocked for separate owner action, never be repaired, inferred, or relabelled
as real-platform acceptance by this tooling.

## Status

Fixture validation and CI only verify the template and preflight contract.
They do not create or inspect a VHDX, access Hyper-V, or promote the Hyper-V
support status. `REAL_PLATFORM_ACCEPTANCE=NOT_STARTED` until a distinct
explicitly authorized real-platform goal completes.
