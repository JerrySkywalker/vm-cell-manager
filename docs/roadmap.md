# Roadmap

VM Cell Manager's original M0-M5 milestones established the repository-local technical foundation. From this point forward, milestones are defined primarily by **what a human user can accomplish with a released build** and by the real-platform claims attached to that release.

The roadmap remains capability-driven. A numbered release is promoted from `dev` only through the frozen `release/vX.Y.Z` workflow and its declared acceptance gates. Repository-local implementation may continue rapidly on `dev`, but unit, mock, WSL2, or core CI evidence never substitutes for real-platform acceptance.

## Release model

The persistent branch model is:

```text
main               latest stable/release baseline
dev                persistent repository-local integration
agent/*            short-lived Codex/product development branches
release/vX.Y.Z     temporary frozen release candidate
hotfix/*            release fixes originating from main
```

Normal development is:

```text
green dev
  -> agent/*
  -> focused validation
  -> exact-head CI
  -> PR to dev
  -> merge
  -> exact-dev CI
  -> delete agent branch/worktree
```

Release promotion is:

```text
green exact dev head
  -> release/vX.Y.Z
  -> declared repository-local + real-platform acceptance
  -> reviewed PR to main
  -> exact-main CI
  -> immutable vX.Y.Z tag / release
  -> synchronize accepted main history back to dev
  -> retire release branch
```

Version tags are immutable and are never moved, deleted, or reused. A correction receives a new version.

---

# Completed technical foundation — M0-M5

These milestones are retained as completed foundation history. They describe the internal technical capabilities that made the human-usable product roadmap possible; they are not the future release numbering scheme.

## M0 — Architecture bootstrap

**Status:** complete.

Delivered:

- Rust 2024 workspace/binary skeleton;
- local-first Rust CI on a dedicated self-hosted Windows runner;
- read-only `vmcell doctor` and provider probing;
- provider-neutral `Cell`, `Image`, `ProviderCapabilities`, and lifecycle model;
- Hyper-V and QEMU provider modules without VM mutation;
- Guest I/O interface with PowerShell Direct, QGA, and SSH boundaries;
- product scope, competitive landscape, and OpenStack boundary documentation.

M0 did not authorize VM creation.

## M1 — Hyper-V cell foundation

**Status:** repository-local foundation implemented and merged; dedicated real Hyper-V acceptance remains pending.

Delivered/planned acceptance surface:

- Hyper-V prerequisite/capability probe reports exact missing prerequisites;
- image registration for an immutable VHDX base;
- image identity verification before cell creation;
- one differencing VHDX per cell;
- VM create/start/inspect/stop/destroy;
- CPU and memory configuration;
- strict ownership marker + local manifest reconciliation;
- safe/idempotent destroy;
- no automatic Hyper-V enablement, reboot, or host-global switch creation;
- provider integration acceptance on a dedicated Hyper-V-capable host remains external.

## M2 — Windows guest control

**Status:** repository-local implementation merged; real PowerShell Direct acceptance remains pending after M1 real-provider admission.

Delivered:

- PowerShell Direct readiness detection;
- command execution with exit code/stdout/stderr;
- copy-in and copy-out semantics;
- artifact collection;
- TTL model and explicit `vmcell gc`;
- guest-control recovery layered on the M1 ownership reconciliation model.

Mock and fault validation do not constitute real PowerShell Direct acceptance.

## M3 — QEMU reference provider

**Status:** repository-local QEMU/QMP/QGA implementation merged; real QEMU/WHPX/KVM/HVF acceptance remains environment-gated.

Delivered:

- QMP connection and lifecycle;
- QCOW2 base + single overlay;
- QGA guest transport;
- explicit hardware-accelerator discovery for WHPX/KVM/HVF;
- TCG/cross-architecture emulation only through explicit opt-in;
- provider-neutral ownership/reconciliation and bounded protocol/process handling.

## M4 — Linux portability foundation

**Status:** repository-local portability implementation merged; native Linux/KVM acceptance remains environment-gated.

Delivered:

- Linux/Unix conditional compilation and process behavior;
- Unix private state permissions;
- no-follow and device/inode identity gates;
- canonical executable discovery;
- KVM device usability filtering;
- Unix QMP socket coverage;
- portable validation workflow.

