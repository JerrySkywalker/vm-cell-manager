# M1 real-acceptance closeout 003

This repository-local receipt records the non-destructive closeout evidence for
`M1-REAL-ACCEPTANCE-CLOSEOUT-003`. The exact receipt-bearing commit, final
independent audit, and exact-head CI run are bound in Draft PR #3 because this
file cannot name the commit that contains itself.

## Binding

- harness: `JerrySkywalker/dev_governance_files@eba6016b9f68ce9539a793a8914efb0fa3b8a5b0`
- profile: `HIGH_ASSURANCE_WAVE_V1`
- authority/elasticity: `A3` / `B3`
- admitted layer: `L2`, maximum `L3`
- repository: `JerrySkywalker/vm-cell-manager`
- branch: `feat/m1-hyperv-cell-foundation`
- start head: `be1694504e027c82332dc37383fe9aa267c7975a`
- locally validated code head: `3557ff4c8b81ca680b1dbc385b787eeed9c917a9`
- pull request: Draft PR #3

Admission found the requested branch at the exact start head with a clean
single worktree, no stashes or Git lock files, and no remote divergence.
Independent read-only review of the start head found no P0 or P1 findings under
the documented M1 acceptance preconditions.

## Repository-local hardening

The closeout wave adds:

- real child-process abort coverage on both sides of atomic manifest replacement
  for all create/destroy persistence phases;
- cross-process duplicate-root locking and Windows directory-replacement denial;
- distinct-installation and cloned-installation cross-root authority rejection;
- repeated tombstone/name-reuse and runtime-reappearance fail-closed checks;
- canonical persisted provider GUID enforcement; and
- explicit documentation of the subprocess crash/locking validation tier.

The code head passed on official Rust 1.85.0 (`4d91de4e4`, 2025-02-17):

- locked, offline, all-feature Cargo metadata resolution;
- `cargo fmt --all -- --check`;
- `cargo check --locked --offline --all-targets --all-features`;
- `cargo clippy --locked --offline --all-targets --all-features -- -D warnings`;
- `cargo test --locked --offline --all-targets --all-features`: 64 passed;
- `cargo test --locked --offline --doc --all-features`: passed;
- all 10 embedded Hyper-V PowerShell files parsed successfully;
- zero forbidden host-global provider tokens; and
- read-only CLI/JSON smoke tests, including rejection of legacy
  `provider-list`, passed.

The toolchain, Cargo cache, target directory, and CLI smoke root were isolated
under the current user's temporary directory. Shared CI toolchain and runner
configuration were not modified.

## Real-provider admission

Result: `NOT_ADMITTED_SOFT_EXTERNAL_GATE`.

The available workstation could not prove the dedicated, isolated Hyper-V
acceptance context required by `docs/m1-hyperv-acceptance.md`:

- the current process was not elevated;
- read-only Hyper-V inventory failed with a virtualization-provider access error;
- host feature state could not be read in the current context;
- eight GitHub Actions runner services were active; and
- exclusive absence of external Hyper-V and filesystem writers could not be
  proven.

The core/trusted Actions runner was not stopped, privileged, repurposed, or used
for Hyper-V acceptance. No state root, VHDX, VM, switch, host feature, service,
or other provider resource was created or mutated. Consequently there were no
acceptance-owned resources and no cleanup obligation. A pre/post foreign-state
inventory comparison is unavailable because the required read access was not
admitted; foreign state was not touched by this wave.

Real acceptance remains the sole external gate. It requires a separately
approved dedicated Hyper-V host, elevated acceptance identity, ACL-exclusive
ordinary non-reparse state/runtime root, disposable ordinary non-reparse base
VHDX with recorded hash, read-only pre-state inventory, and an exclusive
provider-writer window.

## Evidence reuse boundary

The receipt commit changes documentation only. The Rust 1.85, PowerShell, and
CLI evidence above remains valid for the receipt-bearing head provided the
code, manifests, lockfile, scripts, and workflows are byte-identical to the
validated code head. Exact-head CI and independent audit are still required and
are recorded in Draft PR #3.
