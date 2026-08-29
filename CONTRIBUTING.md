# Contributing

VM Cell Manager is at an early architecture-first stage. Contributions are welcome, but changes should preserve the project's deliberately narrow scope: a daemonless, local, provider-based runtime for disposable full-system virtual-machine execution cells.

## Development principles

- Keep the core provider-neutral.
- Prefer capability discovery over platform assumptions.
- Do not make global host mutations implicitly.
- Do not manage foreign virtual machines by default.
- Keep `--json` output machine-readable and versioned.
- Treat image bases as immutable and execution overlays as disposable.
- Keep cloud scheduling, agent orchestration, and HIL outside this repository.

## Rust checks

Before opening a pull request, run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Generic repository validation consists of fixture, static, and unit tests. It
does not perform live Hyper-V, WHPX, KVM, QEMU, or HVF provider access in
generic CI.

Live provider tests are separate, explicitly admitted real-platform work. The
active pre-v1 paths are Windows x86_64 with Hyper-V or QEMU/WHPX and native
Linux x86_64 with QEMU/KVM. Any such live test must state its host prerequisites
and must not be presented as part of the generic repository-local gate.

macOS and HVF remain modeled as post-v1, non-blocking future direction. That
deferral is not a permanent rejection of Apple support; future live macOS/HVF
work requires its own admission and evidence.