WSL or nested environments may be used for development experiments but are not substitutes for native host acceptance. macOS abstractions remain non-breaking while real HVF acceptance is deferred.

## M5 — Automation contract hardening

**Status:** repository-local implementation complete.

Delivered:

- versioned JSON response schemas;
- deterministic exit-code/error taxonomy;
- stable provider-neutral ownership/reconciliation classifications;
- machine-readable doctor/capability contracts;
- bounded contention and concurrency-lock hardening;
- artifact/log retention and redaction policy;
- automation CLI migration and cross-provider fault/property coverage.

M5 was delivered as sequential short PRs against green integration history rather than a deep unfinished stack.

---

# Human-usable product roadmap

## Current baseline — Unreleased foundation

**Status:** implemented; not yet a public product release.

The current code already contains Hyper-V and QEMU provider abstractions, immutable image registration and copy-on-write cells, lifecycle operations, PowerShell Direct and QGA guest-control implementations, exec/copy/artifact workflows, TTL/GC, versioned automation contracts, crash/concurrency foundations, and Windows/Unix host-safety abstractions.

A user may inspect and exercise non-destructive CLI behavior, but the project must not yet claim a real Windows, Linux, QEMU, KVM, WHPX, or HVF production path solely from repository-local evidence.

The package currently uses pre-1.0 versioning, but no official release tag is assigned to this foundation baseline. The first official `v0.1.0` tag is created only after the release workflow below succeeds.

---

# v0.1.0 — Windows Human MVP

**Repository-local status:** implemented and frozen on `release/v0.1.0`; real
Hyper-V/PowerShell Direct acceptance and promotion to `main` remain pending.

## Product promise

> Given a prepared Windows VHDX on an accepted Hyper-V host, a human user can run a command inside a disposable Windows VM, retrieve its result, and safely dispose of the VM without manually operating Hyper-V.

This is the first version that should feel like a product rather than a virtualization library with a CLI.

## Primary human workflow

The central UX becomes conceptually:

```text
vmcell run --image windows-dev -- <command>
```

Internally the existing engine safely composes:

```text
image
  -> create
  -> start
  -> wait for PowerShell Direct
  -> execute
  -> optionally collect artifacts
  -> stop
  -> destroy
```

Primitive lifecycle commands remain available for debugging and automation.

## Required user-facing capabilities

### `vmcell run`

Add a first-class human workflow command that composes existing lifecycle and guest-control authority rather than introducing a second orchestration implementation.

Required behavior:

- select a registered image;
- create one exact-owned cell;
- start it;
- wait for guest readiness;
- execute the requested command;
- return guest exit code/stdout/stderr coherently;
- clean up automatically on success;
- preserve fail-closed semantics on ambiguity.

Useful human options should cover concepts equivalent to:

```text
--keep
--keep-on-failure
--ttl
--cpu
--memory
```

Exact spelling may be refined during implementation.

### Human-readable lifecycle output

Default non-JSON output should clearly communicate progress and cleanup disposition, for example:

```text
Image verified
Cell created
VM started
Guest ready
Command completed: exit 0
Cell destroyed
```

Failures identify the failed stage and cleanup status without exposing credentials or unsafe provider diagnostics. `--json` remains the automation contract.

### Practical image onboarding

A user with an ordinary prepared VHDX must be able to understand whether it is usable. The `image add/inspect` path should clearly answer whether the file is supported, immutable/structurally suitable, successfully registered, and associated with the intended guest/provider identity.

A dedicated `image validate` command may be introduced if it materially clarifies the workflow; image building itself remains outside vmcell.

### Windows binary distribution

A user must not need a Rust development environment. `v0.1.0` publishes at least:

- `vmcell.exe`;
- a portable Windows archive;
- SHA-256 checksums;
- reliable version information;
- minimal install/upgrade/remove instructions;
- a concise Windows quick start.

Package-manager integration is optional for this release.

### Canonical quick start

The release documentation contains one bounded copyable flow:

```text
install vmcell
-> doctor
-> register prepared Windows image
-> vmcell run
-> observe command output
-> prove cleanup
```

