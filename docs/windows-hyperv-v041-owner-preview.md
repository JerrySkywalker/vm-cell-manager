# Windows Hyper-V v0.4.1 owner-preview guide

Status: **tooling contract only**  
Real-platform acceptance: **NOT_STARTED**  
Support status: **untested**

This guide binds a future owner-local preview to the frozen VMCell `0.4.1`
candidate. It does not authorize Live preflight, elevation, image preparation,
Hyper-V access, VM creation or start, PowerShell Direct, or any host change.
Those actions require later owner-attended goals with exact inputs and a
bounded window.

## Frozen candidate

The only admitted candidate for this guide is:

```text
release_ref=release/v0.4.1
release_sha=0e7fcf37f4310562d318f9d5c709ddf8e8ca1637
version=0.4.1
windows_binary_sha256=249db6841161d634449142584ad7924b26cbe7b31a41eca9b813dd2eb8acec1b
```

Before a future owner window, re-hash the exact package, binary, image, and
provenance packet. A mismatch blocks the preview; do not relabel another binary
as `0.4.1`.

## Exact guest and firmware target

The narrow target is one owner-controlled Windows Server 2022 x86_64 image in
a Hyper-V Generation 2 VM. The VM must be stopped and must independently prove
Secure Boot enabled with the expected Windows template before its first start.
The image manifest's generation or Secure Boot statements are not substitutes
for this read-back.

The stopped cell must also prove exact VM ID and admitted name, one expected
differencing disk whose parent is the exact immutable parentless base, expected
CPU and startup memory, and zero network adapters. Use the read-only helper
described in [the R5 preflight contract](windows-hyperv-r5-preflight.md); any
unavailable evidence or mismatch blocks start authority.

## Operational and evidence paths

Supply state root, runtime root, image, package, binary, and provenance as
independent roles. Each operational role must already exist and resolve through
ordinary non-reparse ancestry to one unambiguous fixed local NTFS volume backed
by an admitted local disk. Each role must independently meet its declared
capacity plus rollback margin. State and runtime may use different admitted
local volumes.

The sanitized receipt is a separate evidence role. Its parent must already
exist and be ordinary and non-reparse; its leaf must not exist. The tooling
creates the leaf with create-new semantics and refuses overwrite. Evidence
placement never grants operational storage authority.

Never place raw VM names, private paths, host inventory, runner or process
details, guest output, command lines, account identifiers, or credentials in
Git. Keep raw owner evidence outside the repository and publish only opaque IDs,
statuses, and digests.

## Credential boundary

PowerShell Direct authentication is permitted only in a later owner-attended
goal. VMCell requires `--password-stdin`; the secret is exactly one UTF-8 line,
bounded to 4096 bytes. Enter it directly on the attached standard input only
after reviewing the exact command. Do not place the password in Git, argv,
PowerShell variables used to compose commands, environment variables, config,
VMCell state, receipts, transcripts, logs, shell history, or Codex prompts.

The required non-secret guest-local username selector may be supplied as the
reviewed `--username REQUIRED_GUEST_LOCAL_USER` argument. Do not replace the
placeholder in repository material. Do not use `Get-Credential`, a credential
file, an environment variable, or a pipeline that materializes the password in
a command or transcript.

The future invocation shape is:

```powershell
& REQUIRED_EXACT_VMCELL_EXE `
  exec REQUIRED_EXACT_CELL_ID `
  --username REQUIRED_GUEST_LOCAL_USER `
  --password-stdin `
  -- REQUIRED_BOUNDED_PROGRAM REQUIRED_REVIEWED_ARGUMENTS
```

Start the process without a transcript, then supply one password line directly
on standard input when VMCell reads it. The placeholder command is not
authorization and must be rebound to the exact owner packet.

## Owner-preview sequence boundary

1. Verify the frozen candidate and all offline package/image/provenance hashes.
2. Under a separate attended read-only goal, run the corrected role-aware Live
   preflight once and require complete `PREFLIGHT_ELIGIBLE` evidence.
3. Obtain a new explicit real-platform goal before any VM mutation.
4. After exact-owned cell creation, run stopped-cell qualification before start.
5. Start only if the later goal authorizes it and every stopped-cell observation
   is `pass`.
6. Use PowerShell Direct with the bounded-stdin secret channel for the smallest
   harmless readiness and command checks first.
7. Continue through bounded streams, exit status, copy, artifact, repetition,
   recovery, reconciliation, and exact-owned cleanup only while prior evidence
   remains valid.
8. Stop on mismatch, unavailable evidence, unknown effect, or ownership
   ambiguity. Never repair, blindly replay, adopt, or delete ambiguous state.

## Result ceiling

Fixture and Hosted Windows/Linux success prove repository correctness only.
Live preflight, if later authorized, has a `PREFLIGHT_ELIGIBLE` ceiling and is
not real-platform acceptance. Only a separately authorized owner-attended run
and fresh independent closeout may produce `OWNER_LOCAL_PREVIEW_PASS` for the
exact tuple.

Nothing in this guide changes a support row, frozen v0.4.1 acceptance record,
`main`, the release branch, or a release asset. It makes no public-support or
real-acceptance claim.
