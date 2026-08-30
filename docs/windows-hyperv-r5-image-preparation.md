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
binary digests, Windows Server 2022 edition/build and x86_64 architecture, a sanitized image-source
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

The state root, runtime root, image, package, binary, and completed provenance
file are separate operational roles. Each must be placed under an already
existing ordinary non-reparse local NTFS boundary and independently satisfy
the path-derived backing and capacity-plus-rollback policy. Sanitized evidence
output is a separate role and cannot authorize operational storage.

The future guest local profile may be referenced only through sanitized owner
attestation. Its password is supplied to VMCell only as one bounded stdin line;
no credential, product key, token, account identifier, or private path may be
placed in Git, argv, environment, VMCell state, receipts, or logs.

## Status

Fixture validation and CI only verify the template and preflight contract.
They do not create or inspect a VHDX, access Hyper-V, or promote the Hyper-V
support status. `REAL_PLATFORM_ACCEPTANCE=NOT_STARTED` until a distinct
explicitly authorized real-platform goal completes.