The repository-local command and package surface for this flow is documented
in `docs/quickstart-windows.md`. Its mutating steps remain release-gated until
the dedicated-host acceptance claims below pass; `doctor` readiness alone is
not admission.

## Real-platform release gates

The frozen `release/v0.1.0` candidate must prove on an admitted dedicated Windows host:

- Hyper-V capability and safe host admission;
- VHDX registration and immutable identity;
- create/start/inspect/stop/destroy;
- PowerShell Direct readiness;
- command execution;
- copy-in/copy-out;
- artifact collection;
- exact cleanup and unchanged foreign state;
- release-critical failure/cleanup behavior.

Only after those claims pass may the candidate promote to `main` and receive immutable tag `v0.1.0`.

## Explicitly not required

- QEMU as a release claim;
- Linux or macOS host support;
- interactive shell;
- TUI or GUI;
- remote image registry;
- automatic image construction;
- shared folders;
- Secure Boot/TPM;
- GPU/device assignment;
- multi-host scheduling;
- application-specific automation.

## Human-visible completion criterion

A new user with the documented Windows host and prepared VHDX can install the published binary and successfully execute `vmcell run ...` without touching Hyper-V Manager.

---

# v0.2.0 — Windows Daily Driver

**Repository-local status:** complete on `dev`; real Windows acceptance,
release promotion, and an immutable version tag remain pending.

## Product promise

> A Windows developer can use vmcell repeatedly as a normal local development tool, not merely as a successful demo.

The frozen `v0.1.0` candidate targets proof of one accepted end-to-end Windows
workflow. The v0.2 repository candidate adds the ergonomics needed for repeated
daily use. Neither candidate is a public release until its declared external
acceptance and promotion gates pass.

## Required user-facing capabilities

### Interactive human session

Add a supported interactive entry workflow, tentatively:

```text
vmcell shell CELL
```

or equivalent. It should let a human enter an already running exact-owned Windows cell for diagnosis or exploratory work without manually operating Hyper-V/PowerShell Direct.

If the transport cannot provide true terminal semantics, the UX must expose that limitation rather than pretending to be a local PTY.

Repository-local Slice B implements this as a line-oriented PowerShell Direct
console over the existing guest-operation authority. Every line is separately
bounded and freshly authorized; there is no PTY or persistent remote process,
and unknown operations stop without replay or automatic cleanup. Real
PowerShell Direct acceptance remains release-gated.

### Human-oriented status and diagnosis

Users should be able to answer:

- What cells exist?
- Which are running or expired?
- Which guest operations are uncertain?
- Why can this host not create a cell?
- What can I safely clean up?

This should evolve existing `doctor`, `list`, `inspect`, `operation`, and `reconcile` surfaces where possible rather than multiplying commands unnecessarily.

**Repository-local implementation:** `vmcell status` now provides one
schema-versioned read-only aggregate while the existing human surfaces use the
same state, phase, retention, reconciliation, required-action, and uncertainty
vocabulary. Provider unavailability preserves local evidence and forces manual
review instead of aborting the whole summary or claiming cleanup authority.

### Better image lifecycle UX

Cover the normal lifecycle of locally supplied images:

- validate/register;
- inspect;
- list variants/provider compatibility;
- detect stale/changed backing files;
- safely unregister metadata when no owned cells depend on it.

No online image marketplace or general-purpose image builder is required.

**Repository-local implementation:** candidate and registered validation,
provider-variant status, content/backing drift proof, CellId-sorted dependency
inspection, and metadata-only `image unregister` are implemented. All
non-destroyed cell references block removal. The operation never probes a
provider or touches base-image bytes; real provider/image acceptance remains a
separate release gate.

### Configuration ergonomics

Introduce a small explicit user configuration mechanism for appropriate defaults such as provider policy, CPU/memory defaults, safe state-root defaults, and human-output preferences. Configuration never silently grants destructive authority; command-line arguments remain authoritative.

**Repository-local implementation:** schema-v1 bounded JSON configuration now
provides only non-authorizing defaults for new-work provider selection,
CPU/memory, state root, lock/readiness/action timeouts, and human run progress.
CLI values win, existing durable provider identity is unchanged, and unknown
credential/accelerator/TCG/authority fields fail before state or provider
access. Real provider behavior remains release-gated.

