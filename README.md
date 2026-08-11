# VM Cell Manager

`vmcell` is a Rust-first, daemonless local execution-cell runtime for disposable **full-system virtual machines** across native hypervisor backends.

> **Status:** pre-alpha. The repository-local `0.3.0` Cross-Platform Human MVP
> candidate is complete on `dev` over the M1-M5 foundations. It is not a public
> release. Real Hyper-V, PowerShell Direct, QEMU, WHPX, KVM, QGA, and HVF
> acceptance remains separately gated and is not established by core CI.

The project is aimed at local development, CI, engineering software, and autonomous-tool workloads that need a clean, reproducible VM without turning a workstation into a cloud control plane.

## Windows Human MVP quick start

The canonical portable-install → doctor → VHDX validate/register → `vmcell
run` → cleanup-verification workflow is documented in the
[Windows Human MVP Quick Start](docs/quickstart-windows.md). Real Hyper-V and
PowerShell Direct execution remains release-gated; repository-local CI and a
ready doctor probe do not authorize or establish platform acceptance.

## Windows Daily Driver

The canonical v0.2 install → doctor → image lifecycle → run/retain →
status/inspect → shell → reconcile/cleanup → upgrade workflow is
[Windows Daily Driver](docs/windows-daily-driver.md). It describes implemented
repository-local behavior while preserving the same separate real-platform
release gate.

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
vmcell status [--json]
vmcell provider list [--json]
```

The current `dev` candidate exposes:

```text
vmcell doctor
vmcell status
vmcell state check
vmcell completion powershell
vmcell provider list
vmcell image add --id IMAGE --path BASE.vhdx --guest-os windows --provider hyperv
vmcell image add --id IMAGE --path BASE.qcow2 --guest-os linux --provider qemu
vmcell image validate --path BASE.vhdx --guest-os windows --provider hyperv
vmcell image validate --id IMAGE --provider hyperv
vmcell image list
vmcell image inspect IMAGE
vmcell image dependencies IMAGE
vmcell image unregister IMAGE
vmcell create --image IMAGE --provider PROVIDER --cpu-count 2 --memory-mib 4096 [--accelerator POLICY] [--allow-tcg] [--ttl-seconds N]
vmcell run --image IMAGE [--provider PROVIDER] [--accelerator POLICY] [--allow-tcg] [--cpu 2] [--memory 4096] [--ttl N] [--keep | --keep-on-failure] -- PROGRAM [ARG...]
vmcell --json run --image IMAGE [--provider PROVIDER] [--accelerator POLICY] [--allow-tcg] --plan-only
vmcell job plan --spec vmcell.toml
vmcell run --spec vmcell.toml --plan-only
vmcell run --spec vmcell.toml [--username USER --password-stdin]
vmcell list
vmcell inspect CELL_ID
vmcell start CELL_ID
vmcell stop CELL_ID
vmcell destroy CELL_ID
vmcell reconcile [CELL_ID]
vmcell exec CELL_ID --username USER --password-stdin -- PROGRAM [ARG...]
vmcell shell CELL_ID --username USER --password-stdin
vmcell copy-in CELL_ID --source HOST_FILE --destination GUEST_PATH --username USER --password-stdin
vmcell copy-out CELL_ID --source GUEST_PATH --username USER --password-stdin
vmcell artifact collect CELL_ID --path GUEST_PATH --username USER --password-stdin
vmcell artifact inspect CELL_ID OPERATION_ID
vmcell artifact prune [--older-than-seconds N] [--max-artifacts N] [--dry-run]
vmcell operation list [CELL_ID]
vmcell operation inspect OPERATION_ID
vmcell operation reconcile OPERATION_ID
vmcell gc
```

State/provider commands support global `--json`, `--config PATH`, `--state-root
PATH`, bounded `--lock-timeout-ms N`, and `--human-output normal|quiet`
options. `completion powershell` is deliberately human-only and rejects
`--json`; it accesses neither configuration, state, nor providers. The timeout
applies to each state-lock
acquisition; artifact dry runs leave records untouched but may initialize lock
infrastructure on a new state root. Guest
credentials are accepted only through bounded stdin and are never written to
state. Guest actions require a current installation identity, a pinned runtime,
and an exact-owned running VM rechecked by its provider identity. Windows uses
PowerShell Direct; M3 adds credentialless Linux QGA. Real QEMU/KVM, WHPX, and
HVF acceptance remain separate host gates.

The optional [user configuration](docs/user-configuration.md) is bounded,
versioned, and non-authorizing. CLI values win. Configuration may supply safe
defaults for new work and state selection, but never credentials, accelerator
or TCG permission, lifecycle intent, ownership exceptions, or provider-object
identity.

`vmcell shell` is a deliberately line-oriented PowerShell Direct console for an
already running, ready, exact-owned Hyper-V Windows cell. It is not a local or
guest PTY: every nonempty line starts one independent bounded
`powershell.exe -Command` operation with a fresh ownership proof. Guest stdin,
`Read-Host`, full-screen controls, and persistent cwd/environment/process state
are unavailable. The password is supplied on bounded stdin exactly as for
`exec`; shell lines come from the attached Windows console. `.exit`, EOF, or a
cooperative Ctrl-C leaves the cell running. Timeout, transport/session failure,
ownership drift, or a prior nonterminal operation stops the loop without
replay or automatic cleanup and reports the durable operation ID when one was
recorded.

`vmcell status` is a read-only, provider-tolerant daily-use summary. It keeps
durable cell/image/operation evidence visible when a provider is unavailable,
derives expired/manual retention at one evaluation instant, marks
transport-active guest operations as uncertain, and reports only
non-authorizing cleanup or manual-review guidance. Existing `list`, `inspect`,
`reconcile`, `operation`, and `doctor` human output uses the same vocabulary.

Human `vmcell run` first writes its provider-neutral descriptive execution plan
to stderr, then writes concise lifecycle and cleanup progress, forwards bounded
guest stdout/stderr to their matching streams, and returns the guest exit code
after a completed command. `--json` suppresses progress and emits one versioned
run report with the additive `plan` field; failure envelopes include the plan
when resolution completed, safe recovery identifiers, and cleanup disposition,
but never guest stream contents. `run --plan-only` exposes the same versioned
plan without credentials or mutation. See [run selection](docs/run-selection.md).

A [versioned job specification](docs/job-spec.md) describes one prepared,
disposable execution-cell workload without becoming a provisioning DSL. `vmcell
job plan --spec` and `vmcell run --spec ... --plan-only` are read-only and
non-authorizing. Each admitted non-plan `run --spec` lifecycle run receives a
fresh job and cell identity, even when the same specification hash is used
again; this is not deduplication, replay, or a claim that guest/application
output is byte identical. Job results and durable correlation remain bounded
observability metadata, never lifecycle or provider authority.

The canonical [native Linux QEMU/KVM/QGA walkthrough](docs/linux-kvm-qga.md)
uses the same provider-neutral workflow for a prepared Linux x86_64 QCOW2
guest. KVM admission distinguishes a missing device from one that the current
identity cannot open read/write, repairs neither condition, and never falls
back to TCG. Its exact-SHA preflight is non-authorizing; real KVM/QGA lifecycle
acceptance remains pending and the support row stays `untested`.

Before first use of a newer binary against an existing root, `vmcell state
check` provides a read-only, provider-free durable compatibility preflight.
The frozen v0.1 candidate and v0.2 share durable format version 1; compatible
v0.1 records are read in place without rewrite. v0.4 job-correlated cell,
operation, and artifact records use format version 2 so an older binary refuses
their provenance before mutation; direct and historical records remain version
1. Unknown schemas fail with `vmcell.state.upgrade_required` before mutation.
See [durable-state compatibility and recovery](docs/state-compatibility.md).

`vmcell image validate` is read-only. Candidate-path mode proves that the
ordinary non-reparse base file has the selected provider's format, has no
backing parent, and can be hashed. Registered-image mode repeats those checks
and compares canonical path, size, format, and SHA-256 with the durable image
record. Human `image add`, `list`, and `inspect` output includes guest/provider
identity and immutable content identity; `--json` retains versioned records and
validation reports. `image dependencies` lists every durable cell reference;
all non-destroyed references block `image unregister`. Unregistering is a
provider-neutral metadata-only operation: it never probes a provider or reads,
hashes, modifies, or deletes registered base-image contents. Existing
variant files are opened read-only/no-follow only long enough to reject a
reparse or same-file alias to the manifest. Destroyed cell
tombstones retain their copied image binding and do not block metadata removal.
The operation is idempotent and reports `bytes_deleted=false`. vmcell does not
build, mount, or modify guest images.

## Windows portable distribution

The repository can build a deterministic portable Windows archive containing
`vmcell.exe`, generated PowerShell completion, license/notice, bounded
install/upgrade/remove guidance, candidate-only Scoop/WinGet metadata, and
versioned build provenance. A sibling `SHA256SUMS.txt` binds the archive. The
manual trusted-runner workflow does not publish a package, release, or tag; see
[Windows Portable Package](docs/windows-portable-package.md) for layout and
verification, and [Windows install, upgrade, and remove](docs/windows-install-upgrade-remove.md)
for the operator workflow.

The repository-local Linux distribution mirrors that discipline for
`x86_64-unknown-linux-gnu`: a deterministic versioned `.tar.gz`, generated Bash
and Zsh completion, in-archive and adjacent SHA-256 manifests, exact build
provenance, and an unprivileged temporary-prefix install/remove smoke test. The
manual native-Linux exact-SHA lane records the observed GLIBC symbol floor and
does not claim musl/static portability or real KVM/QGA acceptance. See the
[Linux portable package](docs/linux-portable-package.md) and
[Linux install, upgrade, and remove](docs/linux-install-upgrade-remove.md).

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
- prune artifacts only through an explicit bounded dry-runnable command that
  persists `artifact_pruned_at` before exact-subtree deletion;
- emit and persist stable redacted error codes instead of raw provider stderr;
- run TTL cleanup only through explicit `vmcell gc` and the existing exact-owned destroy path.

`vmcell doctor` is expected to report missing prerequisites rather than attempting to repair the host automatically.

## Platform direction

The machine-validated [platform support matrix](docs/support-matrix.md) is the
single source for host, architecture, provider, accelerator, guest, and
transport status. An absent combination is not inferred from a similar row.
Repository CI, mocks, fake protocols, and WSL2 evidence cannot promote a real
platform combination to `supported`.

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
- **M1 — Hyper-V cell foundation:** repository-local implementation is merged; real Hyper-V acceptance remains gated.
- **M2 — Windows guest control:** repository-local implementation is merged; real PowerShell Direct guest acceptance remains gated.
- **M3 — QEMU provider:** repository-local QMP lifecycle, QCOW2 overlay, and QGA contracts are merged; real QEMU/WHPX/KVM/HVF acceptance remains gated.
- **v0.3 Windows QEMU/WHPX:** the repository-local Linux QCOW2 + QGA path,
  executable/process identity hardening, [human walkthrough](docs/windows-qemu-whpx.md),
  and non-mutating acceptance preflight are implemented; real WHPX/QGA
  acceptance remains pending and the support row remains `untested`.
- **M4 — Linux portability foundation:** Unix state/path/process and KVM capability foundations are merged; native Linux/KVM and macOS/HVF acceptance remains gated.
- **v0.3 native Linux QEMU/KVM/QGA:** the repository-local Linux human workflow,
  typed KVM admission, bounded Unix control endpoints, exact process/state
  identity, [walkthrough](docs/linux-kvm-qga.md), and non-mutating acceptance
  preflight are implemented; real KVM/QGA acceptance remains pending.
- **v0.3 Linux distribution:** the deterministic `x86_64-unknown-linux-gnu`
  portable candidate, exact build provenance, checksum layers, completions,
  and unprivileged install/upgrade/remove contract are implemented; no package
  repository or public release has been published.
- **M5 — automation hardening:** repository-local implementation is merged; stable JSON, deterministic failures, reconciliation, contention, retention, and automation CLI contracts are covered by cross-provider tests.

Provider-specific capabilities such as TPM, Secure Boot, nested virtualization, GPU/device support, or additional native providers come only after the core lifecycle is stable.

## Development

Rust project code is intended to remain Rust-first across platforms. Hypervisor-specific integrations may call stable platform interfaces or vendor tools where that is the correct boundary; the project does not reimplement Hyper-V or QEMU.

Before submitting changes:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Core CI runs Rust checks on a dedicated self-hosted Windows runner with no
Hyper-V mutation responsibilities. A manual exact-SHA GitHub-hosted Ubuntu lane
provides non-privileged native-Linux compile, test, and package evidence without
virtualization lifecycle mutation. Real provider integration belongs on a
separate explicitly privileged and isolated runner; neither repository lane is
real-platform acceptance.

## License

Licensed under the [Apache License 2.0](LICENSE).
