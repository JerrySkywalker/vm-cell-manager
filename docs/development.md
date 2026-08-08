# Development

## Toolchain

VM Cell Manager uses Rust 2024 edition with a declared minimum Rust version of 1.85.0 for the bootstrap phase.

Recommended local checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

The declared compiler floor is proved separately with an isolated Rust 1.85.0 toolchain and the locked graph:

```bash
cargo metadata --locked --offline --all-features --format-version 1
cargo check --locked --workspace --all-targets --all-features
cargo test --locked --workspace --all-targets --all-features
cargo test --locked --workspace --all-features --doc
```

Core CI validates Rust code on the dedicated self-hosted Windows `core` runner. It remains non-privileged with respect to VM lifecycle. Real provider acceptance must use a different, explicitly isolated runner/host that exposes Hyper-V, KVM, HVF, or WHPX.

## Provider safety rule

M0 probes remain read-only. M1 Hyper-V mutation is reachable only through ownership-checked lifecycle commands carrying an engine-issued installation/runtime authority and must not be invoked as part of ordinary unit/core CI. No code path may automatically enable host virtualization features, reboot, modify switches, or mutate a VM by name alone.

## M1 validation tiers

1. Unit and contract tests use a mocked provider/executor and temporary state roots.
2. Core CI runs format, Clippy, and all non-destructive tests.
3. Real Hyper-V acceptance is a separately authorized activity on a dedicated host with a disposable VHDX, an isolated state root, and pre/post foreign-VM and switch checks.

## Platform boundaries

- Windows-native lifecycle work belongs under `src/providers/hyperv`.
- Provider-neutral mutation ordering and ownership proof belong under `src/engine`.
- Portable QEMU lifecycle work belongs under `src/providers/qemu`; KVM/HVF/WHPX are accelerators, not separate top-level providers.
- PowerShell Direct, QGA, and SSH belong under `src/guest`.
- Application-specific tooling does not belong in provider or guest-transport modules.

## Dependencies

Prefer small, actively maintained, permissively licensed dependencies. New dependencies should have a clear purpose and should not pull cloud-control or daemon infrastructure into the local runtime.

A future `cargo deny check` CI job should enforce the repository's dependency and license policy once the initial lockfile exists.
