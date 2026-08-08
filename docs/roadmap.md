# Roadmap

The roadmap is capability-driven. Milestones are intentionally narrow so the project can stop, reuse an existing tool, or revise a provider boundary before large amounts of platform-specific code accumulate.

## M0 — Architecture bootstrap

Status: complete.

Acceptance:

- Rust 2024 workspace/binary skeleton;
- local-first Rust CI on a dedicated self-hosted Windows runner;
- read-only `vmcell doctor` and provider probing;
- provider-neutral `Cell`, `Image`, `ProviderCapabilities`, and lifecycle model;
- Hyper-V and QEMU provider modules without VM mutation;
- Guest I/O interface with PowerShell Direct, QGA, and SSH placeholders;
- product scope, competitive landscape, and OpenStack boundary documented.

No VM creation is authorized by M0.

## M1 — Hyper-V cell foundation

Goal: first useful Windows-native disposable full-system cell.

Status: implementation review candidate; dedicated real Hyper-V acceptance remains pending.

Planned acceptance:

- Hyper-V prerequisite/capability probe reports exact missing prerequisites;
- image registration for an immutable VHDX base;
- image identity verification before cell creation;
- one differencing VHDX per cell;
- VM create/start/inspect/stop/destroy;
- CPU and memory configuration;
- strict ownership marker + local manifest reconciliation;
- safe/idempotent destroy;
- no automatic Hyper-V enablement, reboot, or host-global switch creation;
- provider integration tests on a dedicated Hyper-V-capable host.

## M2 — Windows guest control

Goal: make a Hyper-V Windows cell useful to automation without requiring guest networking.

Planned acceptance:

- PowerShell Direct readiness detection;
- command execution with exit code/stdout/stderr;
- copy-in and copy-out semantics;
- artifact collection;
- TTL model and explicit `vmcell gc`;
- guest-control recovery layered on the M1 ownership reconciliation model.

## M3 — QEMU reference provider

Goal: validate the provider abstraction against an independent VMM.

Planned acceptance:

- QMP connection and lifecycle;
- QCOW2 base + single overlay;
- QGA guest transport;
- WHPX acceptance on Windows first, because it can be developed alongside Hyper-V;
- explicit hardware-accelerator discovery;
- TCG/cross-architecture emulation only through explicit opt-in.

## M4 — Linux and macOS host acceptance

Goal: make the same QEMU provider portable rather than create separate KVM/HVF providers.

Planned acceptance:

- QEMU/KVM on a real Linux host;
- QEMU/HVF on a real macOS host;
- host-specific packaging/path/state behavior;
- architecture mismatch diagnostics;
- documented guest support matrix.

WSL or nested environments may be used for development experiments but are not substitutes for final host acceptance.

## M5 — Automation contract hardening

Goal: make `vmcell` safe to consume from CI and higher-level orchestration.

Planned acceptance:

- versioned JSON response schemas;
- documented exit-code taxonomy;
- stable ownership/reconciliation semantics;
- machine-readable `doctor` capability reports;
- deterministic failure categories;
- concurrency-lock hardening beyond the narrow M1 exclusive mutation lock;
- artifact/log retention policy.

## Later candidates

Only after the core lifecycle is stable:

- Secure Boot / TPM;
- nested virtualization;
- shared-folder abstractions;
- GPU/device capability discovery or assignment where host/provider support is reliable;
- Apple Virtualization.framework provider if QEMU/HVF leaves a material macOS gap;
- libvirt provider if it adds enough operational value to justify another dependency boundary;
- optional local scheduling hooks for `vmcell gc`.

## Explicit non-roadmap items

These are not deferred features; they belong outside the project unless the product boundary is intentionally redefined:

- OpenStack/cloud placement;
- multi-host scheduler;
- agent framework;
- GitHub Actions protocol implementation;
- physical HIL/device leasing;
- engineering-application automation;
- distributed database or image service;
- general VM inventory GUI.
