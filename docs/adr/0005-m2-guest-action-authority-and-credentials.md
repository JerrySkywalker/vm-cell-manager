# ADR 0005: M2 guest action authority and ephemeral credentials

- Status: Accepted
- Date: 2026-08-09

## Context

M2 adds PowerShell Direct guest execution and file movement. A bare CellId, VM
name, or transport session is not ownership authority. Guest credentials are
also materially more sensitive than the versioned local state used by M1.

## Decision

`CellEngine` remains the only component that can authorize a guest action. It
constructs an opaque `GuestActionAuthority` only while all of these remain
true:

- the current installation identity matches the cell manifest;
- the provider GUID, CellId-derived name, ownership marker, configuration path,
  single overlay, networking, CPU, and memory exactly reconcile;
- the provider reports the VM running and the cell is in the ready phase;
- ordinary state/runtime identities are pinned; and
- the local mutation lock is held for the complete action.

`GuestTransport` receives that authority plus the already-proven provider
snapshot. PowerShell Direct takes the same cross-process provider mutex as M1,
rechecks the complete snapshot by GUID immediately before creating a session,
and never treats a VM name as authority.

Credentials use a dedicated non-serializable Rust value. The CLI accepts the
password only from stdin. The PowerShell child receives a length-framed stdin
channel in which the non-secret action JSON and credential bytes are separate.
Passwords and tokens never enter argv, environment variables, state manifests,
operation records, logs, errors, receipts, or PR evidence. Credential and wire
buffers are zeroed when dropped.

Readiness polling, deadlines, output limits, and retry decisions remain in
Rust. A transport process timeout kills the owned host child and returns an
unknown/recovery-required classification; it never implies that a guest process
was stopped. Nonterminal or unknown operations are never automatically replayed.

## Consequences

- Provider lifecycle and guest I/O remain separate abstractions.
- Library callers cannot bypass M1 proof by directly invoking the built-in
  transport.
- Credential/session/readiness failures can be classified without persisting
  raw child stderr or secrets.
- PowerShell remains a fixed compatibility shell, not a second orchestration
  state machine.
- QGA and SSH remain explicit unsupported stubs in M2.

