# VMCell one-week owner checklist

Status: **OWNER APPROVED IMPLEMENTATION BASELINE**
Contract: `vmcell.one-week-owner-checklist.v1`
This checklist is not execution authority.

## A. Approve the target

- [ ] I accept `OWNER_LOCAL_PREVIEW_PASS` as the one-week target.
- [ ] I understand this is not v1.0, public release, or support promotion.
- [ ] I approve Windows Server 2022 x86_64, Hyper-V Generation 2, Secure Boot,
      networkless guest operation, and PowerShell Direct.
- [ ] I approve deferring Apple, macOS, HVF, Windows 11/vTPM, Linux, QEMU,
      WHPX, Docker, JobSpec overlay, and public release.
- [ ] I approve stopping after one production correction or any unknown effect.

## B. Approve repository sequencing

- [ ] PR #77 must be independently revalidated before merge.
- [ ] PR #77 must merge normally into `dev` and exact-dev Hosted Windows/Linux
      CI must pass before the tooling PR begins.
- [ ] The qualification work uses one isolated branch/worktree and one PR.
- [ ] That PR may change tooling, fixtures, static tests, narrow CI, and docs,
      but not the production Rust execution path.
- [ ] `Cargo.lock`, version `0.4.1`, support rows, `main`, and
      `release/v0.4.1` must remain unchanged.
- [ ] Up to two bounded tooling correction cycles are acceptable.
- [ ] Self-hosted checkout-network failure is not mistaken for a source pass or
      source failure; required Hosted correctness checks remain mandatory.

## C. Before starting the unattended Codex goal

- [ ] I have reviewed all six draft files and recorded requested corrections.
- [ ] A fresh independent audit has confirmed the final plan is internally
      consistent and does not grant real-platform authority.
- [ ] The execution goal identifies exact repository, target branch, work
      branch, worktree, bounded Cargo target, and external checkpoint path.
- [ ] The goal requires immediate admission and checkpoint creation.
- [ ] The goal says to continue through useful authorized work rather than stop
      for expected nonblocking conditions.
- [ ] The goal defines valid stop conditions and correction budgets.
- [ ] The goal forbids Live preflight, elevation, image preparation, and any
      Hyper-V or host mutation.

## D. Image preparation decision

- [ ] I control legitimate Windows Server 2022 media/image rights.
- [ ] I accept that VMCell will not download, license, activate, or distribute
      Windows.
- [ ] I will not use `daily.vhdx`, a development VM disk, an attached disk, a
      differencing child, or another foreign image.
- [ ] The final VHDX will be fixed, parentless, detached, read-only, ordinary,
      non-reparse, and exactly hash-bound.
- [ ] The guest will have a configured local profile suitable for PowerShell
      Direct.
- [ ] Credentials and product keys will not enter Git, argv, environment,
      receipts, VMCell state, or Codex prompts.
- [ ] I will complete the provenance fields honestly; unknown security facts
      remain explicit owner-attestation requirements.
- [ ] I have checked local NTFS free capacity for the base VHDX, overlays,
      configuration, artifacts, evidence, and rollback margin.

## E. Operational path decision

- [ ] State root is an ordinary private non-reparse local NTFS directory.
- [ ] Runtime root is separate or explicitly contained and independently
      admitted.
- [ ] Image, package, and binary paths independently pass the same operational
      storage policy.
- [ ] Operational roots do not use the current file-backed/ReFS V: boundary.
- [ ] Sanitized evidence may use a separate location only after the corrected
      path-derived tooling classifies it as evidence rather than VM storage.
- [ ] No tool may create missing broad parents or overwrite a receipt.

## F. Before read-only Live preflight

- [ ] Qualification tooling PR is merged and exact-dev CI is green.
- [ ] Candidate, archive, binary, image, and provenance hashes are exact.
- [ ] The owner session is elevated only for attended read access.
- [ ] Hyper-V feature/module/cmdlets, VMMS, and provider read access are
      available without repair.
- [ ] No active Runner.Worker, VMCell writer, Codex writer, image writer, or
      competing virtualization writer exists.
