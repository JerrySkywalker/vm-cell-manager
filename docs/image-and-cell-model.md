# Image and Cell Model

The product object is an execution **Cell**, not a long-lived virtual machine. A Cell is created from an immutable logical Image and is expected to be disposable.

## Logical image

An Image describes guest identity independently of one hypervisor format.

Example:

```yaml
id: windows-dev-2026-08
guest_os: windows
guest_arch: x86_64
variants:
  - provider: hyperv
    disk_format: vhdx
    path: images/windows-dev-2026-08/base.vhdx
  - provider: qemu
    disk_format: qcow2
    path: images/windows-dev-2026-08/base.qcow2
```

A logical image may have one or more provider variants. Provider artifacts do not need to be byte-compatible with one another.

## Immutability

Once an image variant is registered as a base, normal cell operations must treat it as immutable. The implementation should record enough identity information to detect accidental replacement or mutation before deriving a new cell.

The first practical implementation should prefer simple strong checks over clever reconciliation.

## Disposable overlay

The normal storage shape is exactly one writable layer:

```text
base image
    ↓
cell overlay
```

Examples:

```text
Hyper-V: base.vhdx -> cell.vhdx (differencing)
QEMU:    base.qcow2 -> cell.qcow2 (backing overlay)
```

The core should not require identical disk formats, snapshot semantics, or block APIs across providers.

## Cell identity

Every managed Cell receives a generated `CellId` and a provider identity. The local manifest should eventually record at least:

```text
schema_version
cell_id
owner/install identity
provider
provider_object_id
image/image_variant identity
cpu_count
memory_mib
created_at
expires_at
state
paths owned by the cell
```

Provider object names should include or be strongly associated with the CellId so recovery can reconcile manifests with the hypervisor.

## Ownership rule

A provider-visible VM is not automatically a VM Cell Manager resource.

Destructive mutation requires positive ownership evidence. Normal commands should not adopt or delete foreign VMs merely because their name resembles a `vmcell` object.

Future explicit import/adoption functionality, if ever added, must be a separate high-friction operation.

## Lifecycle

The initial state machine is intentionally small:

```text
Creating -> Stopped -> Running
    │          │          │
    │          └────┐     │
    │               ▼     ▼
    └──────────> Failed  Destroying -> Destroyed
                   │
                   └────> Destroying
```

`Destroyed` is terminal.

The exact provider sequence may differ, but provider implementation must not bypass core lifecycle validation when writing durable state.

## TTL and garbage collection

TTL is an execution-cell convenience, not a background cloud service. The daemonless design means expiration alone does not imply a continuously running reaper.

Initial semantics should be explicit:

- `vmcell gc` finds expired owned cells and applies normal safe-destroy rules;
- future optional OS scheduling may invoke `vmcell gc`, but scheduling is not required for core correctness;
- expiration never grants authority to delete a foreign VM.

## Artifacts

The VM's writable overlay is not itself the durable output contract. Workloads should copy intended results out through a guest transport or shared artifact path before destroy.

This keeps cell destruction cheap and makes the difference between execution state and user artifacts explicit.
