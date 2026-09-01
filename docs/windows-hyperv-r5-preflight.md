# Windows Hyper-V R5 read-only preflight

`tools/windows-hyperv-preflight.ps1` is an admission evidence collector for
the v0.4.1 Windows Hyper-V / Windows guest / PowerShell Direct R5 packet. It is
not an authorization mechanism, does not claim functional acceptance, and does
not change the support matrix.

Every result has `authority: "none"`, `acceptance: false`, `authorizing: false`,
`support_status: "untested"`, and `real_platform_acceptance: "not_started"`.
`PREFLIGHT_ELIGIBLE` only means that
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
  -CandidateChecksumManifestPath REQUIRED_CHECKSUM_MANIFEST_PATH `
  -CandidateBinaryPath REQUIRED_BINARY_PATH `
  -VhdxPath REQUIRED_VHDX_PATH `
  -ProvenancePath REQUIRED_PROVENANCE_PATH `
  -StateRoot REQUIRED_ORDINARY_C_NTFS_STATE_ROOT
```

No fixture, CI result, or live preflight result is R5 real-platform acceptance.
That status remains `NOT_STARTED` until a distinct, explicitly authorized
real-platform goal completes.

## Evidence and fail-closed behavior

The result contains a fixed observation register covering elevation and local
administrators membership; Windows build and architecture; Hyper-V feature,
module, cmdlets, VMMS, and read access; VM and switch inventories; competing
virtualization writers and runner/Codex activity; C: and V: storage boundaries;
immutable VHDX state; provenance and candidate/package/binary/VHDX hashes;
receipt freshness; and exclusive-window eligibility.

`unavailable` is always emitted as an `evidence_gap.*` blocker. It is never
treated as pass. A failed precondition becomes `precondition_failed.*`; the
owner action names the evidence or external remediation needed. The result
stores only codes, statuses, and SHA-256 evidence digests—never raw host paths,
VM names, command lines, credentials, or process identities.

The live V: check rejects ReFS and `File Backed Virtual` storage as unsuitable
for the R5 boundary. The tool also rejects a missing, dynamic, writable,
attached, differencing, or parented VHDX. It leaves all such evidence untouched.
Before live observation uses provenance, it reads one UTF-8 JSON snapshot,
rejects a reparse-point file or parent, and compares the before/after content
digest. The state root, VHDX, candidate package, and candidate binary also
must each be ordinary non-reparse paths with non-reparse ancestors. A missing,
changed, or unsafe receipt or path becomes an evidence gap and can never
produce `PREFLIGHT_ELIGIBLE`. The live result digest is derived from its ordered
sanitized observations, not wall-clock time.

### Raw JSON provenance boundary

Fixture and Live inputs use the same raw UTF-8 JSON boundary before PowerShell
materializes an object. It rejects malformed input; scalar, array, and null
roots; and duplicate or case-conflicting member names at every object depth.
The fixture receipt returns only `fixture.schema_invalid` for a raw-input
failure. Provenance unknown members return a stable
`provenance.unknown_property_count.N` blocker: neither form reflects a member
name, member value, JSON fragment, private path, or parser exception.

The closed-world provenance packet is the following semantic group set:
`schema_version`, `contract`, `authority`, `acceptance`, `authorizing`,
`admission_result`, `admission_status`, `real_platform_acceptance`,
`support_status`, `admitted_for_candidate_sha`, `candidate`, `package`,
`candidate_binary`, `windows`, `hyperv`, `image_source`, `vhdx`, `creation`,
`immutability`, `admission_receipt`, `exclusive_window`, and
`license_evaluation`. In particular, `image_source.reference` must be a
positive `SRC-` source reference and `creation.created_by_evidence_id` must be
a positive `EVID-` creator reference. Both are Unicode-normalized and
surrounding-whitespace-normalized before a token-boundary placeholder policy
rejects `REQUIRED`, `REQUIRED_OWNER`, `TODO`, `TBD`, `CHANGE_ME`, `SAMPLE`,
`EXAMPLE`, `DUMMY`, `UNKNOWN`, and `NOT_EXECUTED` variants. The policy does not
use a substring match, so syntactically valid opaque evidence identifiers such
as `EVID-SAMPLER-CREATOR-20260902` remain valid.

## Provenance template

Start with
[`windows-hyperv-image-provenance-template.json`](receipts/windows-hyperv-image-provenance-template.json).
The v2 template deliberately contains placeholders, not admitted evidence. The
shared fixture/Live validator rejects a missing, null, wrong-type, empty,
whitespace, `REQUIRED_`, `TODO`, `TBD`, `FIXME`, or unsupported `UNKNOWN` value;
it also rejects unknown JSON properties and candidate/package/binary/VHDX identity
mismatches. A completed owner packet binds the exact candidate, frozen release,
archive, checksum manifest, binary, VHDX digest/size, and canonical-path digest;
Windows Server 2022/Generation 2/Secure Boot/PowerShell Direct expectations; and
sanitized source, preparation, immutability, license, and receipt evidence.

The packet contains no public raw path. `credentials_embedded` is a typed owner
attestation, not an automated `false` claim. The documented
`UNKNOWN_REQUIRES_OWNER_ATTESTATION` enum is syntactically permitted but keeps
the result blocked with a typed owner action. Template values therefore cannot
pass through unchanged or promote an admission result.
The non-executing packet instructions are in
[`windows-hyperv-r5-image-preparation.md`](windows-hyperv-r5-image-preparation.md).

## Safety checks and CI

`tools/test-windows-hyperv-preflight.ps1` uses the test-owned
`provenance-contract-matrix.json`, not the production required-field collection,
as its deterministic golden corpus. It compares the independent 63-field matrix
separately with the template, production raw parser and semantic validator,
fixture, Rust template test, and this documentation. It also exercises raw
duplicate/case-conflict input, malformed/scalar roots, placeholder metamorphs
and safe near misses, sanitized unknown-property receipts, restored path and
observation regressions, and fixture isolation. Its AST scan rejects feature,
membership, service, VM, switch, VHD, disk/partition, ACL, network,
process-termination, shutdown, restart, runner, and service mutation command
families in the production tool.

Windows CI invokes only that fixture/static/template contract. It does not run
the live mode, invoke Hyper-V, request elevation, create a VM, manipulate a
service or host feature, or produce real-platform acceptance evidence.

This R1 contract deliberately excludes the R2 static-policy redesign, R3
stopped-cell/storage policy, and R4 broader documentation/CI alignment. It does
not qualify the frozen release, begin real-platform acceptance, or promote
support.
