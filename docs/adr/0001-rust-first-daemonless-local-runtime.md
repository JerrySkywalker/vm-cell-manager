# ADR 0001: Rust-first daemonless local runtime

- Status: Accepted
- Date: 2026-08-08

## Context

The project must run across Windows, Linux, and macOS while remaining a small local execution primitive. A persistent controller would add installation, upgrade, service-account, IPC, recovery, and security surface before the core cell semantics are proven.

## Decision

Project-owned implementation code is Rust-first. The default product is a CLI/library-style local runtime with no required daemon.

Platform providers may invoke stable native interfaces or vendor executables when that is the correct integration boundary; Rust-first does not mean reimplementing Hyper-V, QEMU, PowerShell, or operating-system virtualization APIs.

Durable state is based on versioned local manifests plus provider-observed state.

## Consequences

- Cross-platform lifecycle/core logic stays in one language.
- Local commands remain easy to compose from scripts, CI, and future higher-level systems.
- TTL cleanup is explicit (`vmcell gc`) unless the user separately configures OS scheduling.
- A daemon may be proposed later only for a concrete capability that cannot be served safely by the CLI/runtime model.
