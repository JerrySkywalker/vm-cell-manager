# VMCell owner-attended Windows Hyper-V acceptance plan

Status: **OWNER APPROVED IMPLEMENTATION BASELINE**  
Contract: `vmcell.owner-attended-hyperv-acceptance-plan.v1`  
Authorizing: **false until a later exact owner goal**

## 1. Boundary

This document describes a future owner-attended sequence. It is not authority
to run it. A later goal must bind the exact candidate, image, paths, host,
operator, exclusive window, expected foreign prestate, and evidence directory.

The real run is intentionally separate from unattended repository development
because it may create, start, stop, and remove exact-owned Hyper-V resources.
It must never repair the host, stop a runner, change a switch, alter Windows
features, or prepare an image as an incidental action.

## 2. Required owner inputs

Before authorization, the owner supplies or approves:

- dedicated Windows x86_64 host and bounded attended window;
- exact frozen candidate/package/binary identities;
- Windows Server 2022 x86_64 Generation-2-compatible fixed VHDX;
- legitimate image source and human-reviewed licensing/evaluation metadata;
- image provenance, exact size, SHA-256, parentless/detached/read-only state;
- ordinary private non-reparse local NTFS roots for state, runtime, image,
  package, and binary;
- sufficient free capacity for the fixed base, overlays, VM configuration,
  artifacts, evidence, and rollback margin;
- one guest local user profile and valid credentials available for interactive
  stdin entry;
- fresh foreign VM/switch/process inventory and an exclusive writer window;
- a new ordinary non-reparse evidence directory that contains no prior receipt.

Credentials, product keys, media tokens, and private paths are never committed,
placed in argv or environment variables, or written to receipts.

## 3. Image preparation gate

Image preparation occurs outside VMCell under separate owner supervision.

Required final state:

- guest: Windows Server 2022 x86_64;
- format: VHDX;
- intended VM generation: Generation 2;
- firmware policy: Secure Boot using the normal Windows template;
- disk type: fixed;
- parent: none;
- attachment: detached;
- filesystem item: ordinary, non-reparse, read-only;
- guest: at least one configured local profile suitable for PowerShell Direct;
- embedded credentials: owner-attested status, never guessed by automation;
- bytes: exact size and SHA-256 recorded after final detach;
- provenance: candidate, source, creation method, Windows edition/build,
  security properties, timestamps, licensing constraints, and admission status.

The provenance statement that an image is Generation-2-compatible is not proof
of the VM's eventual firmware configuration. That is observed after VM creation
and before first start.

Forbidden image sources include a daily development VHDX, an attached foreign
VM disk, a differencing child, a parented disk, an image containing operational
secrets, or bytes acquired without legitimate owner control.

## 4. Preauthorization read-only admission

Before any real mutation:

1. verify the exact merged qualification tooling and its CI;
2. verify candidate, archive, binary, version, image, and provenance hashes;
3. verify ordinary non-reparse ancestry and path-derived volume/backing policy
   for each operational root;
4. verify capacity and rollback margin;
5. verify elevated effective identity and Hyper-V Administrators capability;
6. verify Hyper-V feature/module/cmdlets, VMMS, and provider read access;
7. record sanitized counts/fingerprints for foreign VMs and switches;
8. verify no active VMCell, runner worker, Codex writer, image writer, or other
   virtualization writer conflicts with the window;
9. run the non-authorizing Live preflight once;
10. require `PREFLIGHT_PASS` with no evidence gap.

An unavailable observation is a block. It is never inferred as PASS.

## 5. Authorized sequence design

The later owner goal should authorize the following stages individually. Each
stage is a commit point: the next stage begins only when the current evidence is
complete.

### R0 — final prestate

- Recheck candidate, image hash, image read-only state, exclusive window, and
  foreign inventory immediately before mutation.
- Allocate one fresh exact-owned cell namespace.
- Confirm the evidence directory is empty and non-overwriting.

### R1 — image registration

- Register only the exact admitted image.
- Inspect the registered identity and reject any path/hash/format mismatch.
- Do not import or modify image bytes.

### R2 — stopped cell

- Create exactly one stopped networkless VM through VMCell.
- Prove one differencing overlay with the exact base parent.
- Read back and bind VM ID, owned name/marker, configuration path, disk path,
  memory, CPU count, and stopped state.
- Independently observe Generation 2, Secure Boot enabled with the expected
  template, exactly one expected disk, and zero network adapters.
- If any invariant differs, do not start the VM. Preserve the exact-owned
  object for the documented cleanup or manual-review branch.

