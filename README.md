# VM Cell Manager

`vmcell` is a Rust-first, daemonless local execution-cell runtime for disposable **full-system virtual machines** across native hypervisor backends.

> **Status:** pre-alpha / stacked M2 review candidate. M1 implements an ownership-checked Hyper-V lifecycle. M2 adds repository-local PowerShell Direct guest-control, artifact, TTL, and explicit-GC contracts. Real Hyper-V and guest acceptance remain separately gated and are not run by core CI.

The project is aimed at local development, CI, engineering software, and autonomous-tool workloads that need a clean, reproducible VM without turning a workstation into a cloud control plane.

## Why this project

There are already strong open-source VM tools. VM Cell Manager exists only for the gap that remains between them:

- [smolvm](https://github.com/smol-machines/smolvm) is a strong Rust microVM runtime, but its current guest model is Linux-oriented.
- [Multipass](https://github.com/canonical/multipass) provides excellent cross-platform Ubuntu/cloud-style instances, but is Ubuntu-centric and daemon-managed.
- [Vagrant](https://github.com/hashicorp/vagrant) is a mature environment-provisioning system, but has a broader configuration/provisioning product model.
- libvirt, QEMU, Hyper-V, Virtualization.framework, and similar systems are lower-level virtualization technologies rather than the narrow execution-cell abstraction this project targets.

VM Cell Manager focuses on a deliberately smaller intersection:

- full Windows and Linux guests as first-class workloads;
- native Hyper-V on Windows rather than forcing a portable backend everywhere;
- QEMU as the portable reference provider across KVM, HVF, and WHPX;
- immutable base images with disposable copy-on-write overlays;
- local ownership and predictable cleanup;
- machine-readable automation from day one;
- no required daemon, cluster, cloud scheduler, or agent framework.

If the project ever becomes merely another Linux microVM runtime, Ubuntu VM launcher, or Vagrant-style provisioning DSL, it should stop and reuse the existing tool that already solves that problem better.

## Architecture

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

Windows uses Hyper-V as the first-class native path. QEMU is the portable reference provider: KVM on Linux, HVF on macOS, and WHPX on Windows. A future provider must solve a problem that these two do not; provider count is not a goal by itself.

### Internal architecture

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

The separation between **Provider** and **Guest I/O** is intentional. Starting a VM and executing a process inside it are different concerns:

- Hyper-V + Windows can use PowerShell Direct without relying on guest networking.
- QEMU can use the QEMU Guest Agent.
- Linux and other reachable guests can use SSH.

A provider should not own application-level protocols such as MATLAB, STK, Ansys, GitHub Actions, or MCP. Those belong above `vmcell`.

## Core model

VM Cell Manager has five small domain concepts:

1. **Image** — a logical immutable guest environment.
2. **Image Variant** — a provider-specific artifact for that logical image, such as VHDX or QCOW2.
3. **Cell** — one disposable execution instance derived from an image.
4. **Provider Capability** — what the current host/provider can actually supply.
5. **Guest Transport** — how commands and files cross the host/guest boundary.

The disk model is deliberately shallow:

```text
Immutable Base
      │
      └── Writable Cell Overlay
```

For Hyper-V this maps naturally to a base VHDX plus a differencing VHDX. For QEMU it maps to a base QCOW2 plus a backing/overlay QCOW2. Deep checkpoint trees are not part of the normal cell model; destroy-and-recreate is the preferred rollback mechanism.

## Provider philosophy

The core does **not** reduce every hypervisor to the lowest common denominator. Providers advertise capabilities instead.

Examples of capability dimensions include:

- host and guest architecture;
- full-system VM support;
- hardware acceleration;
- copy-on-write overlay support;
- guest operating systems;
- guest execution transports;
- network-independent guest execution;
- later: TPM, Secure Boot, nested virtualization, shared folders, GPU/device capabilities.

Cross-architecture emulation is not treated as equivalent to hardware virtualization. A future `--allow-emulation` path must be explicit rather than silently turning an accelerated workload into a much slower emulated one.

## Current CLI

Read-only discovery remains available:

```text
vmcell doctor [--json]
vmcell provider list [--json]
```

The stacked M1-M3 branches expose:

```text
vmcell doctor
vmcell provider list
vmcell image add --id IMAGE --path BASE.vhdx --guest-os windows --provider hyperv
vmcell image add --id IMAGE --path BASE.qcow2 --guest-os linux --provider qemu
vmcell image list
vmcell image inspect IMAGE
vmcell create --image IMAGE --provider PROVIDER --cpu-count 2 --memory-mib 4096 [--accelerator POLICY] [--allow-tcg] [--ttl-seconds N]
vmcell list
vmcell inspect CELL_ID
vmcell start CELL_ID
vmcell stop CELL_ID
vmcell destroy CELL_ID
vmcell reconcile [CELL_ID]
vmcell exec CELL_ID --username USER --password-stdin -- PROGRAM [ARG...]
vmcell copy-in CELL_ID --source HOST_FILE --destination GUEST_PATH --username USER --password-stdin
vmcell copy-out CELL_ID --source GUEST_PATH --username USER --password-stdin
vmcell artifact collect CELL_ID --path GUEST_PATH --username USER --password-stdin
vmcell artifact inspect CELL_ID OPERATION_ID
vmcell operation list [CELL_ID]
vmcell operation inspect OPERATION_ID
vmcell operation reconcile OPERATION_ID
vmcell gc
```

All commands support global `--json` and `--state-root PATH` options. Guest
credentials are accepted only through bounded stdin and are never written to
state. Guest actions require a current installation identity, a pinned runtime,
and an exact-owned running VM rechecked by its provider identity. Windows uses
PowerShell Direct; M3 adds credentialless Linux QGA. Real QEMU/KVM, WHPX, and
HVF acceptance remain separate host gates.

## Safety and ownership

The local runtime is designed to be conservative around host state.

Initial implementation rules:

- do not automatically enable Hyper-V, KVM, or other host features;
- do not reboot the host;
- do not create or rewrite host-global virtual networking implicitly;
- do not mutate foreign VMs by default;
- only destroy a VM after ownership and local manifest identity agree;
- require an engine-issued, current-installation/runtime authority for every provider mutation;
- quarantine pre-ID/name-only crash remnants instead of adopting or deleting them;
- keep base images immutable after registration;
- make cleanup idempotent where practical;
- expose stable JSON output and explicit exit codes for automation.
- never persist guest credentials, command arguments, command output, or raw transport errors;
- never automatically replay an unknown or partially completed guest action;
- keep artifacts in a CellId/operation-bound, hash-verified state subtree;
- run TTL cleanup only through explicit `vmcell gc` and the existing exact-owned destroy path.

`vmcell doctor` is expected to report missing prerequisites rather than attempting to repair the host automatically.

## Platform direction

| Host | Primary provider | Accelerator / native path | Initial guest focus |
|---|---|---|---|
| Windows | Hyper-V | Hyper-V | Windows, Linux |
| Windows | QEMU | WHPX | Windows, Linux |
| Linux | QEMU | KVM | Linux, Windows |
| macOS | QEMU | HVF | Linux; other guests where technically and legally appropriate |

An Apple Virtualization.framework provider may be considered later if it provides clear value beyond QEMU/HVF. libvirt, VirtualBox, VMware, Parallels, and cloud providers are not bootstrap dependencies.

## Engineering workloads

A major motivation is the class of workloads that do not fit comfortably into Linux-only sandboxes:

- Windows GitHub Actions runners;
- Visual Studio and vendor Windows SDKs;
- MATLAB and STK workflows;
- Ansys and other engineering solvers;
- CAD/CAE environments where a complete Windows guest is required;
- autonomous coding or research tasks that should be able to damage a disposable VM without damaging the workstation.

VM Cell Manager does not automate those applications itself. It only supplies the execution cell in which their own APIs, CLIs, MCP servers, or automation adapters can run.

## Relationship to OpenStack

OpenStack is intentionally **not** a VM Cell Manager provider target in the current product model.

`vmcell` answers:

> Create and manage a disposable execution cell on **this local host**.

OpenStack answers:

> Turn a pool of compute, image, identity, networking, and placement services into a multi-host IaaS cloud.

Those are different layers. If a future orchestration system needs both, they should be peers:

```text
Higher-level execution/orchestration layer
├── vm-cell-manager    # local host execution
└── OpenStack          # cloud / multi-host execution
```

Adding cloud tenancy, remote placement, quotas, SDN, distributed image services, or cluster scheduling to this repository would violate its product boundary.

## Non-goals

VM Cell Manager is not intended to become:

- an OpenStack replacement;
- a multi-host scheduler;
- a distributed image registry;
- a general-purpose VM GUI;
- a Vagrant-compatible provisioning language;
- a container runtime;
- a Linux-only microVM sandbox;
- an MCP server or agent framework;
- a GitHub Actions implementation;
- a physical HIL/device controller.

Higher-level systems may compose `vmcell` with those capabilities without moving them into this repository.

## Project layout

```text
src/
├─ cli/            # human CLI and versioned machine-readable output
├─ core/           # cell, image, capability, lifecycle domain model
├─ engine/         # Rust-owned lifecycle, ownership, and reconciliation
├─ providers/      # Hyper-V, QEMU, future local VM providers
├─ guest/          # PowerShell Direct, QGA, SSH transports
└─ state/          # local manifests, locks, logs, ownership metadata
```

See [`docs/architecture.md`](docs/architecture.md) for the design contract and [`docs/roadmap.md`](docs/roadmap.md) for implementation phases.

## Roadmap

The first milestones are intentionally incremental:

- **M0 — Architecture bootstrap:** complete; read-only provider discovery, domain model, documentation, and local-first self-hosted Windows CI.
- **M1 — Hyper-V cell foundation:** implementation review candidate; image registration, single-level differencing disk, create/start/inspect/stop/destroy, and ownership reconciliation are implemented, with real-provider acceptance still gated.
- **M2 — Windows guest control:** stacked implementation candidate; PowerShell Direct exec and file transfer, TTL cleanup, and artifact collection are mock/static validated, while real guest acceptance remains gated.
- **M3 — QEMU provider:** QMP lifecycle, QCOW2 overlay, QGA transport, WHPX reference validation on Windows.
- **M4 — Linux/macOS portability:** KVM and HVF acceptance, packaging and path/state semantics.
- **M5 — automation hardening:** stable JSON schemas, exit-code contract, recovery and crash consistency.

Provider-specific capabilities such as TPM, Secure Boot, nested virtualization, GPU/device support, or additional native providers come only after the core lifecycle is stable.

## Development

Rust project code is intended to remain Rust-first across platforms. Hypervisor-specific integrations may call stable platform interfaces or vendor tools where that is the correct boundary; the project does not reimplement Hyper-V or QEMU.

Before submitting changes:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Core CI runs Rust checks on a dedicated self-hosted Windows runner with no Hyper-V mutation responsibilities. Real provider integration belongs on a separate explicitly privileged and isolated runner; the existing `core` runner must never be repurposed for it.

## License

Licensed under the [Apache License 2.0](LICENSE).