### Interruption and recovery UX

Normal human interruption must be understandable across Ctrl-C, readiness/command timeout, process crash, retained `--keep-on-failure` cells, and expired cells awaiting GC.

The user is told whether nothing was created, an exact-owned cell remains, cleanup succeeded, or cleanup was refused because state is ambiguous.

**Repository-local implementation:** Windows `run` now samples Ctrl-C
cooperatively at durable stage boundaries. Safe pre-guest interruption can use
the existing exact-owned cleanup path; transport-active/unknown work remains
nonterminal, retained, and nonreplayed; completed commands retain their result
and requested keep policy. Status, operation reconciliation, and explicit GC
continue to provide the recovery path for retained/expired cells.

### Upgrade compatibility

A user upgrading from `v0.1.x` to `v0.2.0` receives a defined durable-state compatibility story. Older state is either supported, explicitly migrated through a bounded path, or rejected with deterministic upgrade instructions. Silent reinterpretation is forbidden.

**Repository-local implementation:** the frozen v0.1 candidate and v0.2 share
durable format version 1. `vmcell state check` validates active schema-1 records
read-only without provider access or rewrite. Unknown schemas return the stable
`vmcell.state.upgrade_required` integrity result and stop mutation; no implicit
migration or downgrade exists.

### Windows installation quality

Improve deterministic archive layout, completion where practical, stable version/help output, upgrade documentation, and optional Scoop/WinGet integration if maintainable.

**Repository-local implementation:** the `0.2.0` candidate produces a
byte-reproducible portable ZIP with exact-version help, generated PowerShell
completion, schema-v1 candidate metadata shaped for later Scoop/WinGet work,
and explicit install/upgrade/state-check/rollback/remove guidance. Packaging
remains a trusted, manual, non-publishing workflow; no tag, release, or package
manager submission is created by this milestone.

## Release gates

The frozen `v0.1` Windows candidate remains the first real-platform admission
baseline. v0.2 release acceptance additionally focuses on repeated operation,
interruption/recovery, retained cells, state compatibility, interactive/session
behavior, and image lifecycle operations. Neither release has real-platform
acceptance merely because repository-local v0.2 implementation is complete.

## Human-visible completion criterion

A developer can install vmcell, maintain a small local image collection, run disposable jobs, enter/debug retained cells, understand failures, upgrade vmcell, and clean up safely without knowing vmcell's internal state layout.

---

# v0.3.0 — Cross-Platform Human MVP

## Product promise

> The same human-facing vmcell workflow works on both Windows and native Linux, with provider differences expressed as capabilities rather than different products.

This milestone makes the original cross-platform execution-cell vision directly visible.

## Required accepted platform paths

At minimum:

```text
Windows + Hyper-V + Windows guest + PowerShell Direct
Windows + QEMU/WHPX + supported guest
Linux + QEMU/KVM + Linux guest + QGA
```

Native Linux evidence is required; WSL2 is development evidence only. macOS/HVF may remain deferred or experimental.

## Required user-facing capabilities

### Provider-neutral `vmcell run`

A normal user should generally choose an image/workload rather than a VMM. `vmcell run --image ...` should select an accepted local provider/accelerator path where unambiguous, while explicit overrides remain available. Hardware acceleration must never silently fall back to TCG/emulation.

**Repository-local contract:** [run selection](run-selection.md) now resolves a
versioned, non-authorizing plan from logical image variants, host/provider
probes, exact accelerators, guest identity, transport capability, and the
support matrix before mutation. CLI overrides outrank an explicitly present
config preference, which outranks deterministic native resolution. TCG
requires its two explicit CLI flags and is never a fallback. Real platform
acceptance remains deferred.

**Windows QEMU/WHPX foundation:** the
[canonical Windows QEMU/WHPX walkthrough](windows-qemu-whpx.md) reuses the
existing QEMU/QMP/QGA lifecycle and provider-neutral plan for a prepared Linux
x86_64 QCOW2 guest. Windows discovery binds canonical executable identities
and content hashes, reports WHPX capability explicitly, and never falls back
to TCG. A fixture-tested, non-mutating dedicated-host preflight and acceptance
receipt template are present; real WHPX/QGA lifecycle acceptance remains
external, so the support row stays `untested`.

