# ADR 0004: M1 ownership and provider mutation

- Status: Accepted
- Date: 2026-08-08

## Context

M1 introduces the first host-mutating provider operations. Hyper-V objects are shared host resources, so a generated name alone cannot distinguish an owned disposable cell from foreign state. External VM creation and local manifest persistence also cannot be one atomic filesystem transaction.

## Decision

Rust owns the lifecycle state machine. Before the first provider mutation it writes a versioned `Creating` manifest containing the installation id, CellId, operation id, intended provider name, configuration path, overlay path, and immutable image identity.

Hyper-V integration uses fixed, single-purpose Windows PowerShell scriptlets with structured JSON input/output. PowerShell performs provider verbs but does not select recovery actions or maintain durable lifecycle state.

Start, stop, and destroy require agreement across:

- the local cell manifest;
- the recorded Hyper-V VM id and name;
- the provider ownership marker;
- the owned configuration path;
- exactly one attached differencing VHDX at the recorded path;
- zero network adapters;
- the recorded CPU and memory bounds.

Name-only discovery is classified as unproven and is never mutated. Reconciliation is read-only in M1. A narrow OS/filesystem mutation lock serializes local state changes; it is not a distributed lease or orchestration abstraction.

## Consequences

- A crash can leave an incomplete manifest, overlay, or provider object, but ambiguity fails closed instead of adopting or deleting it.
- Exact-owned destroy is idempotent and removes only the recorded VM plus the CellId-scoped runtime directory.
- Base VHDX identity is reverified and held read-only while its single differencing child is created.
- QEMU mutation, Guest I/O, TTL/GC, daemons, host feature enablement, reboot, and virtual-switch mutation remain outside M1.
- Real Hyper-V mutation testing requires separate explicit authorization and a dedicated integration host; the core CI runner remains read-only infrastructure.
