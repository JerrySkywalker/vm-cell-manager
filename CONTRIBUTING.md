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

Platform-specific provider tests may require Hyper-V, QEMU, KVM, HVF, or WHPX and should be clearly marked when they cannot run in generic CI.