- [ ] Foreign VMs and switches have been inventoried through sanitized counts
      and fingerprints.
- [ ] The evidence directory is new, ordinary, non-reparse, and empty.
- [ ] I understand `PREFLIGHT_ELIGIBLE` is not real-platform PASS.

## G. Before real Hyper-V authorization

- [ ] I have opened a new, exact, owner-attended goal rather than extending the
      unattended repository goal.
- [ ] The goal binds one candidate SHA, binary hash, image hash, host identity,
      path set, foreign prestate, and time window.
- [ ] I have reserved 6–10 attended hours.
- [ ] I can enter guest credentials interactively when requested.
- [ ] No production runner or unrelated development task needs the host during
      the window.
- [ ] The authorized mutations are limited to one exact-owned VM and disposable
      overlay lifecycle.
- [ ] Host feature, service, runner, switch, firewall, network, ACL, base image,
      and foreign-object mutation remain forbidden.
- [ ] I approve immediate stop on mismatch or unknown effect.

## H. During the real run

- [ ] Confirm final prestate and image hash immediately before mutation.
- [ ] After VM creation and before start, inspect Generation 2, Secure Boot,
      exact overlay parent, ownership, stopped state, and zero network adapters.
- [ ] Start only after all stopped-cell checks pass.
- [ ] Run the smallest readiness and harmless command test first.
- [ ] Verify stdout, stderr, zero exit, and typed nonzero exit.
- [ ] Verify bounded copy-in, copy-out, artifact SHA-256, and correlation.
- [ ] Complete three fresh-identity sequential runs.
- [ ] Exercise only one preselected bounded recovery case.
- [ ] Never replay an unknown-effect operation.
- [ ] Stop rather than repair or adopt ambiguous state.

## I. Closeout

- [ ] Stop/remove only exact-owned resources after fresh ownership checks.
- [ ] Repeat destroy to prove idempotent absence.
- [ ] Verify VM, runtime, overlay, and transient endpoints are absent.
- [ ] Verify immutable base SHA-256 is unchanged.
- [ ] Verify foreign VM/switch/service/feature/process fingerprints are
      unchanged.
- [ ] Keep raw host and guest evidence outside Git.
- [ ] Review the sanitized receipt for secrets, paths, host identifiers, raw
      commands, and guest output.
- [ ] Obtain an independent closeout audit.
- [ ] Do not promote support or publish a release in the same goal.

## J. Owner time budget

| Owner involvement | Expected time |
| --- | ---: |
| Review and approve planning bundle | 30–60 minutes |
| Image preparation | 2–6 hours |
| Live admission | 1–2 hours |
| Real acceptance | 6–10 hours |
| Closeout decision | 30–60 minutes |

Repository implementation and CI may run unattended. Image preparation and the
real provider window remain attended.

## K. Owner sign-off record

Complete only after auditing the final documents:

```text
PLAN_BUNDLE_VERSION=
PLAN_BUNDLE_SHA256_MANIFEST=
OWNER_DECISION=APPROVED | CORRECTIONS_REQUIRED | REJECTED
TARGET=OWNER_LOCAL_PREVIEW_PASS
WINDOWS_GUEST=WINDOWS_SERVER_2022_X86_64
VM_GENERATION=2
SECURE_BOOT=REQUIRED
OPERATIONAL_STORAGE=LOCAL_NTFS_PATH_DERIVED
PR77_SEQUENCE=APPROVED | NOT_APPROVED
TOOLING_PR_SCOPE=APPROVED | NOT_APPROVED
UNATTENDED_REPOSITORY_WORK=APPROVED | NOT_APPROVED
IMAGE_PREPARATION_AUTHORIZED=false
REAL_HYPERV_AUTHORIZED=false
PRODUCTION_CORRECTION_LIMIT=1
OWNER_NOTES=
DECIDED_AT_UTC=
```

Approval of the planning bundle still leaves image preparation and real
Hyper-V authorization false until separate explicit goals are issued.
