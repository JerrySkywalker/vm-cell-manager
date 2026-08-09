# ADR 0009: Dev integration, release promotion, and separate platform acceptance

- Status: accepted
- Milestone: M5

## Decision

`main` is the stable release baseline. `dev` is the persistent
repository-local integration branch. Normal product work never targets
`main`; it uses short-lived `agent/*` branches and completes this bounded cycle
before another product slice starts:

```text
branch agent/* from green dev
  -> focused local validation
  -> exact-head branch CI
  -> focused review in a PR targeting dev
  -> merge to dev
  -> exact-dev CI
  -> delete agent branch and associated worktree
```

Active `agent/*` PR depth targets zero or one. A transient depth of two is the
hard maximum and is allowed only while retargeting, synchronizing, or recovering
an already admitted slice; it must not become a development stack. Unfinished
slices do not serve as the base for new product work. `agent/*` branches are
deleted after accepted exact-dev CI.

Branch and dev CI prove repository-local integration contracts. A
repository-local candidate with no P0/P1 finding and green exact-head CI is
merge-eligible even when real Hyper-V, PowerShell Direct, QEMU, KVM, WHPX, or
HVF acceptance is pending. Those platform gates remain explicit, separately
tracked claims and must never be reported as completed by unit, mock, WSL2, or
core CI evidence.

## Release promotion

A release starts only from a green exact `dev` head. Create a temporary frozen
`release/vX.Y.Z` branch at that commit, declare the repository-local and
real-platform acceptance claims required for that release, and permit only
release corrections on the branch. After the declared gates pass, promote the
release branch to `main` through a reviewed PR, verify exact-main CI, and then
create the immutable `vX.Y.Z` tag on the accepted main commit. Version tags are
never moved, deleted, or reused; a correction receives a new version.

After promotion, synchronize the accepted main history back to `dev` before
normal development resumes. Retire the release branch after exact-dev CI proves
the synchronization. A deferred platform gate may remain deferred only when the
release record says so explicitly; repository-local CI never implies that the
gate passed.

## Hotfixes

Hotfix branches are named `hotfix/*` and originate from the current stable
`main`. Validate and merge the hotfix to `main` through a reviewed PR, verify
exact-main CI, and issue a new immutable patch-version tag when the fix is
released. Then merge the accepted main history into a short `agent/*`
synchronization branch, open its PR to `dev`, verify exact-dev CI, and delete
both temporary branches. Never force-push, move a tag, or leave a fix only on
one long-lived branch.

The self-hosted Windows core workflow remains trusted push/workflow-dispatch
infrastructure. It must not automatically execute untrusted public-fork pull
request code. A future public contribution workflow requires a separate
non-privileged execution design before enabling `pull_request` triggers.

For one exact source and claim there is one canonical complete gate. Do not
queue duplicate canonical gates or churn same-head reruns. A failure is first
classified as product, runner, checkout, toolchain, or transient. Dev
regressions are corrected promptly by the smallest safe fix-forward or by
reverting the responsible merge. Main regressions use the hotfix procedure or
a safe revert. History is never force-pushed to hide them.

## Consequences

- Review units stay small and independently reversible.
- Every accepted dev integration has immediate exact-dev evidence, and every
  release or hotfix promotion has immediate exact-main evidence.
- M1-M4 real-platform acceptance remains visible without blocking unrelated
  repository-local contract work.
- Agent, release, and hotfix branches and associated worktrees are removed
  after their accepted synchronization point.
- Long-lived stacked Draft trains are not the normal development model.
