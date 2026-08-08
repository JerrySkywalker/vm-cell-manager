# ADR 0004: M1 ownership and provider mutation

- Status: Accepted
- Date: 2026-08-08

## Context

M1 introduces the first host-mutating provider operations. Hyper-V objects are shared host resources, so a generated name alone cannot distinguish an owned disposable cell from foreign state. External VM creation and local manifest persistence also cannot be one atomic filesystem transaction.

## Decision

Rust owns the lifecycle state machine. Before the first provider mutation it writes a versioned `Creating` manifest containing the installation id, CellId, operation id, intended provider name, configuration path, overlay path, and immutable image identity.

Hyper-V integration uses fixed, single-purpose Windows PowerShell scriptlets with structured JSON input/output. `New-VM` returns the immutable provider id before marker, networking, and CPU configuration; Rust persists that id and then drives bounded claim and configuration actions. PowerShell performs provider verbs and validates Rust-supplied expected-state preconditions but does not select recovery actions or maintain durable lifecycle state.

Provider mutators require an engine-issued authority token that cannot be constructed by a library caller. The token borrows a pinned current-installation handle and pinned ordinary state/runtime directory handles, and binds one CellId, marker, configuration path, and overlay path. The provider rejects a request that does not match that authority before launching PowerShell.

Start, stop, and destroy require agreement across:

- the local cell manifest;
- the recorded Hyper-V VM id and name;
- the provider ownership marker;
- the owned configuration path;
- exactly one attached differencing VHDX at the recorded path;
- zero network adapters;
- the recorded CPU and memory bounds.

Name-only discovery is classified as unproven and is never mutated. Reconciliation is read-only in M1. A narrow OS/filesystem mutation lock serializes local state changes; it is not a distributed lease or orchestration abstraction. Mutating PowerShell scriptlets additionally hold one process-external Hyper-V provider mutex and refresh the full expected VM snapshot immediately before the bounded verb. Real-provider admission must exclude non-`vmcell` Hyper-V writers because Hyper-V does not expose an atomic ownership-check-and-verb transaction.

## Consequences

- A crash can leave an incomplete manifest, overlay, or provider object, but ambiguity fails closed instead of adopting or deleting it.
- Once the provider id and exact creation receipt are durable, an interrupted claim may be retried by id and the object may then be destroyed. Claimed but partially configured objects use a narrower exact-owned destroy proof. Pre-id/name-only ambiguity remains terminal quarantine and is never automatically deleted.
- Lifecycle authorization binds the cell marker to the current persisted installation identity, and unsupported state schemas fail before provider or runtime mutation.
- Installation identity and ordinary state/runtime directories remain pinned with no-delete handles across provider mutation. Runtime creation/deletion rejects redirected ancestor chains; Windows provider path aliases are normalized only for identity comparison, never to weaken containment. Recursive removal relies on Rust's handle-relative, reparse-safe Windows implementation while the physical runtime parent remains pinned.
- Persisted image/cell IDs must match their manifest filenames; manifest files and existing ancestors must be ordinary non-reparse objects.
- Exact-owned destroy is idempotent and removes only the recorded VM plus the CellId-scoped runtime directory.
- Base VHDX identity is reverified and held read-only while its single differencing child is created.
- QEMU mutation, Guest I/O, TTL/GC, daemons, host feature enablement, reboot, and virtual-switch mutation remain outside M1.
- Real Hyper-V mutation testing requires separate explicit authorization and a dedicated integration host; the core CI runner remains read-only infrastructure.
