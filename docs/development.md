# Development

## Toolchain

VM Cell Manager uses Rust 2024 edition with a declared minimum Rust version of 1.85.0 for the bootstrap phase.

Recommended local checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Because provider integration depends on host virtualization capabilities, generic CI validates portable Rust code while dedicated provider acceptance will later run on hosts that explicitly expose Hyper-V, KVM, HVF, or WHPX.

## Bootstrap safety rule

M0 provider code is read-only. A change that creates, starts, stops, destroys, enables, reconfigures, or otherwise mutates host virtualization state belongs to a later milestone and should not be disguised as a probe or discovery change.

## Platform boundaries

- Windows-native lifecycle work belongs under `src/providers/hyperv`.
- Portable QEMU lifecycle work belongs under `src/providers/qemu`; KVM/HVF/WHPX are accelerators, not separate top-level providers.
- PowerShell Direct, QGA, and SSH belong under `src/guest`.
- Application-specific tooling does not belong in provider or guest-transport modules.

## Dependencies

Prefer small, actively maintained, permissively licensed dependencies. New dependencies should have a clear purpose and should not pull cloud-control or daemon infrastructure into the local runtime.

A future `cargo deny check` CI job should enforce the repository's dependency and license policy once the initial lockfile exists.
