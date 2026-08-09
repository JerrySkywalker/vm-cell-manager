# ADR 0009: Rapid trunk CI/CD and separate platform acceptance

- Status: accepted
- Milestone: M5

## Decision

`main` is the repository integration branch. Product changes use short-lived
branches and complete this bounded cycle before another product slice starts:

```text
branch from green main
  -> focused local validation
  -> exact-head branch CI
  -> focused review
  -> merge to main
  -> exact-main CI
  -> delete branch
```

Active product PR depth targets zero or one. A transient depth of two is the
hard maximum and is allowed only while retargeting or recovering an already
admitted slice; it must not become a development stack. Unfinished slices do
not serve as the base for new product work.

Branch and main CI prove repository-local contracts. A repository-local
candidate with no P0/P1 finding and green exact-head CI is merge-eligible even
when real Hyper-V, PowerShell Direct, QEMU, KVM, WHPX, or HVF acceptance is
pending. Those platform gates remain explicit, separately tracked claims and
must never be reported as completed by unit, mock, WSL2, or core CI evidence.

The self-hosted Windows core workflow remains trusted push/workflow-dispatch
infrastructure. It must not automatically execute untrusted public-fork pull
request code. A future public contribution workflow requires a separate
non-privileged execution design before enabling `pull_request` triggers.

For one exact source and claim there is one canonical complete gate. Do not
queue duplicate canonical gates or churn same-head reruns. A failure is first
classified as product, runner, checkout, toolchain, or transient. Main
regressions are corrected promptly by the smallest safe fix-forward or by
reverting the responsible merge; history is never force-pushed to hide them.

## Consequences

- Review units stay small and independently reversible.
- Every accepted main commit has immediate exact-main evidence.
- M1-M4 real-platform acceptance remains visible without blocking unrelated
  repository-local contract work.
- Feature branches and associated worktrees are removed after accepted main.
- Long-lived stacked Draft trains are not the normal development model.