**Native Linux QEMU/KVM/QGA foundation:** the
[canonical native Linux walkthrough](linux-kvm-qga.md) reuses that same
provider-neutral plan, QEMU/QMP/QGA lifecycle, immutable-base/single-overlay
model, ownership, recovery, and artifact surface. KVM remains an accelerator.
Admission requires a stable ordinary `/dev/kvm` character-device identity and
a read/write open by the current identity, without ioctl, repair, module load,
or TCG fallback. Unix control paths are bounded and stale/colliding paths are
retained for manual review. The exact-SHA preflight and receipt template are
non-authorizing; real KVM/QGA lifecycle acceptance remains external, so the
support row stays `untested`.

### Real QEMU lifecycle acceptance

Prove real QEMU process lifecycle, immutable QCOW2 base, exactly one overlay, QMP lifecycle/reconciliation, QGA guest readiness, command execution, file transfer, exact cleanup, and crash/failure classification.

### Native Linux distribution

Publish a supported native Linux binary/install path and prove Unix state permissions, path containment, process-tree termination, QMP Unix sockets, KVM admission, durable state, and cleanup semantics.

**Repository-local implementation:** the manual exact-SHA native-Linux lane
builds and validates a deterministic `x86_64-unknown-linux-gnu` portable
archive on the declared Ubuntu 24.04/Rust 1.85.0 baseline. The package carries
generated Bash/Zsh completion, exact source/build provenance, internal and
adjacent checksums, an observed GLIBC symbol floor, and bounded install,
upgrade, rollback, and removal guidance. Repeated package assembly from the
same binary and declared inputs is byte-identical; cross-environment binary
reproducibility is not claimed. No apt/rpm publication or real KVM/QGA
acceptance is implied.

### Cross-platform image variants

A logical image identity can expose provider-specific variants cleanly where appropriate, without requiring users to understand internal manifest mechanics.

### Published support matrix

Each release explicitly distinguishes supported, experimental, development-only, untested, and unsupported host/provider/accelerator/guest/transport combinations.

**Repository-local contract:** the [platform support matrix](support-matrix.md)
is rendered from one typed source and checked byte-for-byte in core tests.
Duplicate, conflicting, undocumented, impossible, or evidence-free promoted
combinations fail closed. Current real WHPX, KVM, QGA, and PowerShell Direct
paths remain `untested`; explicit TCG paths are `development-only` and never an
implicit fallback.

## Explicitly not required

- fully supported macOS;
- GUI/TUI;
- cloud or multi-host scheduling;
- automatic application orchestration;
- general-purpose image building;
- Secure Boot/TPM/GPU passthrough;
- distributed image management.

## Human-visible completion criterion

A Windows or Linux developer can install the appropriate binary and use substantially the same image/run/exec/artifact/inspection/cleanup workflow on an accepted native provider without learning provider-specific orchestration commands.

---

# v0.4.0 — Reproducible Jobs

## Product promise

> A human or automation system can describe one disposable VM job once, inspect what vmcell will do, run it repeatedly, and obtain a coherent result bundle without scripting low-level lifecycle glue.

This establishes the first durable execution-job abstraction above individual CLI verbs and the intended future boundary with higher-level orchestration systems.

## Required user-facing capabilities

### Versioned run specification

Allow the `vmcell run` workflow to be represented by a small versioned declarative specification, conceptually:

```text
vmcell run --spec vmcell.toml
```

The specification may describe only execution-cell concerns such as image, provider/accelerator policy, CPU/memory, TTL, command/arguments, timeouts, copy-in inputs, artifact outputs, and cleanup/keep-on-failure behavior.

It must not become a Vagrant/Packer-style provisioning DSL. It does not own package-install workflows, arbitrary provisioning stages, infrastructure orchestration, application-specific automation, host networking, cloud resources, multi-machine topology, or distributed scheduling.

### Human-readable planning

