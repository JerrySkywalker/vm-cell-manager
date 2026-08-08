# ADR 0004: M1 ownership and provider mutation

- Status: Accepted
- Date: 2026-08-08

## Context

M1 introduces the first host-mutating provider operations. Hyper-V objects are shared host resources, so a generated name alone cannot distinguish an owned disposable cell from foreign state. External VM creation and local manifest persistence also cannot be one atomic filesystem transaction.

## Decision

Rust owns the lifecycle state machine. Before the first provider mutation it writes a versioned `Creating` manifest containing the installation id, CellId, operation id, intended provider name, configuration path, overlay path, and immutable image identity.

Hyper-V integration uses fixed, single-purpose Windows PowerShell scriptlets with structured JSON input/output. `New-VM` returns the immutable provider id before marker, networking, and CPU configuration; Rust persists that id and then drives bounded claim and configuration actions. PowerShell performs provider verbs and validates Rust-supplied expected-state preconditions but does not select recovery actions or maintain durable lifecycle state.

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
- Once the provider id is durable, incomplete claim/configuration is classified as bounded provisioning state; pre-id or pre-marker ambiguity remains terminal and is never destroyed by name.
- Lifecycle authorization binds the cell marker to the current persisted installation identity, and unsupported state schemas fail before provider or runtime mutation.
- Runtime creation/deletion rejects redirected ancestor chains; Windows provider path aliases are normalized only for identity comparison, never to weaken containment.
- Exact-owned destroy is idempotent and removes only the recorded VM plus the CellId-scoped runtime directory.
- Base VHDX identity is reverified and held read-only while its single differencing child is created.
- QEMU mutation, Guest I/O, TTL/GC, daemons, host feature enablement, reboot, and virtual-switch mutation remain outside M1.
- Real Hyper-V mutation testing requires separate explicit authorization and a dedicated integration host; the core CI runner remains read-only infrastructure.
