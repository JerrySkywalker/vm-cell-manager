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

Core CI observes Rust code on the dedicated self-hosted Windows `core` runner,
while the manual GitHub-hosted Windows lane supplies disposable repository
correctness evidence. Both remain non-privileged with respect to VM lifecycle.
Real provider acceptance must use a different, explicitly isolated runner/host
that exposes Hyper-V, KVM, HVF, or WHPX. See
`docs/adr/0016-disposable-correctness-and-windows-performance-evidence.md`.

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

The Windows portable-package contract is implemented by
`tools/package-windows.ps1` and tested by `tools/test-windows-package.ps1`.
Normal CI proves byte-identical repeat builds, fixed archive layout, checksums,
generated PowerShell completion, schema-v1 candidate package metadata, and
provenance without publishing. The separate `Package Windows` workflow is
manual `workflow_dispatch` only on the trusted runner and uploads a bounded
short-retention artifact; it does not tag, promote, or create a release. See
`docs/windows-portable-package.md` for the exact contract. Metadata remains
`candidate_only`; it is input for a future reviewed Scoop/WinGet submission,
not a package-manager publication action.

Public-facing Windows usage follows `docs/quickstart-windows.md` for the frozen
v0.1 candidate and `docs/windows-daily-driver.md` for the repository-local v0.2
workflow. Keep command spelling synchronized with CLI help and the portable
archive layout. Both documents must distinguish repository-local evidence and
a ready capability probe from dedicated-host Hyper-V/PowerShell Direct release
acceptance.

On a native Linux development host, `tools/check-linux.sh` runs the locked
repository-local portability suite. It does not install Rust, QEMU, KVM
components, packages, or change `/dev/kvm` permissions. WSL2 output is useful
development evidence but is never recorded as real Linux host acceptance.

The `Repository Validation` workflow provides the canonical manual exact-source
dispatcher and repository Linux lane. Its required lane selector runs exactly
one of Linux correctness, Windows correctness, or Linux R3 per dispatch. The
dispatcher concurrency key includes both that lane and the exact source SHA, so
a pending Windows or R3 request cannot replace a pending Linux request for the
same commit. A second pending request for the same lane and SHA remains
deduplicated without canceling an in-progress run. The
Windows and R3 choices invoke same-commit reusable workflows because GitHub only
registers a new `workflow_dispatch` file after it exists on the default branch.
It is manual `workflow_dispatch` only, accepts one exact lowercase 40-hex
repository commit, and checks out and proves that SHA on the declared
GitHub-hosted `ubuntu-24.04` x86_64 baseline. The ephemeral job installs exactly
Rust 1.85.0 with rustfmt and Clippy, then runs locked metadata, format, check,
Clippy, all-target tests, doc-tests, Linux shell parsing, the fixture-only
Linux KVM preflight contract, and the deterministic Linux portable-package
contract. The package gate builds into the external Cargo target, records the
observed GLIBC symbol floor, validates two byte-identical assemblies, and runs
only an unprivileged temporary-prefix install/remove smoke. It has read-only
repository permission, persists no checkout credentials, and has no `push` or
`pull_request` trigger. Repository tests may perform the existing non-mutating
KVM usability probe, which can open `/dev/kvm` read/write but issues no KVM
ioctl and creates no VM. The lane never repairs device permissions, loads
modules, or runs provider lifecycle commands. Hosted Linux evidence is native
repository compile/test evidence, not real KVM/QGA acceptance.

`Linux Reliability` is a separate manual `workflow_dispatch` R3 lane. It
accepts the same exact lowercase 40-hex source SHA, proves the same hosted
Ubuntu 24.04 x86_64/Rust 1.85 baseline, and uses only ephemeral Cargo, Rustup,
and target directories. GitHub's normal owner-repository dispatch authorization
is the source-trust admission: a job-level condition rejects a fork or
non-dispatch context before runner allocation, then the lane checks out that
owner repository's exact input SHA and proves it after
checkout. A SHA syntax check alone is never a trust or support claim. It runs
exactly the five fixed, ignored cases named in
`tools/reliability-campaign.json`; it does not call `tools/check-linux.sh`,
the package gate, provider commands, or a guest. The campaign script creates a
short-lived, strict receipt under the ephemeral runner temp directory binding
the exact source SHA, manifest digest, case count, toolchain, and terminal
result. It is never uploaded or used as real-platform, release, or support
evidence. The normal Linux lane remains the sole owner of the full repository
and package gates. Windows core checks only the lane's committed static safety
contract; it never runs the extended Linux campaign. The campaign has a hard
600-second workflow boundary, while each named case remains independently
bounded to 120 seconds and one MiB of captured test output.

The trusted Windows core gate keeps its fixed 30-minute timeout and canonical
commands. Its timing helper writes only allowlisted stage names, UTC timestamps,
and bounded durations to runner-temp records, then aggregates valid records into
the final Actions job summary. A partial, malformed, or absent record is ignored;
timing remains best-effort diagnostic evidence, never a gate result. It never
records commands, paths, runner identity, process data, environment values, logs,
or error text. Those markers cannot classify a product failure, authorize
recovery, replace an exact-head gate, or promote support.

`Windows Validation` is the disposable repository-correctness counterpart. The
manual dispatcher passes one exact lowercase 40-hex repository commit to a
same-commit reusable workflow on GitHub's stable standard `windows-2025` x64
image. The job uses `contents: read`, a pinned checkout with credential
persistence disabled, no secrets, OIDC, cache, artifact upload, environment, or
automatic trigger. It installs exactly Rust 1.85.0 into ephemeral runner state,
keeps Cargo output outside the checkout, and proves the source/package/MSRV
binding before locked check, Clippy, full tests, doc-tests, and the portable
Windows package contract. Its separately bounded 45-minute cold-VM timeout does
not alter the self-hosted R4 30-minute performance contract. The sanitized
summary receipt explicitly denies R4 runner-health, real-platform acceptance,
and support-promotion meaning. A hosted pass may establish repository
correctness; it cannot establish Hyper-V, WHPX, provider, guest, or shared-host
health.

The version-neutral A-G reliability train, its deterministic campaign protocol,
residual race limits, compatibility matrix, and long-term runner topology are
closed out in [the reliability closeout](reliability-closeout.md).

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