Before mutation, users can inspect the resolved execution plan, conceptually through `vmcell run --spec ... --plan` or `vmcell plan --spec ...`. Planning is read-only and never grants mutation authority.

### Deterministic result model

A completed job produces one coherent result model containing job/cell/image identity, provider/accelerator, guest exit status, timing, artifact manifests, cleanup disposition, and stable error classification where applicable. Human output summarizes it; `--json` exposes a versioned machine-readable form.

### Operation correlation

All operations belonging to one `run` invocation are correlated under one run/job identity while reusing existing durable operation/state concepts rather than creating a second orchestration state machine.

### Repeatability semantics

Running the same specification twice means two fresh disposable execution cells unless explicitly requested otherwise. vmcell guarantees deterministic interpretation of its execution contract, not bit-for-bit deterministic application output.

## Release gates

Specification-driven and equivalent direct-CLI execution must resolve to the same authority, lifecycle, guest-control, artifact, and cleanup contracts across the accepted Windows/Linux paths.

## Human-visible completion criterion

A user can place a small versioned vmcell job specification next to a project and repeatedly execute `vmcell run --spec vmcell.toml` without writing lifecycle glue scripts.

---

# v0.5.0 — Three-Host Portability

## Product promise

> Windows, Linux, and macOS users can use the same vmcell product model on an explicitly supported local virtualization path.

This closes the original desktop-host portability story.

## Required accepted platform path

Add real macOS acceptance through QEMU/HVF and a supported guest. Supported host architecture is stated explicitly; Apple Silicon and Intel are not treated as equivalent unless both are tested.

## Required capabilities

- real QEMU/HVF accelerator discovery and architecture diagnostics;
- immutable QCOW2 base/overlay lifecycle;
- QMP/QGA where supported;
- command execution, artifact transfer, interruption, cleanup, foreign-state preservation;
- native supported macOS binary for each claimed architecture;
- platform-consistent doctor/image/run/inspect/destroy UX;
- explicit distinction among hardware virtualization, emulation required, and unsupported architecture combinations;
- no silent TCG fallback.

After real QEMU/HVF experience exists, explicitly decide whether a native Apple Virtualization.framework provider solves a demonstrated gap. Provider count remains a cost, not a goal.

## Explicitly not required

- feature parity for every guest OS on every host;
- Apple Virtualization.framework provider;
- nested virtualization;
- GPU/device assignment;
- GUI;
- cloud execution.

## Human-visible completion criterion

A documented Windows, Linux, or macOS user can install vmcell and recognize substantially the same product/workflow on each supported platform.

---

# v0.6.0 — Image and Distribution Maturity

## Product promise

> A normal user can maintain a trustworthy local library of execution images and move vmcell installations between machines without manually understanding internal state files.

Prepared images remain external artifacts, but their lifecycle inside vmcell becomes mature.

## Required user-facing capabilities

### Mature local image catalog

Users can clearly answer what images exist, where variants came from, whether backing files changed, which providers can use them, which cells depend on them, and whether metadata can be safely removed.

Image identity remains content- and metadata-bound rather than path-name-only.

### Portable image metadata

Define a versioned portable manifest for prepared images and provider variants, including logical image ID, guest OS, architecture, provider variant, artifact hash, expected format, guest transport capability, and human description where useful. Credentials are forbidden.

### Import/export workflow

Provide a supported method to move image metadata between vmcell installations. If image bytes are exported/bundled, integrity and size semantics are explicit. A remote registry is not required.

### Safe unregister/remove semantics

Clearly distinguish removing registration metadata from deleting image bytes. Destructive byte deletion requires explicit authorization.

### Installation channels

Routine installation should no longer depend primarily on manually downloading arbitrary archives. Maintainable supported channels should exist for claimed platforms where justified.

### Upgrade/downgrade clarity

Before mutation, users are told whether the binary can read existing state, image metadata, job specs, and artifact records. Unsupported downgrade paths fail closed.

## Human-visible completion criterion

A user can install vmcell on a new supported machine, import a known image definition, validate its local artifact, and begin running jobs without reconstructing internal state manually.

---

# v0.7.0 — Public Beta

## Product promise