### R3 — first start and readiness

- Recheck complete ownership immediately before start.
- Start only the exact-owned stopped VM.
- Wait within the bounded readiness timeout.
- Probe PowerShell Direct using interactively supplied guest credentials.
- Classify guest-not-ready, authentication failure, session failure, timeout,
  and unknown response separately.

### R4 — minimum useful command

- Execute a harmless bounded command that produces known stdout, known stderr,
  and a known zero exit code.
- Verify output encoding, byte counts, output limit, timeout handling, and
  operation receipt correlation.
- Do not proceed if output is truncated, mismatched, or ambiguously attributed.

### R5 — functional owner preview

- Execute one known nonzero command and preserve its typed result without
  treating nonzero as transport failure.
- Copy one bounded non-secret file into the guest.
- Copy one bounded non-secret file out and verify its SHA-256.
- Collect one bounded artifact and verify operation/artifact correlation.
- Repeat the admitted workflow until three sequential fresh-identity runs have
  completed. Do not reuse prior cell authority.

### R6 — recovery and reconciliation

- Exercise exactly one preselected bounded failure or interruption case.
- Record durable intent and observed effect.
- Do not replay an unknown-effect operation.
- Reconcile from current exact provider and durable state.
- Quarantine ambiguity for owner decision rather than adopting or deleting it.

### R7 — exact-owned cleanup

- Stop only the exact-owned VM after a fresh full ownership check.
- Destroy only the exact-owned VM and disposable overlay/runtime objects.
- Repeat destroy to prove idempotent absence behavior.
- Never delete by name alone and never touch foreign VMs, disks, switches, or
  processes.

### R8 — poststate and receipt

- Prove VM, runtime, overlay, and transient endpoints are absent.
- Recompute the immutable base hash and require equality.
- Reobserve host feature, service, switch, and foreign inventory fingerprints.
- Produce sanitized receipts with opaque evidence IDs and digests.
- Keep raw host and guest output outside Git.

## 6. Mutation envelope

| Family | Permitted in later exact goal | Never permitted |
| --- | --- | --- |
| VM | One exact-owned create/start/stop/remove sequence | Foreign or ambiguous VM mutation |
| VHDX | One disposable differencing overlay | Base modification, mount, resize, convert, optimize, attach as foreign storage |
| Network | Remove/retain zero adapter only through reviewed VMCell behavior | Switch, adapter, firewall, route, DNS, or host-network changes |
| Service/feature | Read-only observation | Start/stop/restart/reconfigure VMMS, runner, Windows features, or services |
| Credentials | Interactive bounded stdin | argv, environment, receipts, logs, state, Git |
| Cleanup | Exact-owned, identity-rechecked resources | Name-only, path-only, PID-only, or foreign cleanup |

## 7. Immediate stop conditions

Stop the mutation sequence on:

- source, package, binary, image, or provenance mismatch;
- base image writable, attached, parented, reparse-backed, or changed;
- unexpected Generation, Secure Boot, disk parent, network adapter, VM state,
  ownership marker, configuration path, CPU, or memory;
- access denied or unavailable required observation;
- active competing writer or loss of exclusive window;
- guest credential, readiness, session, timeout, output, copy, or artifact
  ambiguity;
- unknown effect or durable-state disagreement;
- failure to prove exact-owned cleanup;
- foreign VM, switch, service, feature, process, or inventory change;
- suspected secret or private-host disclosure.

The correct response is preserve evidence and stop. Do not repair, replay,
restart services, alter the host, or widen authority.

## 8. Expected duration and attendance

| Stage | Expected attended time |
| --- | ---: |
| Final admission | 1–2 hours |
| First stopped cell and firmware proof | 1 hour |
| First start and PowerShell Direct | 1–2 hours |
| Functionality and repetition | 2–3 hours |
| Recovery, cleanup, and poststate | 1–2 hours |
| Total | 6–10 hours |

The owner should remain available for UAC, guest credential entry, evidence
review, and every decision to proceed after a stage boundary.

## 9. Result ceiling

Allowed real-run results:

- `OWNER_LOCAL_PREVIEW_PASS`;
- `PARTIAL_RESUMABLE`;
- `BLOCKED_EXTERNAL`;
- `OWNER_DECISION_REQUIRED`;
- `FAIL_SAFETY_OR_CORRECTNESS`.

Even `OWNER_LOCAL_PREVIEW_PASS` does not publish a release or automatically
change a support row. Those require a later, independent governance decision.
