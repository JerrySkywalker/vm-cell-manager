# ADR 0016: Separate disposable correctness from Windows performance evidence

## Status

Accepted for repository-local CI. This decision does not change support or
real-platform acceptance.

## Context

The trusted self-hosted Windows core runner is useful for observing the
repository on the maintained Windows environment, but repeated shared-host
contention can exhaust its fixed 30-minute R4 budget without producing a source
failure. Treating that host-performance outcome as the only Windows correctness
gate couples repository merge eligibility to unrelated runner load.

GitHub requires a `workflow_dispatch` workflow to exist on the default branch.
PR-head validation therefore cannot directly register a newly added workflow.
The existing manual exact-SHA repository validation workflow is the dispatcher;
one required lane selector runs Linux correctness, Windows correctness, or R3,
and the reusable choices load from the same commit as the caller. The supplied
source SHA must also equal both caller and reusable-workflow SHAs. This preserves
exact candidate binding without changing the default `dev` branch, `main`, or
repeating another lane.

## Decision

Repository correctness and environment performance are independent evidence:

- `Windows Validation` is a manual, exact-SHA correctness lane on the stable
  standard GitHub-hosted `windows-2025` x64 image. It uses only
  `contents: read`, persists no checkout credential, accepts no secret or OIDC
  input, uses no cache, and uploads no artifact.
- The hosted lane installs the declared Rust 1.85.0 MSRV in an ephemeral Rustup
  home, keeps build output outside the checkout, and binds the exact source,
  Cargo version/MSRV, locked graph, format, PowerShell/workflow safety, Clippy,
  full tests, doc-tests, and deterministic Windows package contract.
- Its 45-minute correctness timeout covers cold toolchain/dependency setup and
  debug plus release builds on a fresh standard hosted VM. It is unrelated to
  and does not modify the self-hosted R4 30-minute performance contract.
- Its bounded pre-post job summary states only exact source, observed step
  status, total elapsed seconds, image, architecture, toolchain, and negative
  authority flags. The Actions job conclusion remains the correctness result;
  the summary is diagnostic only and is not uploaded.
- The self-hosted `Windows Core / Self-hosted` lane remains intact as R4
  observational/performance evidence. It cannot override a hosted source
  failure and a hosted pass cannot establish self-hosted runner health.

## Evidence semantics

| Evidence | Proves | Does not prove |
| --- | --- | --- |
| Hosted Windows correctness | repository compiles, tests, and packages on the declared disposable Windows tuple | R4 runner health, Hyper-V, WHPX, provider/guest behavior, support |
| Hosted Linux correctness | repository compiles, tests, and packages on the declared disposable Linux tuple | KVM/QGA acceptance or support |
| Hosted Linux R3 | bounded deterministic extended campaign | canonical correctness, soak, provider acceptance, support |
| Self-hosted Windows R4 | timing and outcome on the maintained shared Windows runner | repository failure when contention prevents completion |
| Dedicated real host R5 | only the separately authorized tuple and receipt | publication or another tuple |

No R0-R4 result promotes a support row. Provider acceptance remains R5 on a
dedicated host with a release-specific, owner-authorized receipt.

## Consequences

Merge eligibility requires P0=0/P1=0 plus exact-source hosted Windows and Linux
correctness and any slice-specific R3 evidence. A repeated same-signature R4
retry is not required to prove repository correctness. Future dedicated
`vmcell` self-hosted isolation may restore R4 predictability, but this decision
does not authorize runner, service, process, host, or provider mutation.