> vmcell is ready for users outside the original development environment, with documented limits and an explicit compatibility policy that is beginning to stabilize.

## Beta entry requirements

### Supported platform matrix

Primary advertised combinations have repeated real-platform acceptance and are categorized explicitly as supported, experimental, development-only, untested, or unsupported. There is no ambiguous "should work" category.

### Installation documentation

A new user can go from no vmcell installation to a successful disposable workload using public documentation without repository-development knowledge.

### Compatibility policy

Publish the first explicit compatibility policy for CLI, JSON schemas, job-spec schemas, durable state, image metadata, and artifact manifests. Pre-1.0 breaking changes may still occur but are intentional, documented, and migratable where practical.

### Security model publication

Document the real security boundary across host authority, vmcell ownership authority, guest administrator authority, foreign provider state, image trust, guest-filesystem trust, and artifact trust. Disposable VMs must not be marketed as a stronger sandbox boundary than the implementation proves.

### Contribution-safe CI

If external contributors are expected, introduce a separate safe validation path for untrusted pull requests. The trusted self-hosted runner must never execute arbitrary public-fork code automatically.

### Diagnostic quality

A user filing a bug can produce a sanitized diagnostic report/package with useful version/capability/error information without leaking credentials or unrelated host state.

## Human-visible completion criterion

A technically competent external user can install vmcell, follow the public support matrix/quick start, complete a real workload, diagnose ordinary failures, and file a useful bug without private guidance from the original developer.

---

# v0.8.0 — Reliability Beta

## Product promise

> vmcell remains trustworthy under repeated, interrupted, concurrent, upgraded, and long-running everyday use, not only in a one-off demonstration.

This milestone is deliberately reliability-heavy rather than feature-heavy.

## Required validation themes

### Repeated lifecycle stress

Exercise large repeated create/start/exec/artifact/stop/destroy sequences and prove no systematic accumulation of owned VMs, overlays, sockets, orphaned processes, stale operation state, or unbounded artifacts.

### Crash/interruption campaigns

Validate vmcell process termination, guest command timeout, provider process death, host session loss where relevant, reboot where a supported recovery contract exists, and corrupted/incomplete durable records. Unknown effects remain non-replayed unless explicitly safe.

### Concurrency behavior

Define deterministic locking, contention diagnostics, and safe serialization of conflicting local mutations without turning vmcell into a daemon or scheduler.

### Upgrade matrix

Repeatedly prove supported upgrades across real previous released versions. Durable-state evolution becomes part of release acceptance.

### Resource bounds

Document/validate practical bounds for command output, copy sizes, artifact retention, concurrent cells where applicable, timeout values, disk growth, and state growth.

### Performance baseline

Establish observational regression baselines for create/start/guest-ready/destroy. They are regression signals, not hard cross-machine guarantees.

### Long-running usage

Use realistic multi-hour workloads to expose TTL, timeout, guest-transport, artifact, and cleanup reliability issues.

## Human-visible completion criterion

A user can keep vmcell installed and use it repeatedly over an extended period without regularly needing to manually repair vmcell state or provider leftovers.

---

# v0.9.0 — 1.0 Contract Candidate

## Product promise

> The intended v1.0 feature set is substantially complete; remaining work is contract validation, compatibility, documentation, and release hardening.

No broad new subsystem should normally enter after this point.

## Contract freeze candidates

Freeze intended v1 behavior for:

- primary CLI hierarchy;
- exit-code taxonomy;
- versioned JSON envelopes;
- job specification;
- durable state schema/migration model;
- image metadata;
- artifact records;
- ownership semantics;
- cleanup semantics;
- provider capability reporting.

Breaking a frozen candidate requires an explicit decision that v1.0 must wait.

## Required work

- full documentation separation among quick start, daily usage, automation reference, job-spec reference, image management, platform installation, troubleshooting, security model, support matrix, architecture, and development/contribution;
- final scope audit for drift toward cloud orchestration, application automation, provisioning DSLs, general VM management, or background scheduling;
- compatibility rehearsal using real artifacts/state/specifications from earlier public releases;
- SemVer prerelease candidates such as `v1.0.0-rc.1`, each through the normal frozen release branch and declared acceptance matrix.

