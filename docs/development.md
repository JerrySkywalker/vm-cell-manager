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

## Branch and integration policy

`main` is the stable release baseline and `dev` is the persistent
repository-local integration branch. Start each independently reviewable slice
from green `dev` on an ephemeral `agent/*` branch. Normal development PRs target
`dev`, never `main`. Keep active agent PR depth at zero or one and never exceed
a transient depth of two during retarget, synchronization, or recovery. A slice
completes focused validation, exact-head CI, focused review, merge to `dev`,
exact-dev CI, and agent branch/worktree cleanup before the next slice starts.

Do not repeat a canonical gate for an unchanged head and claim. Classify CI
failures before changing product code, and fix-forward or revert promptly if
`dev` regresses. The self-hosted core workflow is push/workflow-dispatch only;
do not add an automatic untrusted-fork `pull_request` path to that runner.

Release candidates use temporary frozen `release/vX.Y.Z` branches created from
green exact `dev`. Their release record declares which repository-local and
real-platform gates are required or explicitly deferred. Promote an accepted
release to `main` by reviewed PR, verify exact-main CI, create the immutable
`vX.Y.Z` tag on that accepted commit, synchronize main back to `dev`, verify
exact-dev CI, and remove the temporary release branch. Never move, delete, or
reuse a version tag.

Release fixes use `hotfix/*` branches created from `main`. Merge the reviewed
fix to `main`, verify exact-main CI, create a new patch-version tag when
releasing it, and synchronize the accepted main history back through a short
`agent/*` PR to `dev`. Delete both temporary branches after exact-dev CI. Never
force-push or allow a hotfix to remain unsynchronized.

Repository-local merge eligibility and real-platform acceptance are distinct.
P0/P1-clean, exact-head-green work may merge while separately documented
Hyper-V, PowerShell Direct, QEMU, KVM, WHPX, or HVF acceptance remains pending.
Never describe mock, WSL2, or core CI evidence as real-provider acceptance.

On a native Linux development host, `tools/check-linux.sh` runs the locked
repository-local portability suite. It does not install Rust, QEMU, KVM
components, packages, or change `/dev/kvm` permissions. WSL2 output is useful
development evidence but is never recorded as real Linux host acceptance.

## Provider safety rule

M0 probes remain read-only. M1 Hyper-V mutation is reachable only through ownership-checked lifecycle commands carrying an engine-issued installation/runtime authority and must not be invoked as part of ordinary unit/core CI. No code path may automatically enable host virtualization features, reboot, modify switches, or mutate a VM by name alone.

## M1 validation tiers

1. Unit and contract tests use a mocked provider/executor and temporary state roots.
2. Windows safety tests use real subprocess aborts and cross-process filesystem contention to exercise manifest crash atomicity, duplicate-root locking, and state-directory replacement resistance without invoking Hyper-V.
3. Core CI runs format, Clippy, and all non-destructive tests.
4. Real Hyper-V acceptance is a separately authorized activity on a dedicated host with a disposable VHDX, an isolated state root, and pre/post foreign-VM and switch checks.

M5 automation contract tests are repository-local and provider-neutral. They
exhaust typed probe-status/capability contradictions for both built-in provider
names, exercise every provider fault category, and run the compiled CLI to
verify JSON/human exit behavior and legacy-command migration. These tests do
not substitute for real provider or guest acceptance.

## Platform boundaries

- Windows-native lifecycle work belongs under `src/providers/hyperv`.
- Provider-neutral mutation ordering and ownership proof belong under `src/engine`.
- Portable QEMU lifecycle work belongs under `src/providers/qemu`; KVM/HVF/WHPX are accelerators, not separate top-level providers.
- Unix state is private-owner state: directories `0700`, files `0600`, and
  authority-bearing opens use no-follow/inode checks.
- PowerShell Direct, QGA, and SSH belong under `src/guest`.
- Application-specific tooling does not belong in provider or guest-transport modules.

## Dependencies

Prefer small, actively maintained, permissively licensed dependencies. New dependencies should have a clear purpose and should not pull cloud-control or daemon infrastructure into the local runtime.

A future `cargo deny check` CI job should enforce the repository's dependency and license policy once the initial lockfile exists.
