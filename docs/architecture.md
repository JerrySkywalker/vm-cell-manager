# Architecture

VM Cell Manager is a daemonless local runtime for disposable full-system VM execution cells. The core owns lifecycle semantics and local ownership; providers own hypervisor-specific realization; guest transports own host-to-guest command/file movement.

## Top-level architecture

```text
                     vmcell
                       │
              Portable Rust Core
                       │
              Provider Interface
                       │
       ┌───────────────┼────────────────┐
       ▼               ▼                ▼
    Hyper-V           QEMU          future providers
    Provider         Provider
       │               │
       ▼        ┌──────┼──────┐
    Windows     ▼      ▼      ▼
               KVM    HVF    WHPX
              Linux   mac    Windows
```

The core selects or validates a local provider. It does not schedule across hosts and it does not delegate placement to a cloud controller.

## Internal modules

```text
┌─────────────────────────────────────────┐
│                  CLI                    │
│           human + --json output         │
├─────────────────────────────────────────┤
│              Cell Engine                │
│ lifecycle / ownership / recovery / TTL  │
├──────────────┬──────────────┬───────────┤
│ Image Store  │ Provider     │ Guest I/O │
│              │              │           │
│ image        │ Hyper-V      │ PS Direct │
│ variants     │ QEMU         │ QGA       │
│ COW overlay  │ future...    │ SSH       │
├──────────────┴──────────────┴───────────┤
│            Local State Store            │
│        manifests + lock + logs          │
└─────────────────────────────────────────┘
```

The implementation areas map to Rust modules rather than services:

- `cli`: human-facing CLI and versioned JSON output;
- `core`: provider-neutral cell/image/capability/lifecycle model;
- `engine`: Rust-owned mutation ordering, ownership proof, and reconciliation;
- `providers`: local hypervisor implementations;
- `guest`: PowerShell Direct, QGA, SSH, and future guest transports;
- `state`: local manifests, locks, ownership metadata, and logs;
- the cell engine composes `core`, `providers`, and `state` in-process rather than running as a daemon.

Provider-neutral `run` adds a read-only planning seam before this lifecycle:

```text
logical image variants + host/provider probes + support matrix + CLI/config preference
   -> versioned descriptive execution plan (authorizing=false)
   -> fresh engine image/provider/capability revalidation
   -> existing CellEngine lifecycle and authority issuance
```

The plan selects an exact provider, accelerator, and guest transport but grants
no provider authority. A fresh provider probe and exact plan/spec/image binding
must still pass before mutation; the normal immutable-image, ownership, and
provider/guest authority checks remain unchanged. See
[provider-neutral run selection](run-selection.md).

M2 guest actions use a second, narrower authority flow:

```text
CLI/library request + ephemeral credential
   -> CellEngine acquires the local mutation lock
   -> current installation + exact-owned running VM proof
   -> pinned state/runtime handles
   -> opaque GuestActionAuthority
   -> PowerShell Direct takes the provider mutex and rechecks by GUID
   -> readiness / exec / copy
   -> non-secret operation result + atomic artifact commit
```

Guest credentials are not durable state. Guest operation records intentionally
omit command arguments, output, raw transport errors, and authentication
material. An unknown operation is evidence of possible guest side effects, not
permission to replay it.

The M3 QEMU path preserves the same two-authority shape:

```text
CellEngine + current installation + pinned runtime
   -> provider-neutral mutation authority
   -> versioned QEMU definition and process receipt
   -> bounded QMP UUID/name proof
   -> lifecycle verb

CellEngine + exact-owned running QEMU snapshot
   -> GuestActionAuthority
   -> bounded credentialless QGA
   -> Linux guest exec/copy beneath the CellId workspace
```

The definition binds the immutable QCOW2 parent, exactly one overlay,
configuration and control endpoints, CPU/memory, explicit accelerator policy,
and the hash of every launch argument. Process ids and socket names are
recovery evidence, not standalone mutation authority.

## Control flow

The M1 create flow is:

```text
CLI request
   │
   ▼
validate CellSpec
   │
   ▼
acquire one local mutation lock
   │
   ▼
resolve and re-verify immutable Image Variant
   │
   ▼
write durable Creating ownership intent
   │
   ▼
create exactly one writable overlay
   │
   ▼
provider creates the smallest VM object and returns its immutable id
   │
   ▼
record provider id durably
   │
   ▼
claim by id, then configure networkless CPU state
   │
   ▼
reconcile complete ownership
   │
   ▼
Cell = stopped/ready
```

Durable intent precedes the first provider mutation. Destroy reverses the flow only after the local manifest, provider id, ownership marker, configuration path, disk attachment, and bounded VM configuration all agree. Foreign VM discovery or a name match never implies mutation authority.

## Why Provider and Guest I/O are separate

Hypervisor lifecycle and guest execution are orthogonal.

Examples:

- Hyper-V + Windows: VM lifecycle via Hyper-V, command execution via PowerShell Direct.
- Hyper-V + Linux: VM lifecycle via Hyper-V, command execution via SSH or another guest mechanism.
- QEMU + Windows/Linux: VM lifecycle via QMP, command execution via QGA or SSH.

This separation prevents provider code from accumulating application protocols and keeps command execution independent from virtual networking where possible.

## No lowest-common-denominator design

Provider differences are represented through capabilities rather than hidden.

The core should ask whether a provider offers a capability, not assume that every provider offers the same feature set. Examples include copy-on-write support, hardware acceleration, networkless guest execution, Secure Boot, TPM, nested virtualization, shared folders, GPU/device assignment, or snapshots.

Unsupported capability requests should fail explicitly.

## Local state and source of truth

VM Cell Manager is daemonless by default. Durable local state is expected to consist of versioned manifests plus provider-observed state.

A cell mutation should require both:

1. a local manifest proving `vmcell` ownership; and
2. provider identity that still matches the manifest.

M1 makes that rule concrete: the current persisted installation id, CellId, operation id, provider VM id, provider marker, configuration path, and single attached overlay are checked before start, stop, or destroy. Privileged provider verbs require an unforgeable engine-issued mutation authority, receive the already-proven provider snapshot, and revalidate it immediately before mutation while holding a cross-process provider mutex. Provider drift is reported without automatic adoption or repair.

Persisted installation, image, cell, and ownership schemas are rejected when their versions are unsupported. A missing installation identity is never recreated by a lifecycle authorization path.

On Windows, path identity treats ordinary drive and UNC paths as equivalent to their verbatim `\\?\` forms for provider reconciliation. This alias normalization is not used for containment: state and runtime creation/deletion separately reject reparse points throughout the existing ancestor chain, pin ordinary directory identities across provider use, and require the physical CellId directory to be a direct child of the physical runtime root. Persisted manifest filenames are also bound to their embedded IDs.

On Unix, vmcell state directories are current-user-owned `0700` directories
and state/configuration files are `0600`. Authority-bearing file and directory
opens use no-follow/close-on-exec flags and revalidate device/inode identity.
Linux exposes KVM only when both QEMU advertises it and the current identity can
open `/dev/kvm`; a compiled but inaccessible accelerator is not silently
selected.

The provider mutex coordinates `vmcell` processes; it cannot serialize unrelated Hyper-V tools. The Rust mutation guard also pins the state root and its `locks`, `images`, `cells`, and `runtime` children against replacement while an operation is active. Real-provider acceptance therefore requires an isolated host window with no concurrent external Hyper-V writer and exclusive, ACL-enforced control of the configured vmcell state root.

The state store is not intended to become a distributed database. If multi-host scheduling is required, it belongs above this project.

## Recovery philosophy

The normal rollback unit is the entire disposable cell:

```text
failure
  ↓
destroy owned cell
  ↓
new overlay from immutable base
  ↓
recreate
```

Deep snapshot/checkpoint trees are deliberately not the default abstraction. Provider-specific snapshots may be added later for narrowly justified use cases, but the core recovery contract remains destroy-and-recreate wherever possible.

## Application boundary

VM Cell Manager ends at a usable VM execution cell. Application-specific control stays outside:

```text
vmcell
  ↓
Windows/Linux VM
  ↓
application adapter / CLI / API / MCP
  ↓
MATLAB / STK / Ansys / GitHub runner / other tooling
```

That boundary allows this repository to remain useful without becoming an agent framework or engineering-software integration monolith.