No tag is moved.

## Human-visible completion criterion

A user of the final `v0.9.x` / `v1.0.0-rc` experiences essentially the same normal workflow and contracts that will ship as v1.0.

---

# v1.0.0 — Stable Execution Cell Runtime

## Product promise

> vmcell is a stable, installable, documented local runtime for creating disposable full-system execution cells on explicitly supported desktop/server host platforms, with predictable automation contracts and safe ownership/cleanup semantics.

v1.0 is primarily a **contract commitment**, not a declaration that every virtualization feature exists.

## Required v1 guarantees

### Human workflow

A normal supported flow is concise:

```text
install
-> doctor
-> register/import prepared image
-> run job
-> inspect result/artifacts
-> cleanup
```

Primitive lifecycle commands remain available.

### Automation workflow

Higher-level systems can depend on versioned machine-readable responses, stable failure taxonomy, reproducible execution specifications, deterministic ownership/reconciliation semantics, and deterministic artifact/result records.

### Platform support

Every combination advertised as supported has real acceptance evidence; unsupported/experimental combinations remain explicitly marked.

### Upgrade stability

A supported v1.x upgrade never silently reinterprets durable state. State/schema changes follow explicit compatibility and migration policy.

### Release discipline

`main` remains the latest accepted release baseline, `dev` the repository-local integration line, `agent/*` the normal short-lived development surface, and `release/vX.Y.Z` the frozen acceptance/promotion branch.

### Safety boundary

v1 guarantees only the security/ownership properties actually documented and tested. It does not imply arbitrary malicious guest administrators, hypervisor vulnerabilities, host administrators, or privileged external writers are inside vmcell's protection boundary.

---

# Explicitly not required before v1.0

The following may be useful future capabilities but are not prerequisites for a credible v1 runtime:

- graphical UI;
- rich TUI;
- multi-host scheduler;
- cloud placement;
- distributed image registry;
- general VM inventory management;
- engineering-application automation;
- MCP adapters for individual applications;
- physical HIL/device leasing;
- GPU passthrough;
- generic USB/device assignment;
- nested virtualization;
- Secure Boot / vTPM on every provider;
- shared folders;
- snapshot/checkpoint trees;
- Apple Virtualization.framework provider;
- libvirt provider.

Some may be added before v1 only when a concrete user requirement and safe design justify them; they must not become artificial release gates.

---

# Post-v1 direction

After v1.0, roadmap decisions are demand-driven rather than completeness-driven.

Potential execution-cell capabilities include Secure Boot/vTPM, nested virtualization, controlled shared folders, GPU/device capability discovery/assignment, and richer network isolation policies.

Additional providers are justified only by demonstrated value: Apple Virtualization.framework if QEMU/HVF leaves a material gap, or libvirt if it materially improves Linux operational integration.

Higher-level coordination normally remains above vmcell:

```text
Coordination / Research Loop
          |
          v
Execution Controller
          |
          v
versioned vmcell job contract
          |
          v
vmcell
          |
          v
Hyper-V / QEMU
          |
          v
prepared guest environment
```

The upper layer may select images, submit work, collect artifacts, and schedule resources. vmcell remains responsible only for the bounded local execution-cell contract.

---

# Product progression summary

```text
Completed M0-M5
Technical foundation
        |
        v
v0.1  Windows Human MVP
        |
        v
v0.2  Windows Daily Driver
        |
        v
v0.3  Windows + Linux Human MVP
        |
        v
v0.4  Reproducible Jobs
        |
        v
v0.5  Windows + Linux + macOS portability
        |
        v
v0.6  Image / Distribution maturity
        |
        v
v0.7  Public Beta
        |
        v
v0.8  Reliability Beta
        |
        v
v0.9  1.0 Contract Candidate
        |
        v
v1.0  Stable Execution Cell Runtime
```

Repository-local v0.2 Windows Daily Driver implementation is complete on
`dev`. The next repository-development milestone is **v0.3.0 — Cross-Platform
Human MVP**. Public release advancement remains separately blocked on the
dedicated Windows acceptance path, beginning with the frozen v0.1 candidate.
